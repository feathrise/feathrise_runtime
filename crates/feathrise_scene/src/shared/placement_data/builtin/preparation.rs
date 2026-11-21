//! 内置作者网格的准备与原子安装。大几何的显示、包围盒与三角碰撞共用单飞后台槽。

use super::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};

// 延用旧作者容量内的固定小几何路径；超出该预算只在后台生成完整结果。
const INLINE_TRIANGLES: usize = 124;
const QUEUED_ROOTS: usize = 16_384;
const POLL_BUDGET: usize = 32;

pub(super) struct Request {
    pub(super) target: ScenePlacementAssetTarget,
    pub(super) geometry: BuiltinGeometryKey,
    pub(super) material: ([u8; 3], u16, bool),
    pub(super) collision: bool,
}

#[derive(Component, PartialEq, Eq)]
struct Ticket(u64);

#[derive(EntityEvent)]
/// 定向通知对应根已安装完整视觉/碰撞，或保留原节点并报告失败。
pub struct BuiltinVisualPrepared {
    pub entity: Entity,
    pub result: Result<(), String>,
}

struct Prepared {
    mesh: Mesh,
    bounds: Aabb,
    collider: Option<Collider>,
}

#[derive(Resource, Default)]
struct Preparation {
    next_ticket: u64,
    queue: VecDeque<Entity>,
    pending: HashMap<Entity, (Ticket, Request)>,
    running: Option<(Entity, Ticket, Request, Vec3, Task<Result<Prepared, String>>)>,
    retiring: Option<Task<()>>,
}

pub(in super::super) fn install_preparation(app: &mut App) {
    app.init_resource::<Preparation>().add_systems(Update, poll);
}

fn matches_request(world: &World, root: Entity, target: &ScenePlacementAssetTarget) -> bool {
    world.get::<ScenePlacementInstance>(root).is_some_and(|instance| {
        (instance.is_active() || instance.is_pending()) && &instance.asset_target() == target
    })
}

pub(super) fn request(world: &mut World, root: Entity, mut request: Request) {
    if !matches_request(world, root, &request.target) {
        return; // 旧命令不能触碰已经移除、退役或更换来源的根。
    }
    request.collision &= world.get::<super::super::motion::MotionPlatform>(root).is_none();
    world.init_resource::<Preparation>();
    let large = needs_worker(&request.geometry);
    if large && builtin_placement_visual_ready(world, root) {
        world.entity_mut(root).remove::<Ticket>();
        set_builtin_placement_collision_enabled_world(world, root, true);
        publish(world, root, Ok(()));
        return; // 同一几何/材质/碰撞已采用，位置或显隐变更不会再次准备源。
    }
    if !large {
        world.entity_mut(root).remove::<Ticket>();
        let scale = collider_scale(world, root);
        let prepared = prepare(&request.target, &request.geometry, request.collision, scale);
        finish(world, root, request, prepared);
        return;
    }
    let ticket = {
        let mut state = world.resource_mut::<Preparation>();
        if state.pending.len() >= QUEUED_ROOTS && !state.pending.contains_key(&root) {
            drop(state);
            publish(world, root, Err("内置网格准备队列已满".into()));
            return;
        }
        state.next_ticket = state.next_ticket.wrapping_add(1).max(1);
        let ticket = state.next_ticket;
        if state.pending.insert(root, (Ticket(ticket), request)).is_none() {
            state.queue.push_back(root);
        }
        ticket
    };
    world.entity_mut(root).insert(Ticket(ticket));
}

fn needs_worker(geometry: &BuiltinGeometryKey) -> bool {
    matches!(geometry, BuiltinGeometryKey::Mesh(geometry) if geometry.triangles().len() > INLINE_TRIANGLES)
}

fn collider_scale(world: &World, root: Entity) -> Vec3 {
    // Update尚未统一传播变换；从当前父链计算，避免以旧GlobalTransform烘焙后由Avian同步重建。
    let mut parents = [Mat4::IDENTITY; 32];
    let mut node = Some(root);
    let mut count = 0;
    while let Some(entity) = node {
        let Some(matrix) = parents.get_mut(count) else { return Vec3::ONE; };
        *matrix = world.get::<Transform>(entity).map_or(Mat4::IDENTITY, Transform::to_matrix);
        node = world.get::<ChildOf>(entity).map(ChildOf::parent);
        count += 1;
    }
    if count == 1 { return world.get::<Transform>(root).map_or(Vec3::ONE, |t| t.scale); }
    let world = parents[..count].iter().rev().copied().fold(Mat4::IDENTITY, |outer, local| outer * local);
    GlobalTransform::from(world).scale()
}

