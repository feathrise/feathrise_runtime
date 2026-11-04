//! 独立体素的单飞状态机；观察窗口、玩家/近角色碰撞、字段和GPU证明共同采用。
use super::{build, resources::*, surface};
use crate::maps::dynamic_map::{
    instances::TerrainObjectInstance,
    resources::{DynamicMapRuntimePhase, DynamicMapRuntimeState, DynamicMapRuntimeStatus},
    sparse_runtime::DynamicClipmapAuthoringView,
    surface::DynamicSurfaceState,
};
use bevy::{
    asset::AssetEvent,
    camera::visibility::NoAutoAabb,
    ecs::system::SystemParam,
    prelude::*,
    render::storage::ShaderBuffer,
    tasks::{AsyncComputeTaskPool, IoTaskPool, futures::check_ready},
};
use feathrise_environment::terrain_surface::{
    TerrainSurfaceFallbackImages, TerrainSurfaceMaterial, TerrainVolumeSurface,
};
use feathrise_map_content::MapVoxelTerrainSource;
use feathrise_scene_core::residency::{InstanceFrame, InstanceRes, InstanceResMut, ResidentAssetEventReader};

#[derive(SystemParam)]
pub(in crate::maps::dynamic_map) struct AssetsInput<'w> {
    asset_root: Option<Res<'w, feathrise_map_content::RuntimeAssetRoot>>,
    sources: Res<'w, Assets<MapVoxelTerrainSource>>,
    server: Res<'w, AssetServer>,
    meshes: ResMut<'w, Assets<Mesh>>,
    standard: ResMut<'w, Assets<StandardMaterial>>,
    surface: ResMut<'w, Assets<TerrainSurfaceMaterial>>,
    buffers: ResMut<'w, Assets<ShaderBuffer>>,
    fallback: Res<'w, TerrainSurfaceFallbackImages>,
}
#[derive(SystemParam)]
pub(in crate::maps::dynamic_map) struct FocusInput<'w, 's> {
    cameras: Query<
        'w,
        's,
        (
            &'static Camera,
            &'static GlobalTransform,
            Option<&'static DynamicClipmapAuthoringView>,
        ),
        With<Camera3d>,
    >,
    players: Query<'w, 's, &'static GlobalTransform, With<crate::SceneRuntimeObserver>>,
    actors: Query<
        'w,
        's,
        (
            &'static GlobalTransform,
            &'static crate::SceneCollisionParticipant,
        ),
    >,
}

