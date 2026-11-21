//! 静态 GLB 网格的有界捕获、后台 Trimesh 烘焙与完整采用。
//!
//! 避免把高密洞穴交给同步 ColliderConstructorHierarchy。每帧最多复制128 KiB、
//! 访问64个节点，全域一个捕获和一个CPU槽。候选只保存已完成形状，整批可见时才
//! 安装真实 Collider；活动预览和热更保留上一完整形状直到新形状成功。

use super::{
    ScenePlacementInstance,
    components::{PlacementPendingInstance, PlacementPendingRevision, PlacementWorldInstanceReady},
    resources::{ScenePlacementRuntimeState, ScenePlacementRuntimeStatus},
};
use crate::shared::world_asset_lifecycle::WorldAssetLifecycleIndex;
use avian3d::prelude::{Collider, ColliderConstructorHierarchyReady, ColliderDisabled, RigidBody, TrimeshFlags};
use bevy::world_serialization::WorldInstanceReady;
use bevy::{
    asset::{AssetEvent, AssetId},
    mesh::{Indices, PrimitiveTopology, VertexAttributeValues},
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use std::{
    collections::{BTreeSet, HashMap, HashSet, VecDeque},
    ops::Bound::{Excluded, Unbounded},
    time::Instant,
};

mod components;
mod resources;
mod static_shape; // 真正Trimesh查询，静态质量与包围球元数据不在插入帧重算。
pub(crate) use static_shape::StaticTerrainShape; // 通用碰撞LOD只能后台适配真正静态包装，不猜测Custom几何。
mod types;
pub use components::DeferredTerrainPreviewCollider;
pub(super) use components::FormalTerrainPatchCollider;
pub(crate) use components::InstallTerrainPatchCollider;
pub use components::TerrainPatchColliderFailure;
pub(crate) use components::TerrainPatchColliderReady;
pub use components::TerrainPatchCollisionRequest;
pub(super) use components::TerrainColliderProxy;
pub(crate) use components::TerrainPhysicsProxy;
use components::{AuthorCollisionSuppression, TerrainMeshSources, TerrainWorldReady};
pub(crate) use resources::TerrainCollisionGarbage;
pub(super) use resources::TerrainCollisionRuntime;
use resources::MeshChangeSweep;
use types::{Capture, Cooked, MeshCapture};

const COPY_BYTES: usize = 128 * 1024;
const ROOTS_PER_FRAME: usize = 128; // 分帧查询预算，只控制一次推进的工作量。
const MAX_VERTICES: usize = 262_144;
const MAX_TRIANGLES: usize = 262_144;
const MAX_MESHES: usize = 32;
// Parry 的 BvhNodeData 为子树叶数保留低 30 位，高两位用于变化标记。
const PARRY_BVH_MAX_LEAVES: usize = (1 << 30) - 1;

fn sync_proxy_participation(world: &mut World, proxy: Entity, enabled: bool) {
    let Ok(mut proxy) = world.get_entity_mut(proxy) else { return; };
    if enabled {
        if proxy.take::<AuthorCollisionSuppression>().is_some() {
            proxy.remove::<ColliderDisabled>();
        }
    } else if !proxy.contains::<ColliderDisabled>() {
        proxy.insert((ColliderDisabled, AuthorCollisionSuppression));
    }
}

impl ScenePlacementInstance {
    /// 编辑器临时切换完整 GLB 的碰撞；只改变当前摆放拥有的独立物理根。
    pub fn set_editor_collision_participation(world: &mut World, root: Entity, enabled: bool) {
        if world.get::<Self>(root).is_none() { return; }
        if enabled {
            world.entity_mut(root).remove::<AuthorCollisionSuppression>();
        } else if world.get::<TerrainPatchCollisionRequest>(root).is_some() {
            // 正在准备的新代理也读取这个标记，不会在异步结果迟到时恢复禁用碰撞。
            world.entity_mut(root).insert(AuthorCollisionSuppression);
        }
        if let Some(proxy) = world.get::<TerrainColliderProxy>(root).map(|proxy| proxy.0) {
            sync_proxy_participation(world, proxy, enabled);
        }
    }
}

pub(super) fn install_ready_collider(
    event: On<Insert<InstallTerrainPatchCollider>>,
    ready: Query<(&TerrainPatchColliderReady, &GlobalTransform)>,
    mut commands: Commands,
) {
    if let Ok((ready, transform)) = ready.get(event.entity) {
        install_physics_proxy(
            &mut commands,
            event.entity,
            *transform,
            ready.physical.clone(),
        );
        commands
            .entity(event.entity)
            .insert(FormalTerrainPatchCollider)
            .remove::<InstallTerrainPatchCollider>();
    }
}

/// 物理根使用单位缩放与已烘焙顶点，Avian插入和固定步均无需重新缩放大BVH。
/// 不挂在GLB实例树下，热重建可视节点时仍保留上一完整支撑；作者根负责有界回收。
fn install_physics_proxy(
    commands: &mut Commands,
    root: Entity,
    global: GlobalTransform,
    collider: Collider,
) {
    let matrix = global.to_matrix();
    let (_, rotation, translation) = global.to_scale_rotation_translation();
    let transform = Transform {
        rotation,
        translation,
        ..default()
    };
    commands.queue(move |world: &mut World| {
        if world.get_entity(root).is_err() {
            if let Some(mut garbage) = world.get_resource_mut::<TerrainCollisionGarbage>() {
                garbage.retain_shape(&collider);
            }
            return;
        }
        let existing = world
            .get::<TerrainColliderProxy>(root)
            .map(|proxy| proxy.0)
            .filter(|entity| world.get::<TerrainPhysicsProxy>(*entity).is_some());
        // Avian on_insert从GlobalTransform分解缩放；旋转矩阵的舍入残差也可能触发
        // Custom形状同步重缩放大BVH。此独占命令内先提供精确单位缩放，物理位姿显式
        // 来自Position/Rotation，再立即恢复渲染变换；任何系统都观察不到中间状态。
        let bundle = (
            collider,
            transform,
            GlobalTransform::IDENTITY,
            avian3d::prelude::Position(translation),
            avian3d::prelude::Rotation(rotation),
        );
        let proxy = if let Some(existing) = existing {
            world.entity_mut(existing).insert(bundle);
            // 所有权组件不能整项替换，否则Discard观察器会回收正在复用的物理实体。
            world.get_mut::<TerrainColliderProxy>(root).unwrap().1 = matrix;
            existing
        } else {
            let proxy = world
                .spawn((
                    bundle,
                    TerrainPhysicsProxy,
                    RigidBody::Static,
                    Name::new("三维地形独立物理根"),
                ))
                .id();
            world
                .entity_mut(root)
                .insert(TerrainColliderProxy(proxy, matrix));
            proxy
        };
        world
            .entity_mut(proxy)
            .insert(GlobalTransform::from(transform));
        if world.get::<AuthorCollisionSuppression>(root).is_some() {
            sync_proxy_participation(world, proxy, false);
        }
    });
}

/// 按实体顺序跨帧轮转，已完成项或者暂时未就绪项都不能挡住后续根。
fn poll_roots(registered: &BTreeSet<Entity>, cursor: Option<Entity>) -> impl Iterator<Item = Entity> + '_ {
    registered
        .range((cursor.map_or(Unbounded, Excluded), Unbounded))
        .chain(registered.iter())
        .take(registered.len().min(ROOTS_PER_FRAME))
        .copied()
}