fn poll(world: &mut World) {
    // 玩家/编辑器稳定帧只检查单个资源，无需搜索在场网格或复制作者源。
    let ready = {
        let mut state = world.resource_mut::<Preparation>();
        if let Some(retiring) = state.retiring.as_mut() {
            if check_ready(retiring).is_none() { return; }
            state.retiring = None;
        }
        let result = state.running.as_mut().and_then(|(_, _, _, _, work)| check_ready(work));
        result.map(|result| (state.running.take().expect("完成结果拥有当前槽"), result))
    };
    if let Some(((root, ticket, job, scale, _), result)) = ready {
        if world.get::<Ticket>(root) == Some(&ticket) && matches_request(world, root, &job.target) {
            if job.collision && result.is_ok() && collider_scale(world, root) != scale {
                // 运行中缩放换代必须重新准备，绝不把大碰撞的缩放烘焙留到采用帧。
                request(world, root, job);
                retire(world, result);
                return;
            }
            world.entity_mut(root).remove::<Ticket>();
            finish(world, root, job, result);
        } else {
            // 旧结果和其大源在后台统一析构；完成前不给下一工作借用相同的内存信用。
            retire(world, (job, result));
            return;
        }
    }
    if world.resource::<Preparation>().running.is_some() { return; }
    // 工作槽仅消费一个活请求；删除/换图遗留的有限前缀分帧跳过。
    for _ in 0..POLL_BUDGET {
        let next = {
            let mut state = world.resource_mut::<Preparation>();
            state.queue.pop_front().and_then(|root| state.pending.remove(&root).map(|entry| (root, entry)))
        };
        let Some((root, (ticket, request))) = next else { break; };
        if world.get::<Ticket>(root) != Some(&ticket) || !matches_request(world, root, &request.target) {
            if AsyncComputeTaskPool::try_get().is_some() {
                retire(world, request);
                return;
            }
            continue;
        }
        let Some(pool) = AsyncComputeTaskPool::try_get() else {
            world.entity_mut(root).remove::<Ticket>();
            publish(world, root, Err("内置网格缺少后台计算池".into()));
            continue;
        };
        let (target, geometry, collision) = (request.target.clone(), request.geometry.clone(), request.collision);
        let scale = collider_scale(world, root);
        let task = pool.spawn(async move { prepare(&target, &geometry, collision, scale) });
        world.resource_mut::<Preparation>().running = Some((root, ticket, request, scale, task));
        break;
    }
}

fn retire(world: &mut World, value: impl Send + 'static) {
    let task = AsyncComputeTaskPool::get().spawn(async move { drop(value); });
    world.resource_mut::<Preparation>().retiring = Some(task);
}

fn prepare(
    target: &ScenePlacementAssetTarget, geometry: &BuiltinGeometryKey, collision: bool, scale: Vec3,
) -> Result<Prepared, String> {
    target.validate()?;
    let author = target.editable_geometry().ok_or("内置记录没有作者网格")?;
    let cube_uv = matches!(geometry, BuiltinGeometryKey::Cuboid(_)).then(|| parameter_cuboid_uv0(&author));
    let source_collider = if collision {
        let trimesh_error = |error| format!("内置网格三角碰撞准备失败：{error:?}");
        let mut collider = match geometry {
            BuiltinGeometryKey::Cuboid(size) => {
                let [x, y, z] = size.map(|mm| mm as f32 / 1_000.0);
                Collider::cuboid(x, y, z)
            }
            BuiltinGeometryKey::Editable(points) => Collider::try_trimesh(
                points.map(author_position).to_vec(),
                EDITABLE_CUBOID_TRIANGLES.map(|triangle| triangle.map(|index| index as u32)).to_vec(),
            ).map_err(trimesh_error)?,
            BuiltinGeometryKey::Mesh(geometry) => Collider::try_trimesh(
                geometry.vertices().iter().copied().map(author_position).collect(),
                geometry.triangles().iter().map(|triangle| triangle.map(|index| index as u32)).collect(),
            ).map_err(trimesh_error)?,
        };
        collider.set_scale(scale, 10);
        Some(collider)
    } else { None };
    let mut mesh = editable_mesh_mesh(author);
    if let Some(uv) = cube_uv { mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv); }
    let bounds = mesh.get_aabb().ok_or("内置网格没有有效包围盒")?;
    Ok(Prepared { mesh, bounds, collider: source_collider })
}