#[allow(clippy::too_many_arguments)]
pub(in crate::maps::dynamic_map) fn drive(
    mut commands: Commands,
    frame: InstanceFrame,
    definitions: Query<&TerrainObjectInstance>,
    mut runtime: InstanceResMut<VoxelObjectRuntime>,
    mut map: InstanceResMut<DynamicMapRuntimeState>,
    mut status: InstanceResMut<DynamicMapRuntimeStatus>,
    mut assets: AssetsInput,
    mut events: ResidentAssetEventReader<MapVoxelTerrainSource>,
    focus: FocusInput,
    controls: Res<crate::TerrainVolumeStreamControl>,
    surface_state: InstanceRes<DynamicSurfaceState>,
    children: Query<&Children>,
    gpu: (
        Option<ResMut<crate::WaterGpuUploadQueue>>,
        Option<Res<crate::WaterGpuUploadReceipts>>,
        Res<VoxelWorkBudget>,
    ),
) {
    let (mut uploads, receipts, budget) = gpu;
    if let Some(task) = runtime.release.as_mut()
        && check_ready(task).is_some()
    {
        runtime.release = None;
    }
    if runtime.release.is_none() && !runtime.garbage.is_empty() {
        let garbage = std::mem::take(&mut runtime.garbage);
        // 上一帧ECS替换与ground发布已经完成，Arc/BVH的最后释放留在唯一后台回收槽。
        runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
            release_garbage(garbage);
        }));
    }
    retire_entities(&mut commands, &mut runtime, &children); // 等IO、上传或新源时同样归还旧根。
    let Some(instance) = frame.instance() else {
        return;
    };
    let Ok(definition) = definitions.get(instance) else {
        return;
    };
    let Some(descriptor) = definition.definition.voxel.as_ref() else {
        return;
    };
    let Some(root) = map.root else {
        return;
    };
    if runtime.source_path.as_deref() != Some(&descriptor.source_path) {
        runtime.source = Some(assets.server.load(descriptor.source_path.clone()));
        runtime.source_path = Some(descriptor.source_path.clone());
        runtime.revision = runtime.revision.wrapping_add(1);
    }
    let source_handle = runtime.source.as_ref().unwrap().clone();
    for event in events.read() {
        if matches!(event,AssetEvent::Modified {id}|AssetEvent::Removed {id} if *id==source_handle.id())
        {
            runtime.revision = runtime.revision.wrapping_add(1);
        }
    }
    let Some(source) = assets.sources.get(&source_handle) else {
        if matches!(
            assets.server.load_state(source_handle.id()),
            bevy::asset::LoadState::Failed(_)
        ) {
            status.phase = DynamicMapRuntimePhase::Failed;
            status.detail = "独立体素作者源加载失败，保留上一完整地形".into();
        }
        return;
    };
    if map.active_map_id.as_ref() != Some(&source.map_id)
        || source.terrain_id != definition.definition.id
    {
        status.phase = DynamicMapRuntimePhase::Failed;
        status.detail = "独立体素源身份无效".into();
        return;
    }
    if runtime.source_proxy.is_none() {
        runtime.source_proxy = Some(surface::spawn_proxy(
            &mut commands,
            root,
            &mut assets.standard,
            &mut assets.meshes,
        ));
    }
    let view = controls
        .focus_override
        .or_else(|| {
            focus
                .cameras
                .iter()
                .filter(|(camera, _, _)| camera.is_active)
                .find_map(|(_, _, author)| {
                    author.map(|author| {
                        Vec3::new(author.focus_xz.x, author.focus_y, author.focus_xz.y)
                    })
                })
        })
        .or_else(|| {
            focus
                .cameras
                .iter()
                .find(|(camera, _, _)| camera.is_active)
                .map(|(_, pose, _)| pose.translation())
        })
        .unwrap_or(Vec3::ZERO);
    let player = focus
        .players
        .iter()
        .next()
        .map(GlobalTransform::translation)
        .or(controls.physics_focus)
        .unwrap_or(view);
    let scale = frame
        .frame()
        .source_to_world
        .to_scale_rotation_translation()
        .0;
    let anchor_extent = (source.cell_size_meters * 32.0).max(4.0 / scale.min_element());
    let key = |world: Vec3| {
        (frame.to_local(world) / anchor_extent)
            .floor()
            .as_ivec3()
            .to_array()
    };
    let player_center = key(player);
    let mut actor_centers = std::collections::BTreeSet::new();
    let role_radius = definition
        .definition
        .terrain
        .role_collision_preload_radius_meters
        .unwrap_or(120.0);
    for (pose, actor) in &focus.actors {
        if actor.source_map.as_ref() == source.map_id.as_str()
            && pose.translation().distance_squared(player) <= role_radius * role_radius
        {
            actor_centers.insert(key(pose.translation()));
        }
    }
    actor_centers.remove(&player_center);
    let physics_centers = std::iter::once(player_center)
        .chain(actor_centers)
        .collect();
    let mut desired = Identity {
        root,
        source: source_handle.id(),
        revision: runtime.revision,
        descriptor: descriptor.clone(),
        display_center: key(view),
        physics_centers,
        player_center,
        anchor_extent,
        scale,
    };
    if let Some(previous) = runtime
        .started
        .as_ref()
        .or_else(|| {
            runtime
                .candidate
                .as_ref()
                .map(|candidate| &candidate.built.identity)
        })
        .or(runtime.applied.as_ref())
    {
        coalesce_foci(&mut desired, previous);
    }
    runtime.desired = Some(desired.clone());
    if let Some(active) = runtime.active_surface.as_mut() {
        surface::refresh(
            active,
            &surface_state,
            frame.frame().source_to_world,
            &mut assets.surface,
        );
    }
    if runtime.release.is_none()
        && runtime.task.as_ref().is_some_and(|task| task.is_finished())
        && let Some(queue) = uploads.as_mut()
    {
        // 字段存储表同样占用共享登记席位；不能在多个mesh限流之外一次上传多个大表。
        // worker保证字段表不超过这份16MiB信用，实际Mesh留到下一帧登记。
        if !queue
            .try_reserve_terrain_registration(
                crate::shared::large_map::MAX_TERRAIN_GPU_UPLOAD_BYTES_PER_FRAME,
            )
            .unwrap_or(false)
        {
            return;
        }
    }
    if runtime.release.is_none()
        && let Some(result) = runtime.task.as_mut().and_then(check_ready)
    {
        runtime.task = None;
        let started = runtime.started.take();
        match result {
            Ok(built) if built.identity == desired => {
                let valid = built
                    .fields
                    .validate_storage_limit(controls.gpu_storage_limit)
                    .and_then(|_| {
                        surface::create(
                            &built.fields,
                            &surface_state,
                            frame.frame().source_to_world,
                            built.index.source.max[1],
                            &assets.fallback,
                            &mut assets.surface,
                            &mut assets.buffers,
                        )
                    });
                match valid {
                    Ok(surface) => {
                        let candidate = commands
                            .spawn((
                                Name::new("体素完整隐藏候选"),
                                Transform::IDENTITY,
                                Visibility::Hidden,
                                built.fields.clone(),
                                ChildOf(root),
                            ))
                            .id();
                        let previous = runtime.index.replace((
                            desired.source,
                            desired.revision,
                            built.index.clone(),
                        ));
                        if let Some(previous) = previous {
                            runtime.garbage.push(Garbage::Index(previous));
                        }
                        runtime.candidate = Some(Candidate {
                            root: candidate,
                            built,
                            surface,
                            mesh_ids: Vec::new(),
                            warmup: None,
                            ticket: None,
                            started_at: std::time::Instant::now(),
                        });
                    }
                    Err(error) => {
                        runtime.failed = Some(desired.clone());
                        status.phase = DynamicMapRuntimePhase::Failed;
                        status.detail = error;
                        runtime.garbage.push(Garbage::Built(built));
                    }
                }
            }
            Ok(stale) => {
                runtime.garbage.push(Garbage::Built(stale));
            }
            Err(error) if started.as_ref() == Some(&desired) => {
                runtime.failed = started;
                status.phase = DynamicMapRuntimePhase::Failed;
                status.detail = error;
            }
            Err(_) => {} // 旧失败不能否定已修正的新参数。
        }
    }
    if runtime
        .candidate
        .as_ref()
        .is_some_and(|candidate| candidate.built.identity != desired)
    {
        discard_candidate(&mut commands, &mut runtime, uploads.as_deref_mut());
    }
    if let Some(candidate) = runtime.candidate.as_mut() {
        if surface::refresh(
            &mut candidate.surface,
            &surface_state,
            frame.frame().source_to_world,
            &mut assets.surface,
        ) && let Some(ticket) = candidate.ticket.take()
            && let Some(queue) = uploads.as_mut()
        {
            queue.retire(ticket);
        }
        if let Some((mesh, _)) = candidate.built.meshes.front() {
            let bytes = mesh.count_vertices() * 64 + mesh.indices().map_or(0, |v| v.len() * 4);
            match uploads.as_mut().map_or(Ok(true), |queue| {
                queue.try_reserve_terrain_registration(bytes)
            }) {
                Ok(true) => {
                    let (mesh, aabb) = candidate.built.meshes.pop_front().unwrap();
                    let handle = assets.meshes.add(mesh);
                    candidate.mesh_ids.push(handle.id());
                    if candidate.warmup.is_none() {
                        candidate.warmup = Some(
                            commands
                                .spawn((
                                    Name::new("独立体素GPU预热"),
                                    feathrise_scene_core::residency::MapInstanceDetached,
                                    feathrise_scene_core::residency::MapInstanceOwner(instance),
                                    Mesh3d(handle.clone()),
                                    MeshMaterial3d(candidate.surface.material.clone()),
                                    Transform::from_xyz(0., -100_000., 0.)
                                        .with_scale(Vec3::splat(0.000_001)),
                                    Visibility::Inherited,
                                    crate::DynamicMapTerrainPrimitive,
                                    TerrainVolumeSurface,
                                    bevy::camera::visibility::NoFrustumCulling,
                                ))
                                .id(),
                        );
                    }
                    let _visible_chunk = commands.spawn((
                        Name::new("独立体素地形块"),
                        Mesh3d(handle),
                        MeshMaterial3d(candidate.surface.material.clone()),
                        Transform::IDENTITY,
                        Visibility::Inherited,
                        NoAutoAabb,
                        aabb,
                        crate::DynamicMapTerrainPrimitive,
                        TerrainVolumeSurface,
                        ChildOf(candidate.root),
                    )).id();
                    // 离屏预热不属于可见作者网格；发布构建也不能引用仅F12存在的标记类型。
                    #[cfg(any(feature = "test_panel", feature = "author_view"))]
                    commands.entity(_visible_chunk).insert(crate::TerrainGridTarget);
                }
                Ok(false) => return,
                Err(error) => {
                    runtime.failed = Some(desired.clone());
                    status.phase = DynamicMapRuntimePhase::Failed;
                    status.detail = error;
                    discard_candidate(&mut commands, &mut runtime, uploads.as_deref_mut());
                    return;
                }
            }
        }
        if candidate.built.meshes.is_empty() {
            if candidate.ticket.is_none()
                && let Some(warmup) = candidate.warmup
                && let Some(queue) = uploads.as_mut()
            {
                candidate.ticket = Some(queue.submit_assets(
                    candidate.mesh_ids.clone(),
                    vec![candidate.surface.material.id().untyped()],
                    vec![warmup],
                ));
                candidate.started_at = std::time::Instant::now();
            }
            if let Some(ticket) = candidate.ticket
                && let Some(receipts) = receipts.as_ref()
            {
                if receipts.has_failed(ticket) {
                    runtime.failed = Some(desired.clone());
                    status.phase = DynamicMapRuntimePhase::Failed;
                    status.detail = "体素GPU候选准备失败，保留旧完整地形".into();
                    discard_candidate(&mut commands, &mut runtime, uploads.as_deref_mut());
                    return;
                }
                if !receipts.is_ready(ticket) {
                    return;
                }
            }
            let candidate = runtime.candidate.take().unwrap();
            if let Some(ticket) = candidate.ticket
                && let Some(queue) = uploads.as_mut()
            {
                queue.retire(ticket);
            }
            if let Some(warmup) = candidate.warmup {
                commands.entity(warmup).try_despawn();
            }
            if let Some(previous) = runtime.active.replace(candidate.root) {
                commands
                    .entity(previous)
                    .try_insert(Visibility::Hidden)
                    .remove::<avian3d::prelude::Collider>();
                let fields = runtime
                    .active_fields
                    .take()
                    .expect("已采用体素持有字段回收租约");
                let collider = runtime.active_collider.take();
                runtime.retired.push_back(RetiredRoot {
                    root: previous,
                    fields,
                    collider,
                });
            }
            commands
                .entity(candidate.root)
                .insert(Visibility::Inherited);
            runtime.active_collider = candidate.built.collider.clone();
            runtime.active_fields = Some(candidate.built.fields.clone());
            if let Some(collider) = candidate.built.collider {
                commands.entity(candidate.root).insert(collider);
            }
            let previous = Garbage::Applied(
                runtime.display_query.take(),
                runtime.active_index.take(),
                runtime.active_surface.take(),
            );
            runtime.garbage.push(previous);
            runtime.display_query = candidate.built.display_query;
            runtime.active_surface = Some(candidate.surface);
            runtime.active_index = Some(candidate.built.index);
            runtime.generation = runtime.generation.wrapping_add(1);
            runtime.applied = Some(candidate.built.identity);
            map.collision_ready = true;
            status.phase = DynamicMapRuntimePhase::Ready;
            status.detail = if candidate.built.limited {
                "独立体素附近窗口已采用；几何或字段远区等待共享工作集换入，作者对象完整保留"
            } else {
                "独立体素显示窗口、字段与近场真实碰撞已共同采用"
            }
            .into();
        }
    }
    if runtime.applied.as_ref() != Some(&desired)
        && runtime.task.is_none()
        && runtime.candidate.is_none()
        && runtime.release.is_none()
        && runtime.garbage.is_empty()
        && runtime.failed.as_ref() != Some(&desired)
    {
        let Some(lease) = budget.acquire() else {
            status.detail = "等待共享体素计算席位".into();
            return;
        };
        let source = source.clone();
        let cached = runtime
            .index
            .as_ref()
            .filter(|(source, revision, _)| {
                *source == desired.source && *revision == desired.revision
            })
            .map(|(_, _, index)| index.clone());
        runtime.started = Some(desired.clone());
        let asset_root = assets.asset_root.as_deref().cloned().unwrap_or_default(); // 原一体未注入时沿用SDK目录。
        runtime.task =
            Some(IoTaskPool::get().spawn(build::prepare(asset_root, source, cached, desired, lease)));
        status.detail = "正在后台流送独立体素显示、字段和碰撞窗口".into();
    }
}