/// 活动作者预览只同步已烘焙形状的位姿；缩放或资产换代仍等待后台完整结果。
pub(super) fn sync_preview_physics_pose(
    runtime: Res<TerrainCollisionRuntime>,
    mut cursor: Local<Option<Entity>>,
    roots: Query<
        (
            &TerrainPatchColliderReady,
            &TerrainPatchCollisionRequest,
            &GlobalTransform,
            &ScenePlacementInstance,
            Option<&TerrainColliderProxy>,
        ),
        (
            Without<DeferredTerrainPreviewCollider>,
            Without<FormalTerrainPatchCollider>,
            Without<PlacementPendingInstance>,
        ),
    >,
    mut commands: Commands,
) {
    if roots.is_empty() {
        return;
    }
    for root in poll_roots(&runtime.registered, *cursor) {
        *cursor = Some(root);
        let Ok((ready, request, global, instance, proxy)) = roots.get(root) else {
            continue;
        };
        if !instance.is_active()
            || !instance.uses_async_mesh_collision()
            || !ready.matches(request)
            || ready.scale != global.scale()
        {
            continue;
        }
        let matrix = global.to_matrix();
        if proxy.is_some_and(|proxy| proxy.1 == matrix)
            || !matrix.is_finite()
            || !matrix.inverse().is_finite()
        {
            continue;
        }
        install_physics_proxy(&mut commands, root, *global, ready.physical.clone());
    }
}