fn author_position(point: [i32; 3]) -> Vec3 {
    Vec3::from_array(point.map(|value| value as f32 * 0.0005))
}

fn finish(world: &mut World, root: Entity, request: Request, result: Result<Prepared, String>) {
    let Prepared {mesh, bounds, collider} = match result {
        Ok(prepared) => prepared,
        Err(error) => { publish(world, root, Err(error)); return; }
    };
    world.init_resource::<BuiltinPrimitiveCache>();
    // 只有完整结果到达才替换原来的可见子节点。旧画面和承托一直共同归旧子节点。
    let (mesh, material) = world.resource_scope(|world, mut cache: Mut<BuiltinPrimitiveCache>| {
        let mesh = {
            let mut meshes = world.resource_mut::<Assets<Mesh>>();
            if needs_worker(&request.geometry) {
                // 大网格由现有活动/候选根持有，不在小网格配方缓存中长期钉住共享作者拓扑。
                meshes.add(mesh)
            } else if let Some(handle) = cache.meshes.get(&request.geometry).and_then(|id| meshes.get_strong_handle(*id)) {
                handle
            } else {
                let handle = meshes.add(mesh);
                if !cache.meshes.contains_key(&request.geometry) {
                    if cache.mesh_order.len() >= BUILTIN_CACHE_ENTRIES {
                        if let Some(old) = cache.mesh_order.pop_front() { cache.meshes.remove(&old); }
                    }
                    cache.mesh_order.push_back(request.geometry.clone());
                }
                cache.meshes.insert(request.geometry.clone(), handle.id());
                handle
            }
        };
        let material = {
            let key = request.material;
            let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
            if let Some(handle) = cache.materials.get(&key).and_then(|id| materials.get_strong_handle(*id)) {
                handle
            } else {
                let ([r, g, b], roughness, open_surface) = key;
                let handle = materials.add(StandardMaterial {
                    base_color: Color::srgb_u8(r, g, b),
                    perceptual_roughness: roughness as f32 / 1_000.0,
                    double_sided: open_surface,
                    cull_mode: if open_surface { None } else { Some(bevy::render::render_resource::Face::Back) },
                    // 平面不封底；双面材质与闭壳分开缓存，避免同色立方体继承双面绘制。
                    ..default()
                });
                if !cache.materials.contains_key(&key) {
                    if cache.material_order.len() >= BUILTIN_CACHE_ENTRIES {
                        if let Some(old) = cache.material_order.pop_front() { cache.materials.remove(&old); }
                    }
                    cache.material_order.push_back(key);
                }
                cache.materials.insert(key, handle.id());
                handle
            }
        };
        (mesh, material)
    });
    if let Some(old) = builtin_visual_for_root(world, root) {
        world.despawn(old);
    }
    let hidden = world.get::<Visibility>(root) == Some(&Visibility::Hidden);
    let mut child = world.spawn((
        Name::new(if matches!(request.geometry, BuiltinGeometryKey::Cuboid(_)) { "内置立方体网格" } else { "可编辑网格" }),
        BuiltinPlacementVisual,
        BuiltinVisualSource(request.target),
        ScenePlacementPrimitiveVisual,
        Mesh3d(mesh),
        MeshMaterial3d(material),
        bounds,
        Transform::IDENTITY,
        Visibility::Inherited,
        ChildOf(root),
    ));
    if let Some(collider) = collider {
        child.insert(collider);
        if hidden { child.insert(ColliderDisabled); } // 结果准备期间发生的显隐操作优先。
    }
    let visual = child.id();
    world.entity_mut(root).insert(BuiltinPlacementVisualRoot(visual));
    publish(world, root, Ok(()));
}

fn publish(world: &mut World, root: Entity, result: Result<(), String>) {
    if let Err(error) = &result {
        warn!(?root, "{error}");
    }
    world.trigger(BuiltinVisualPrepared { entity: root, result });
}