fn coalesce_foci(desired: &mut Identity, previous: &Identity) {
    if desired.root != previous.root
        || desired.source != previous.source
        || desired.revision != previous.revision
        || desired.descriptor != previous.descriptor
        || desired.scale != previous.scale
        || desired.anchor_extent != previous.anchor_extent
    {
        return;
    }
    let distance = |a: [i32; 3], b: [i32; 3]| {
        ((Vec3::from_array(a.map(|v| v as f32)) - Vec3::from_array(b.map(|v| v as f32)))
            * desired.anchor_extent
            * desired.scale)
            .length()
    };
    let display_guard = desired
        .descriptor
        .display_lods
        .first()
        .map_or(4., |row| row.max_distance_meters * 0.5);
    if distance(desired.display_center, previous.display_center) <= display_guard {
        desired.display_center = previous.display_center;
    }
    let physics_guard = desired
        .descriptor
        .collision_lods
        .first()
        .map_or(4., |row| row.max_distance_meters * 0.5);
    if distance(desired.player_center, previous.player_center) <= physics_guard {
        desired.player_center = previous.player_center;
    }
    if desired.physics_centers.iter().all(|center| {
        previous
            .physics_centers
            .iter()
            .any(|old| distance(*center, *old) <= physics_guard)
    }) {
        desired
            .physics_centers
            .clone_from(&previous.physics_centers);
    }
}
fn discard_candidate(
    commands: &mut Commands,
    runtime: &mut VoxelObjectRuntime,
    uploads: Option<&mut crate::WaterGpuUploadQueue>,
) {
    let Some(stale) = runtime.candidate.take() else {
        return;
    };
    runtime.retired.push_back(RetiredRoot {
        root: stale.root,
        fields: stale.built.fields.clone(),
        collider: None,
    });
    if let Some(ticket) = stale.ticket
        && let Some(queue) = uploads
    {
        queue.retire(ticket);
    }
    if let Some(warmup) = stale.warmup {
        commands.entity(warmup).try_despawn();
    }
    runtime.garbage.push(Garbage::Candidate(stale));
}
fn retire_entities(
    commands: &mut Commands,
    runtime: &mut VoxelObjectRuntime,
    children: &Query<&Children>,
) {
    if let Some(retired) = runtime.retired.front() {
        let root = retired.root;
        if let Ok(nodes) = children.get(root)
            && !nodes.is_empty()
        {
            for entity in nodes.iter().rev().take(128) {
                commands.entity(entity).try_despawn();
            }
        } else {
            commands.entity(root).try_despawn();
            let retired = runtime.retired.pop_front().unwrap();
            runtime.garbage.push(Garbage::Root(retired));
        }
    }
}