pub(super) fn retire_physics_proxy(
    event: On<Discard<TerrainColliderProxy>>,
    roots: Query<&TerrainColliderProxy>,
    mut commands: Commands,
) {
    if let Ok(proxy) = roots.get(event.entity) {
        commands.entity(proxy.0).try_despawn(); // 无后代的唯一物理实体，不能随普通场景树拖延回收。
    }
}

pub fn terrain_placement_collision(
    record: &feathrise_scene_content::ScenePlacementRecord,
) -> Option<(RigidBody, TerrainPatchCollisionRequest)> {
    if record.terrain_patch.is_none() && !record.asset_target.has_gltf_collision() {
        return None;
    }
    Some((
        RigidBody::Static,
        TerrainPatchCollisionRequest { generation: 1 },
    ))
}

pub(super) fn invalidate_world_ready(
    event: On<WorldInstanceReady>,
    mut roots: Query<(
        &bevy::world_serialization::WorldInstance,
        &mut TerrainPatchCollisionRequest,
    )>,
    mut commands: Commands,
) {
    if let Ok((instance, mut request)) = roots.get_mut(event.entity)
        && **instance == event.instance_id
    {
        request.generation = request.generation.wrapping_add(1).max(1);
        commands.entity(event.entity).insert(TerrainWorldReady);
    }
}

pub(super) fn invalidate_world_root(
    event: On<Insert<WorldAssetRoot>>,
    mut roots: Query<&mut TerrainPatchCollisionRequest>,
    mut commands: Commands,
) {
    if let Ok(mut request) = roots.get_mut(event.entity) {
        request.generation = request.generation.wrapping_add(1).max(1);
        commands.entity(event.entity).remove::<TerrainWorldReady>();
    }
}

pub(super) fn retire_ready_shape(
    event: On<Discard<TerrainPatchColliderReady>>,
    ready: Query<&TerrainPatchColliderReady>,
    mut garbage: ResMut<TerrainCollisionGarbage>,
) {
    if let Ok(ready) = ready.get(event.entity) {
        garbage.retain_shape(&ready.collider);
        garbage.retain_shape(&ready.physical);
    }
}

/// Avian可能为非均匀缩放创建独立形状，正式Collider自身的最后引用也交后台释放。
pub(super) fn retire_installed_shape(
    event: On<Discard<Collider>>,
    installed: Query<&Collider, With<TerrainPhysicsProxy>>,
    mut garbage: ResMut<TerrainCollisionGarbage>,
) {
    if let Ok(shape) = installed.get(event.entity) {
        garbage.retain_shape(shape);
    }
}

/// 采用边界立即撤销旧物理和请求，实体树回收随后分帧完成。
pub(super) fn retire_formal_terrain(commands: &mut Commands, roots: &[Entity]) {
    for &root in roots {
        commands
            .entity(root)
            .try_remove::<(Collider, TerrainPatchCollisionRequest)>();
    }
}

pub(super) fn register_request(
    event: On<Add<TerrainPatchCollisionRequest>>,
    mut runtime: ResMut<TerrainCollisionRuntime>,
) {
    runtime.registered.insert(event.entity);
}

pub(super) fn unregister_request(
    event: On<Remove<TerrainPatchCollisionRequest>>,
    mut runtime: ResMut<TerrainCollisionRuntime>,
    mut commands: Commands,
) {
    runtime.registered.remove(&event.entity);
    runtime.failed.remove(&event.entity);
    commands
        .entity(event.entity)
        .try_remove::<TerrainColliderProxy>();
}

fn capture_step(
    capture: &mut Capture,
    meshes: &Assets<Mesh>,
    children: &Query<&Children>,
    visuals: &Query<(&Mesh3d, &GlobalTransform)>,
) -> Result<bool, String> {
    let mut bytes = COPY_BYTES;
    let mut nodes = 64;
    let legacy_limits = !capture.streamed && !capture.scene_collision;
    while bytes > 0 {
        if let Some(current) = capture.current.as_mut() {
            let mesh = meshes
                .get(&current.handle)
                .ok_or("三维地形网格在捕获期间被移除")?;
            let Some(VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                return Err("三维地形缺少三维位置属性".into());
            };
            let indices = mesh.indices().ok_or("三维地形必须包含三角索引")?;
            if positions.len() != current.vertex_count || indices.len() != current.index_count {
                return Err("三维地形网格在捕获期间换代".into());
            }
            let from = current.vertices.len();
            let end = (from + bytes / 12).min(positions.len());
            current
                .vertices
                .extend(positions[from..end].iter().map(|p| Vec3::from_array(*p)));
            bytes -= (end - from) * 12;
            if end < positions.len() {
                return Ok(false);
            }
            let from = current.indices.len();
            let end = (from + bytes / 4).min(indices.len());
            match indices {
                Indices::U32(values) => current.indices.extend_from_slice(&values[from..end]),
                Indices::U16(values) => current
                    .indices
                    .extend(values[from..end].iter().map(|i| u32::from(*i))),
            }
            bytes -= (end - from) * 4;
            if end < indices.len() {
                return Ok(false);
            }
            let current = capture.current.take().expect("本帧正在捕获网格");
            // 矩阵变换移交后台，不把一次完整顶点变换塞到捕获结束帧。
            capture.meshes.push((current.vertices, current.indices));
            capture.matrices_mut().push(current.matrix);
            continue;
        }
        if nodes == 0 {
            return Ok(false);
        }
        if let Some((entity, offset)) = capture.child_cursor {
            if let Ok(children) = children.get(entity) {
                let end = (offset + nodes).min(children.len());
                capture
                    .stack
                    .extend(children.iter().skip(offset).take(end - offset));
                nodes -= end - offset;
                capture.child_cursor = (end < children.len()).then_some((entity, end));
            } else {
                capture.child_cursor = None;
            }
            continue; // 单个根有很多块时，子列表也分帧遍历，不能在首帧复制整条列表。
        }
        let Some(entity) = capture.stack.pop_front() else {
            return Ok(true);
        };
        nodes -= 1;
        capture.nodes += 1;
        if legacy_limits && capture.nodes > 1024 {
            return Err("三维地形节点超过1024项预算".into());
        }
        if let Ok((handle, transform)) = visuals.get(entity) {
            let Some(mesh) = meshes.get(&handle.0) else {
                capture.stack.push_front(entity);
                capture.nodes -= 1;
                return Ok(false);
            };
            if mesh.primitive_topology() != PrimitiveTopology::TriangleList
                || mesh.contains_attribute(Mesh::ATTRIBUTE_JOINT_INDEX)
                || mesh.has_morph_targets()
            {
                return Err("三维地形只接受静态三角网格".into());
            }
            let Some(VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                return Err("三维地形位置属性无效".into());
            };
            let count = mesh.indices().ok_or("三维地形缺少索引")?.len();
            if count == 0 || !count.is_multiple_of(3) {
                return Err("三维地形索引不是非空三角列表".into());
            }
            capture.vertices = capture
                .vertices
                .checked_add(positions.len())
                .ok_or("三维地形顶点数量溢出")?;
            capture.triangles = capture
                .triangles
                .checked_add(count / 3)
                .ok_or("三维地形三角数量溢出")?;
            if legacy_limits
                && (capture.vertices > MAX_VERTICES
                    || capture.triangles > MAX_TRIANGLES
                    || capture.meshes.len() >= MAX_MESHES)
            {
                return Err("三维地形超过单实例网格/顶点/三角预算，请拆分资产".into());
            }
            if capture.scene_collision {
                if u32::try_from(capture.vertices).is_err() {
                    return Err("顶点总数超出三角网格的32位索引表示范围".into());
                }
                if capture.triangles > PARRY_BVH_MAX_LEAVES {
                    return Err("三角面总数超出 Parry 碰撞树的叶数表示范围".into());
                }
            }
            let mut vertices = Vec::new();
            let mut indices = Vec::new();
            if capture.scene_collision {
                // 先预留空缓冲，再按帧复制，避免密集资产扩容时搬运此前已捕获的数据。
                vertices.try_reserve_exact(positions.len())
                    .map_err(|error| format!("顶点缓冲分配失败：{error}"))?;
                indices.try_reserve_exact(count)
                    .map_err(|error| format!("三角索引缓冲分配失败：{error}"))?;
            }
            capture.sources.insert(handle.0.id());
            capture.mesh_entities.push((entity, handle.0.clone()));
            capture.current = Some(MeshCapture {
                handle: handle.0.clone(),
                matrix: capture.inverse_root * transform.to_matrix(),
                vertex_count: positions.len(),
                index_count: count,
                vertices,
                indices,
            });
        }
        if let Ok(children) = children.get(entity) {
            if legacy_limits && capture.stack.len() + children.len() > 1024 {
                return Err("三维地形节点超过1024项预算".into());
            }
            capture.child_cursor = Some((entity, 0));
        }
    }
    Ok(false)
}