fn release_garbage(garbage: Vec<Garbage>) {
    for value in garbage {
        match value {
            Garbage::Candidate(value) => drop(value),
            Garbage::Built(value) => drop(value),
            Garbage::Index(value) => drop(value),
            Garbage::Applied(query, index, surface) => {
                drop(query);
                drop(index);
                drop(surface);
            }
            Garbage::Root(RetiredRoot {
                root: _,
                fields,
                collider,
            }) => {
                drop(fields);
                drop(collider);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn camera_and_player_motion_coalesce_only_within_the_existing_window() {
        let mut identity = Identity {
            root: Entity::PLACEHOLDER,
            source: Handle::<MapVoxelTerrainSource>::default().id(),
            revision: 1,
            descriptor: feathrise_map_content::MapVoxelTerrainDescriptor {
                cell_size_meters: 0.25,
                min: [-100., -2., -100.],
                max: [100., 0., 100.],
                source_path: "source".into(),
                display_lods: feathrise_map_content::default_collision_lods(),
                collision_lods: feathrise_map_content::default_collision_lods(),
            },
            display_center: [0; 3],
            physics_centers: vec![[0; 3]],
            player_center: [0; 3],
            anchor_extent: 8.,
            scale: Vec3::ONE,
        };
        let previous = identity.clone();
        identity.display_center = [1, 0, 0];
        identity.physics_centers = vec![[1, 0, 0]];
        coalesce_foci(&mut identity, &previous);
        assert_eq!(identity, previous);
        identity.display_center = [3, 0, 0];
        identity.physics_centers = vec![[3, 0, 0]];
        coalesce_foci(&mut identity, &previous);
        assert_ne!(identity, previous);
    }
}