// 与顶点数组同属唯一捕获快照；只在后台使用矩阵。
impl Capture {
    fn matrices_mut(&mut self) -> &mut Vec<Mat4> {
        &mut self.matrices
    }
}

fn cook(capture: Capture) -> Cooked {
    let shape = (|| {
        if capture.meshes.is_empty() {
            return Err("三维地形没有可用的真实网格".into());
        }
        // 一个实例的所有primitive合并为同一个Trimesh；保留真实空腔与倒扣。
        // Parry禁止Compound嵌套Trimesh，不能按材质再包一层复合形状。
        let mut merged_vertices = Vec::new();
        merged_vertices
            .try_reserve_exact(capture.vertices)
            .map_err(|e| format!("三维地形碰撞顶点内存分配失败：{e}"))?;
        let mut triangles = Vec::new();
        triangles
            .try_reserve_exact(capture.triangles)
            .map_err(|e| format!("三维地形碰撞三角内存分配失败：{e}"))?;
        let mut welded = HashMap::<[u32; 3], u32>::new();
        for ((mut vertices, indices), matrix) in capture.meshes.into_iter().zip(capture.matrices) {
            for vertex in &mut vertices {
                *vertex = matrix.transform_point3(*vertex);
            }
            if vertices.iter().any(|p| !p.is_finite())
                || indices.iter().any(|i| *i as usize >= vertices.len())
            {
                return Err("三维地形含非有限位置或越界索引".into());
            }
            if capture.streamed {
                // 显示块在接缝处会复制顶点；物理必须重新焊接，内边缘处理才能识别连续地面。
                let mut remap = Vec::with_capacity(vertices.len());
                for vertex in vertices {
                    let key = vertex
                        .to_array()
                        .map(|v| if v == 0. { 0 } else { v.to_bits() });
                    let next = u32::try_from(merged_vertices.len())
                        .map_err(|_| "三维地形碰撞超出32位顶点索引格式")?;
                    let index = *welded.entry(key).or_insert_with(|| {
                        merged_vertices.push(vertex);
                        next
                    });
                    remap.push(index);
                }
                triangles.extend(indices.chunks_exact(3).map(|i| {
                    [
                        remap[i[0] as usize],
                        remap[i[1] as usize],
                        remap[i[2] as usize],
                    ]
                }));
                continue;
            }
            let base = u32::try_from(merged_vertices.len())
                .map_err(|_| "三维地形碰撞超出32位顶点索引格式")?;
            if merged_vertices
                .len()
                .checked_add(vertices.len())
                .is_none_or(|n| n > u32::MAX as usize)
            {
                return Err("三维地形碰撞超出32位顶点索引格式".into());
            }
            triangles.extend(
                indices
                    .chunks_exact(3)
                    .map(|i| [i[0] + base, i[1] + base, i[2] + base]),
            );
            merged_vertices.extend(vertices);
        }
        let mesh = avian3d::parry::shape::TriMesh::with_flags(
            merged_vertices,
            triangles,
            TrimeshFlags::FIX_INTERNAL_EDGES.into(),
        )
        .map_err(|error| format!("三维地形三角网格无法烘焙：{error}"))?;
        let query = static_shape::StaticTerrainShape::new(mesh);
        let physical = if capture.scale == Vec3::ONE {
            avian3d::parry::shape::SharedShape::new(query.clone())
        } else {
            avian3d::parry::shape::SharedShape(std::sync::Arc::from(
                avian3d::parry::shape::Shape::scale_dyn(&query, capture.scale, 10)
                    .expect("已校验正缩放Trimesh"),
            ))
        };
        Ok((
            Collider::from(avian3d::parry::shape::SharedShape::new(query)),
            Collider::from(physical),
        ))
    })();
    Cooked {
        root: capture.root,
        generation: capture.generation,
        inverse_root: capture.inverse_root,
        scale: capture.scale,
        shape,
        meshes: capture.mesh_entities.into(),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn drive_terrain_collision(
    mut commands: Commands,
    mut runtime: ResMut<TerrainCollisionRuntime>,
    meshes: Res<Assets<Mesh>>,
    mut events: MessageReader<AssetEvent<Mesh>>,
    children: Query<&Children>,
    visuals: Query<(&Mesh3d, &GlobalTransform)>,
    mut roots: Query<(
        Entity,
        &mut TerrainPatchCollisionRequest,
        &ScenePlacementInstance,
        Option<&WorldAssetRoot>,
        Option<&TerrainWorldReady>,
        Option<&TerrainPatchColliderReady>,
        Option<&TerrainMeshSources>,
        &GlobalTransform,
        Option<&FormalTerrainPatchCollider>,
    )>,
    mut state: ResMut<ScenePlacementRuntimeState>,
    mut status: ResMut<ScenePlacementRuntimeStatus>,
    mut garbage: ResMut<TerrainCollisionGarbage>,
    world_assets: Option<Res<WorldAssetLifecycleIndex>>,
    pending_roots: Query<&PlacementPendingRevision>,
    deferred: Query<(), With<DeferredTerrainPreviewCollider>>,
    uploading: Query<(), With<super::terrain_mesh_upload::TerrainMeshUpload>>,
    fields: Query<&crate::VolumeSurfaceFields>,
) {
    let mesh_events_caught_up = {
        let runtime = &mut *runtime;
        // Bevy消息只有短寿命，先合并真实换代ID，再分帧撤销受影响根的旧就绪证明。
        runtime.pending_mesh_changes.extend(events.read().filter_map(|event| match event {
            AssetEvent::Modified { id } | AssetEvent::Removed { id } => Some(*id),
            _ => None,
        }));
        if runtime.mesh_sweep.is_none() && !runtime.pending_mesh_changes.is_empty() {
            runtime.mesh_sweep = Some(MeshChangeSweep {
                changed: std::mem::take(&mut runtime.pending_mesh_changes),
                cursor: None,
            });
        }
        if let Some(sweep) = &mut runtime.mesh_sweep {
            let mut remaining = runtime.registered.range((sweep.cursor.map_or(Unbounded, Excluded), Unbounded));
            for &root in remaining.by_ref().take(ROOTS_PER_FRAME) {
                sweep.cursor = Some(root);
                let Ok((_, mut request, _, world, _, _, sources, _, _)) = roots.get_mut(root) else { continue; };
                let source_changed = sources.is_some_and(|sources| !sources.0.is_disjoint(&sweep.changed));
                let capture_changed = runtime.capture.as_ref().is_some_and(|capture| {
                    capture.root == root && !capture.sources.is_disjoint(&sweep.changed)
                });
                if source_changed || capture_changed {
                    request.generation = request.generation.wrapping_add(1).max(1);
                    let world_epoch = world.and_then(|world| {
                        world_assets.as_ref()?.current_ready_epoch(root, world.0.id())
                    });
                    invalidate_candidate(root, &mut state, &mut commands, &pending_roots, world_epoch);
                }
            }
            if remaining.next().is_none() {
                runtime.mesh_sweep = None;
            }
        }
        runtime.mesh_sweep.is_none() && runtime.pending_mesh_changes.is_empty()
    };
    if state.collision_mesh_events_caught_up != mesh_events_caught_up {
        state.collision_mesh_events_caught_up = mesh_events_caught_up;
    }
    if !mesh_events_caught_up {
        return; // 队尾根还可能持有旧形状，暂不继续捕获或采用后台完成值。
    }
    if let Some((root, generation, started)) = runtime.started
        && started.elapsed().as_secs() >= 30
        && !roots
            .get(root)
            .is_ok_and(|(_, _, instance, _, _, _, _, _, _)| {
                instance.terrain_patch().is_some_and(|p| p.streamed_volume)
            })
        && runtime.failed.get(&root) != Some(&generation)
    {
        runtime.failed.insert(root, generation);
        reject(
            root_is_pending(root, &state, &pending_roots),
            &mut state,
            &mut status,
            "三维地形碰撞准备超过30秒，保留上一完整形状".into(),
        );
    }
    if runtime
        .release
        .as_mut()
        .is_some_and(|task| check_ready(task).is_some())
    {
        runtime.release = None;
    }
    if runtime.release.is_some() {
        return;
    }
    if !garbage.0.is_empty() {
        let shapes = std::mem::take(&mut garbage.0);
        runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
            drop(shapes);
        }));
        return;
    } // 同域单飞释放槽，不为每根创建任务。
    let mut completed_root = None;
    if let Some(result) = runtime.task.as_mut().and_then(check_ready) {
        runtime.task = None;
        runtime.started = None;
        let valid = runtime.failed.get(&result.root) != Some(&result.generation)
            && roots.get(result.root).is_ok_and(
                |(_, request, instance, _, _, _, _, transform, _)| {
                    request.generation == result.generation
                        && instance.uses_async_mesh_collision()
                        && transform.to_matrix().inverse() == result.inverse_root
                },
            );
        if !valid {
            runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
                drop(result);
            }));
            return;
        }
        match result.shape {
            Ok((collider, physical)) => {
                completed_root = Some(result.root); // Ready命令尚未应用，本帧无需把刚完成的根再次排队。
                let update_active = roots.get(result.root).is_ok_and(|(_, _, instance, _, _, _, _, _, formal)| {
                    instance.is_active()
                        && (formal.is_none() || (!instance.is_terrain_patch() && instance.asset_target().has_scene_collision()))
                });
                // 完整模型热更无需地形拓扑接管，可在新形状准备完成后更新活动代理。
                if update_active && !deferred.contains(result.root) {
                    let global = *roots.get(result.root).expect("已复验当前根").7;
                    install_physics_proxy(&mut commands, result.root, global, physical.clone());
                }
                commands
                    .entity(result.root)
                    .remove::<TerrainPatchColliderFailure>()
                    .insert(TerrainPatchColliderReady {
                        collider,
                        physical,
                        scale: result.scale,
                        generation: result.generation,
                        meshes: result.meshes,
                        surface_ready: !fields.get(result.root).is_ok_and(|f| f.0.cell_size > 0.),
                    });
                commands.trigger(ColliderConstructorHierarchyReady {
                    entity: result.root,
                });
                if root_is_pending(result.root, &state, &pending_roots) {
                    status.advance_progress();
                }
            }
            Err(error) => {
                runtime.failed.insert(result.root, result.generation);
                commands
                    .entity(result.root)
                    .insert(TerrainPatchColliderFailure(error.clone()));
                reject(
                    root_is_pending(result.root, &state, &pending_roots),
                    &mut state,
                    &mut status,
                    format!("三维地形碰撞失败：{error}"),
                );
            }
        }
    }
    if runtime.task.is_some() {
        return;
    }
    if runtime.capture.as_ref().is_some_and(|capture| {
        runtime.failed.get(&capture.root) == Some(&capture.generation)
            || roots
                .get(capture.root)
                .map_or(true, |(_, request, _, _, _, _, _, transform, _)| {
                    request.generation != capture.generation
                        || transform.to_matrix().inverse() != capture.inverse_root
                })
    }) {
        runtime.started = None;
        let capture = runtime.capture.take();
        runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
            drop(capture);
        }));
        return;
    }
    if runtime.capture.is_none() {
        let runtime = &mut *runtime;
        for root in poll_roots(&runtime.registered, runtime.capture_cursor) {
            runtime.capture_cursor = Some(root);
            if completed_root == Some(root) {
                continue;
            }
            let Ok((root, request, instance, world, world_ready, ready, _, transform, _)) =
                roots.get(root)
            else {
                continue;
            };
            if !instance.uses_async_mesh_collision()
                || uploading.contains(root)
                || (world.is_some() && world_ready.is_none())
                || ready.is_some_and(|ready| {
                    ready.generation == request.generation && ready.scale == transform.scale()
                })
                || runtime.failed.get(&root) == Some(&request.generation)
            {
                continue;
            }
            let scene_collision = !instance.is_terrain_patch() && instance.asset_target().has_scene_collision();
            runtime.capture = Some(Capture {
                root,
                generation: request.generation,
                inverse_root: transform.to_matrix().inverse(),
                scale: transform.scale(),
                stack: VecDeque::from([root]),
                current: None,
                meshes: Vec::new(),
                sources: HashSet::new(),
                matrices: Vec::new(),
                mesh_entities: Vec::new(),
                nodes: 0,
                vertices: 0,
                triangles: 0,
                scene_collision,
                streamed: instance.terrain_patch().is_some_and(|p| p.streamed_volume),
                child_cursor: None,
            });
            // 普通GLB保留分帧进度和同代采用门，持续处理的大网格不受旧地形的累计秒数约束。
            runtime.started = (!scene_collision).then(|| (root, request.generation, Instant::now()));
            break;
        }
    }
    let Some(capture) = runtime.capture.as_mut() else {
        return;
    };
    let before = (
        capture.nodes,
        capture.child_cursor,
        capture
            .current
            .as_ref()
            .map(|mesh| mesh.vertices.len() + mesh.indices.len()),
        capture.meshes.len(),
    );
    let result = capture_step(capture, &meshes, &children, &visuals);
    let after = (
        capture.nodes,
        capture.child_cursor,
        capture
            .current
            .as_ref()
            .map(|mesh| mesh.vertices.len() + mesh.indices.len()),
        capture.meshes.len(),
    );
    if before != after && root_is_pending(capture.root, &state, &pending_roots) {
        status.advance_progress();
    }
    match result {
        Ok(false) => {}
        Ok(true) => {
            let mut capture = runtime.capture.take().expect("完整捕获已就绪");
            commands
                .entity(capture.root)
                .insert(TerrainMeshSources(std::mem::take(&mut capture.sources)));
            runtime.task = Some(AsyncComputeTaskPool::get().spawn(async move { cook(capture) }));
        }
        Err(error) => {
            let capture = runtime.capture.take().expect("错误捕获存在");
            runtime.failed.insert(capture.root, capture.generation);
            commands
                .entity(capture.root)
                .insert(TerrainPatchColliderFailure(error.clone()));
            reject(
                root_is_pending(capture.root, &state, &pending_roots),
                &mut state,
                &mut status,
                if capture.scene_collision {
                    format!("完整 GLB 碰撞捕获失败：{error}")
                } else {
                    format!("三维地形捕获失败：{error}")
                },
            );
            runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
                drop(capture);
            }));
        }
    }
}

fn root_is_pending(
    root: Entity,
    state: &ScenePlacementRuntimeState,
    pending_roots: &Query<&PlacementPendingRevision>,
) -> bool {
    let Some(staging) = &state.staging else { return false; };
    pending_roots.get(root).is_ok_and(|pending| {
        pending.generation == staging.generation && pending.revision == staging.revision
    })
}

fn invalidate_candidate(
    root: Entity,
    state: &mut ScenePlacementRuntimeState,
    commands: &mut Commands,
    pending_roots: &Query<&PlacementPendingRevision>,
    world_epoch: Option<u64>,
) {
    if !root_is_pending(root, state, pending_roots) {
        return;
    }
    let staging = state.staging.as_mut().expect("已核对当前候选根所属修订");
    staging.ready_roots.remove(&root);
    staging.eligibility_epoch = staging.eligibility_epoch.wrapping_add(1);
    commands
        .entity(root)
        .remove::<super::components::PlacementBuildingCollisionReady>();
    if let Some(epoch) = world_epoch {
        // 只有Mesh改动时World仍就绪，重新授予同代证据供下一份碰撞参加联合结算。
        commands.entity(root).insert(PlacementWorldInstanceReady(epoch));
    }
    state.asset_retry_requested = true;
}

fn reject(
    pending: bool,
    state: &mut ScenePlacementRuntimeState,
    status: &mut ScenePlacementRuntimeStatus,
    error: String,
) {
    if pending {
        super::systems::fail_revision(state, status, error);
    } else {
        warn!("{error}");
    }
}

/// 跨所有权回归只创建同代碰撞证明，不开放生产组件的换代字段。
#[cfg(test)]
pub(super) fn collision_fixture(
    generation: u64,
    collider: Collider,
    meshes: std::sync::Arc<[(Entity, Handle<Mesh>)]>,
) -> (TerrainPatchCollisionRequest, TerrainPatchColliderReady) {
    (
        TerrainPatchCollisionRequest { generation },
        TerrainPatchColliderReady {
            physical: collider.clone(),
            scale: Vec3::ONE,
            collider,
            generation,
            meshes,
            surface_ready: true,
        },
    )
}

#[cfg(test)]
#[path = "terrain_collision_tests.rs"]
mod tests;
