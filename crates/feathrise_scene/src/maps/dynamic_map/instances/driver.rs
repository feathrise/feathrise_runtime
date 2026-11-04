//! 固定两个准备席位驱动完整实例；保留旧完整批次，候选六域就绪后原子显示与启用碰撞。

use avian3d::prelude::{RigidBody, RigidBodyDisabled};
use bevy_tnua::TnuaNotPlatform; // Tnua按ColliderOf读取批次刚体标记，空间树仍可见的隐藏候选也不能承托角色。
use bevy::{prelude::*, tasks::{AsyncComputeTaskPool, futures_lite::future}};
use feathrise_map_content::{ActiveDynamicMap, ExpandedMapInstance, MapInstanceGraph,
    MapInstanceGraphPhase, MapInstanceRootRequest};
use feathrise_scene_core::residency::{initialize_instance_context, retire_instance_context,
    InstanceAssetJournalEpoch, InstanceResourceTable, MapInstanceOwner, MapRuntimeScope, MapSourceFrame};
use feathrise_scene_core::state::ScenePhase;
use std::{collections::BTreeMap, sync::Arc};
use super::{components::*, planning, readiness, resources::*, schedules};

const PREPARATION_SLOTS: usize = 2;
const CONTEXT_BUDGET: usize = 2;
const CLEANUP_BUDGET: usize = 128;

#[cfg(test)]
mod tests;

pub(super) fn publish_startup(scene: Res<State<ScenePhase>>, status: Res<SubMapRuntimeStatus>,
    gate: Option<ResMut<feathrise_scene_core::state::SceneStartupGate>>, progress: Option<ResMut<feathrise_scene_core::state::LoadingProgress>>) {
    let Some(mut gate) = gate else { return; };
    if gate.ready || gate.target != *scene.get() { return; }
    gate.sub_maps_ready = status.ready;
    gate.sub_maps_error.clone_from(&status.last_error);
    let Some(mut progress) = progress else { return; };
    if let Some(error) = gate.sub_maps_error.as_deref() {
        progress.stage = feathrise_scene_core::state::LoadingStage::Failed; progress.completed = 0; progress.total = 1;
        progress.label = format!("地图内容加载失败：{error}");
    } else if !status.ready && progress.stage == feathrise_scene_core::state::LoadingStage::SceneInit {
        // 只发布真实完成的网格/子地图数量；后续碰撞、角色阶段及其他域的失败不能被覆盖。
        progress.completed = status.prepared;
        progress.total = status.total.max(1);
        progress.label = format!("网格与子地图 {}/{} · {}", status.prepared, status.total,
            status.loading_detail.as_deref().unwrap_or("正在解析地图内容"));
    }
}


fn fall_key(value: f32) -> u32 {
    let bits = value.to_bits(); if bits & 0x8000_0000 == 0 { bits ^ 0x8000_0000 } else { !bits }
}
fn fall_value(key: u32) -> f32 {
    f32::from_bits(if key & 0x8000_0000 == 0 { !key } else { key ^ 0x8000_0000 })
}
fn refresh_fall(world: &World, context: &mut Context, node: &ExpandedMapInstance,
    thresholds: &mut std::collections::BTreeSet<(u32, Entity)>) {
    let threshold = readiness::get::<feathrise_scene_core::state::SceneFallBoundary>(world, context.entity)
        .lower_y_for_scene(ScenePhase::Active).map(|value| node.world_pose().transform_point(Vec3::new(0.0, value, 0.0)).y);
    if context.fall_death_y == threshold { return; }
    if let Some(old) = context.fall_death_y { thresholds.remove(&(fall_key(old), context.entity)); }
    if let Some(value) = threshold.filter(|value| value.is_finite()) { thresholds.insert((fall_key(value), context.entity)); }
    context.fall_death_y = threshold;
}
fn publish_fall(world: &mut World, runtime: &SubMapRuntime) {
    let Some(map_id) = &runtime.root_map_id else { return; };
    let Some(handles) = world.resource::<feathrise_map_content::MapContentRegistry>().handles(map_id) else { return; };
    let Some(manifest) = world.resource::<Assets<feathrise_map_content::MapManifestAsset>>().get(&handles.manifest) else { return; };
    let scene = *world.resource::<State<ScenePhase>>().get();
    if scene != ScenePhase::Active && runtime.active.as_ref().is_none_or(|batch| batch.contexts.is_empty()) {
        world.resource_mut::<feathrise_scene_core::state::SceneFallBoundary>().clear_for_scene(scene); return;
    }
    let minimum = world.get_resource::<super::super::clipmap::SparseHeightPageRuntime>()
        .and_then(|pages| pages.adopted_height_bounds(map_id))
        .map_or(manifest.terrain.base_height, |bounds| bounds[0]);
    let mut threshold = (f64::from(minimum) - 16.0).max(-f64::from(f32::MAX)) as f32;
    // 根Clipmap也按已采用浮点高程；不能每帧用旧base覆盖完整页发布的深谷死亡界。
    if let Some((key, _)) = runtime.active.as_ref().and_then(|batch| batch.fall_thresholds.first()) {
        threshold = threshold.min(fall_value(*key));
    }
    world.resource_mut::<feathrise_scene_core::state::SceneFallBoundary>().set_for_scene(scene, threshold);
}


pub(super) fn publish_root(scene: Res<State<ScenePhase>>, active: Res<ActiveDynamicMap>,
    mut request: ResMut<MapInstanceRootRequest>) {
    let map_id = match scene.get() { ScenePhase::Active => active.map_id.clone(),
        _ => None };
    if request.map_id != map_id { request.map_id = map_id; }
}

fn table_mut<T: Resource>(world: &mut World, entity: Entity) -> Mut<'_, T> {
    world.resource_mut::<InstanceResourceTable<T>>().map_unchanged(|table| table.values.get_mut(&entity).unwrap())
}

/// 摆放修订必须交由完整外层批次切换，不能在活动SubMapBatch内提前安装候选盒碰撞。
/// 【功能设计】Static会按现有物理合同归并到外层刚体，因此内层候选门不能替代外层门。
/// 每帧最多读取128个小通知，任意数量的同帧修改只推进一次epoch；旧完整批次保留到新批次全部Ready。
pub(super) fn invalidate_submaps_for_placement_changes(
    mut events: MessageReader<AssetEvent<feathrise_map_content::MapPlacementsAsset>>,
    mut rebuilds: MessageReader<crate::MapPlacementRebuildRequest>,
    buildings: Option<Res<feathrise_map_content::BuildingPresetRegistry>>,
    author_preview: Option<Res<feathrise_map_content::MapAuthorPreview>>,
    mut previous: Local<PlacementBatchInputs>,
    mut epoch: ResMut<InstanceAssetJournalEpoch>,
) {
    let mut changed = false;
    for event in events.read().take(128) {
        changed |= matches!(event, AssetEvent::Modified { .. } | AssetEvent::Removed { .. });
    }
    if !events.is_empty() { events.clear(); changed = true; } // 通知溢出也完整重建，不扫描无界消息尾部。
    for _ in rebuilds.read().take(128) { changed = true; }
    if !rebuilds.is_empty() { rebuilds.clear(); changed = true; }
    let revision = buildings.as_deref().map(|registry| registry.revision);
    let author_document = author_preview.as_deref().and_then(|preview| preview.active.as_ref())
        .map(|input| (input.map_id.clone(), input.placements.id()));
    changed |= previous.initialized && (previous.building_revision != revision || previous.author_document != author_document);
    previous.building_revision = revision;
    previous.author_document = author_document;
    previous.initialized = true;
    if changed { epoch.0 = epoch.0.wrapping_add(1); }
}

#[derive(Default)]
pub(super) struct PlacementBatchInputs {
    initialized: bool,
    building_revision: Option<u64>,
    author_document: Option<(feathrise_map_content::MapId, bevy::asset::AssetId<feathrise_map_content::MapPlacementsAsset>)>,
}

fn begin_batch(world: &mut World, runtime: &mut SubMapRuntime, revision: u64,
    graph: Arc<[ExpandedMapInstance]>) -> Batch {
    runtime.generation = runtime.generation.wrapping_add(1);
    let scene = *world.resource::<State<ScenePhase>>().get();
    let root = world.spawn((Name::new("子地图完整隐藏候选"), SubMapBatch,
        Transform::IDENTITY, Visibility::Hidden, RigidBody::Static, RigidBodyDisabled, TnuaNotPlatform,
        DespawnOnExit(scene))).id();
    Batch { root, graph, revision, next: 1, contexts: Vec::new(), index: BTreeMap::new(),
        leases: Vec::new(), cursor: 0, prepared: 0, fall_thresholds: Default::default(), cleanup: Vec::new(), retirement_cursor: 0, cleanup_started: false }
}

fn hide(world: &mut World, batch: &mut Batch) {
    if let Ok(mut root) = world.get_entity_mut(batch.root) {
        root.insert((Visibility::Hidden, RigidBodyDisabled, TnuaNotPlatform)).remove::<SubMapActive>();
    }
}

/// epoch在候选构造期间也可能换代；已Ready的旧上下文不能混入新修订后整体发布。
fn retire_stale_candidate(world: &mut World, runtime: &mut SubMapRuntime, revision: u64, epoch: u64) {
    if runtime.candidate.as_ref().is_some_and(|batch| batch.revision != revision || runtime.journal_epoch != epoch)
        && runtime.retired.is_empty()
    {
        let mut batch = runtime.candidate.take().unwrap();
        hide(world, &mut batch);
        runtime.retired.push_back(batch);
    }
}

fn create_contexts(world: &mut World, generation: u64, batch: &mut Batch, previous: Option<&Batch>) {
    for _ in 0..CONTEXT_BUDGET {
        let Some(node) = batch.graph.get(batch.next) else { break; };
        let entity = world.spawn_empty().id();
        let parent = if node.path.terrain_id.is_some() && !node.path.instances.is_empty() {
            let mut path = node.path.clone(); path.terrain_id = None;
            batch.index[&path]
        } else if node.path.instances.len() > 1 {
            let mut path = node.path.clone(); path.instances.pop();
            batch.index[&path]
        } else { batch.root };
        world.entity_mut(entity).insert((Name::new(format!("子地图 {}", node.label)),
            SubMapInstance { path: node.path.clone(), source_map_id: node.source_map_id.clone(),
                generation, batch: batch.root }, MapInstanceOwner(entity),
            node.local_pose(), MapSourceFrame::from_transform(node.world_pose()),
            Visibility::Inherited, ChildOf(parent))); // 连续镜头跟随在Transform传播前读取当前完整源框架。
        initialize_instance_context(world, entity);
        if node.terrain.is_some() {
            let map_instance = if node.path.instances.is_empty() { None } else { Some(parent) };
            world.entity_mut(entity).insert(feathrise_scene_core::residency::MapTerrainOwner { map_instance });
        }
        let previous_entity = previous.and_then(|active| active.index.get(&node.path)).copied();
        super::terrain_sources::initialize(world, entity, node, previous_entity);
        // 子调度按自己的批次解析关联姿态；不能依赖暂时移出World的协调器或借用旧批次。
        super::terrain_sources::register_lookup(world, batch.root, &node.path, entity);
        table_mut::<ActiveDynamicMap>(world, entity).select(node.source_map_id.clone());
        *table_mut::<State<ScenePhase>>(world, entity) = State::new(ScenePhase::Active);
        table_mut::<super::super::resources::DynamicMapRuntimeState>(world, entity).dirty = true;
        batch.index.insert(node.path.clone(), entity);
        batch.contexts.push(Context { entity, path: node.path.clone(), schedule: if node.terrain.is_some() { schedules::advance_terrain() } else { schedules::advance(world.get_resource::<crate::SceneApplicationHooks>().copied()) },
            follow: schedules::follow(), initialized: false, ready: false, failed: false, fall_death_y: None });
        batch.next += 1;
    }
    if batch.next == batch.graph.len() {
        super::terrain_sources::seal_lookup(world, batch.root);
    }
}

fn scope(world: &mut World, batch: &Batch, index: usize) {
    let node = &batch.graph[index + 1];
    let preview = node.path.terrain_id.as_ref().filter(|_| node.path.instances.is_empty())
        .and_then(|id| world.get_resource::<super::TerrainObjectTransformPreview>()?.for_object(&node.source_map_id, id));
    *world.resource_mut::<MapRuntimeScope>() = MapRuntimeScope {
        instance: Some(batch.contexts[index].entity),
        frame: MapSourceFrame::from_transform(preview.map_or_else(|| node.world_pose(), |pose| pose.as_transform())),
    };
}

fn advance(world: &mut World, batch: &mut Batch, available: usize, status: &mut SubMapRuntimeStatus) {
    // 老活动批次和新候选合计最多两个席位。在途任务未结清时不轮换到第三个实例。
    while batch.leases.len() < available && !batch.contexts.is_empty() {
        let mut selected = None;
        for _ in 0..1 { // 后台释放席位只有一个，不能覆盖尚未结束的任务。
            let index = batch.cursor % batch.contexts.len(); batch.cursor = batch.cursor.wrapping_add(1);
            if !batch.leases.contains(&index) { selected = Some(index); break; }
        }
        let Some(index) = selected else { break; };
        batch.leases.push(index);
    }
    for slot in 0..batch.leases.len() {
        let index = batch.leases[slot];
        scope(world, batch, index);
        let context = &mut batch.contexts[index];
        if !context.initialized { schedules::initialize(world); context.initialized = true; }
        readiness::refresh_surface_gpu_evidence(world, context.entity);
        context.schedule.run(world);
        refresh_fall(world, context, &batch.graph[index + 1], &mut batch.fall_thresholds);
        let terrain_only = batch.graph[index + 1].terrain.is_some();
        if !terrain_only { super::characters::publish(world, context.entity, &batch.graph[index + 1]); }
        match if terrain_only { readiness::check_terrain(world, context.entity) } else { readiness::check(world, context.entity) } {
            Ok(ready) => {
                context.failed = false;
                if !ready { status.loading_detail = Some(format!("{}：{}", batch.graph[index + 1].label,
                    readiness::waiting_detail(world, context.entity))); }
                if ready != context.ready {
                    if ready { batch.prepared += 1; } else { batch.prepared = batch.prepared.saturating_sub(1); }
                    context.ready = ready;
                }
            }
            Err(error) => { if context.ready { batch.prepared = batch.prepared.saturating_sub(1); context.ready = false; }
                context.failed = true; status.last_error = Some(format!("{}（{}）：{error}", batch.graph[index + 1].label, context.path)); }
        }
    }
    batch.leases.retain(|&index| {
        let context = &batch.contexts[index];
        !((context.ready || context.failed) && !readiness::busy(world, context.entity))
    });
    *world.resource_mut::<MapRuntimeScope>() = MapRuntimeScope::default();
}

/// 场景退出和热更新可能已经递归移除同一根；过期/重叠登记仍消耗预算但不得重复销毁。
pub(super) fn cleanup_entities(world: &mut World, cleanup: &mut Vec<Entity>, budget: usize) {
    for _ in 0..budget {
        let Some(&entity) = cleanup.last() else { break; };
        if world.get_entity(entity).is_err() { cleanup.pop(); continue; }
        let child = world.get::<Children>(entity).and_then(|children| children.first()).copied();
        if let Some(child) = child { cleanup.push(child); }
        else { cleanup.pop(); world.entity_mut(entity).despawn(); }
    }
}

fn retire(world: &mut World, runtime: &mut SubMapRuntime) {
    if let Some(task) = runtime.releasing.as_mut() {
        if future::block_on(future::poll_once(task)).is_none() { return; }
        runtime.releasing = None;
    }
    let Some(batch) = runtime.retired.front_mut() else { return; };
    // 资源表必须活到最后一个所属实体退出，迟到World/Collider回执才有可用上下文。
    if !batch.cleanup_started {
        for _ in 0..1 { // 后台释放席位只有一个，不能覆盖尚未结束的任务。
            let Some(context) = batch.contexts.get(batch.retirement_cursor) else { break; };
            if !readiness::finished(world, context.entity) { return; }
            let detached = std::mem::take(&mut table_mut::<feathrise_scene_core::residency::MapDetachedRoots>(world, context.entity).0);
            batch.cleanup.extend(detached);
            let replaced = runtime.active.as_ref().is_some_and(|active| active.index.contains_key(&context.path));
            if !replaced {
                if let Some(source) = super::characters::retire(world, context.entity, &context.path) {
                    runtime.releasing = Some(AsyncComputeTaskPool::get().spawn(async move { drop(source); }));
                }
            }
            batch.retirement_cursor += 1;
        }
        if batch.retirement_cursor != batch.contexts.len() { return; }
        batch.cleanup.push(batch.root); batch.cleanup_started = true;
    }
    cleanup_entities(world, &mut batch.cleanup, CLEANUP_BUDGET);
    if !batch.cleanup.is_empty() { return; }
    if let Some(context) = batch.contexts.pop() {
        super::terrain_sources::remove_lookup(world, batch.root, &context.path, context.entity);
        let values = retire_instance_context(world, context.entity);
        runtime.releasing = Some(AsyncComputeTaskPool::get().spawn(async move { drop(values); drop(context); }));
        return;
    }
    let batch = runtime.retired.pop_front().unwrap();
    // 空地图没有上下文可逐条退役，也必须回收其小型“创建完整”索引行。
    super::terrain_sources::remove_empty_lookup(world, batch.root);
    runtime.releasing = Some(AsyncComputeTaskPool::get().spawn(async move { drop(batch); }));
}

pub(super) fn drive(world: &mut World) {
    world.resource_scope(|world, mut runtime: Mut<SubMapRuntime>| {
        world.resource_scope(|world, mut status: Mut<SubMapRuntimeStatus>| {
            let requested = world.resource::<MapInstanceRootRequest>().map_id.clone();
            if runtime.root_map_id != requested {
                if let Some(mut batch) = runtime.candidate.take() { hide(world, &mut batch); runtime.retired.push_back(batch); }
                if let Some(mut batch) = runtime.active.take() { hide(world, &mut batch); runtime.retired.push_back(batch); }
                runtime.root_map_id = requested.clone(); runtime.observed_revision = 0;
                if let Some(hooks) = world.get_resource::<crate::SceneApplicationHooks>().copied() {
                    (hooks.reset_instances)(world, requested.clone());
                }
                status.ready = requested.is_none(); status.applied_revision = 0; status.last_error = None; status.loading_detail = None;
            }
            status.root_map_id = requested.clone();
            retire(world, &mut runtime);
            if requested.is_none() { return; }
            let graph = world.resource::<MapInstanceGraph>();
            if graph.phase == MapInstanceGraphPhase::Failed {
                status.last_error = graph.last_error.clone();
                return; // 坏引用或循环不覆盖上一套完整可用实例。
            }
            if graph.phase != MapInstanceGraphPhase::Ready || graph.root_map_id != requested { return; }
            let revision = graph.revision; let structural_revision = graph.structural_revision; let nodes = graph.instances.clone();
            let epoch = world.resource::<InstanceAssetJournalEpoch>().0;
            status.total = nodes.len().saturating_sub(1);
            if let Some(task) = runtime.comparison.as_mut() {
                if let Some(done) = future::block_on(future::poll_once(task)) {
                    runtime.comparison = None;
                    if runtime.active.as_ref().is_some_and(|batch| done.compatible(batch.root, revision, structural_revision)) {
                        runtime.transform_update = Some(done);
                    }
                }
            }
            if let Some(mut update) = runtime.transform_update.take() {
                if runtime.active.as_ref().is_some_and(|batch| update.compatible(batch.root, revision, structural_revision)) {
                    if update.structural {
                        if runtime.retired.is_empty() && runtime.candidate.is_none() {
                            runtime.candidate = Some(begin_batch(world, &mut runtime, revision, update.graph));
                        } else { runtime.transform_update = Some(update); }
                    } else if let Some(batch) = runtime.active.as_mut() {
                        for _ in 0..32 { // 单个父实例拖动仅改一条相对Transform，后代依靠正式层级传播。
                            let Some(index) = update.poses.pop_front() else { break; };
                            let context = &mut batch.contexts[index - 1];
                            world.entity_mut(context.entity).insert((update.graph[index].local_pose(),
                                MapSourceFrame::from_transform(update.graph[index].world_pose())));
                            if let Some(definition) = &update.graph[index].terrain {
                                world.entity_mut(context.entity).insert(super::terrain_sources::TerrainObjectInstance { definition: definition.clone() });
                            }
                            *world.resource_mut::<MapRuntimeScope>() = MapRuntimeScope { instance: Some(context.entity), frame: MapSourceFrame::from_transform(update.graph[index].world_pose()) };
                            context.follow.run(world);
                            refresh_fall(world, context, &update.graph[index], &mut batch.fall_thresholds);
                            if update.graph[index].terrain.is_none() { super::characters::publish(world, context.entity, &update.graph[index]); }
                            *world.resource_mut::<MapRuntimeScope>() = MapRuntimeScope::default();
                        }
                        if update.poses.is_empty() { batch.graph = update.graph; batch.revision = update.revision; runtime.observed_revision = update.revision;
                            status.applied_revision = update.revision; }
                        else { runtime.transform_update = Some(update); }
                    }
                }
            }
            retire_stale_candidate(world, &mut runtime, revision, epoch);
            if runtime.candidate.is_none() && runtime.retired.is_empty() && runtime.comparison.is_none()
                && runtime.transform_update.is_none() && (runtime.observed_revision != revision || runtime.journal_epoch != epoch) {
                if runtime.journal_epoch != epoch || runtime.active.is_none() {
                    runtime.candidate = Some(begin_batch(world, &mut runtime, revision, nodes));
                    runtime.journal_epoch = epoch;
                } else {
                    let batch = runtime.active.as_ref().unwrap(); let root = batch.root; let old = batch.graph.clone();
                    runtime.comparison = Some(AsyncComputeTaskPool::get().spawn(async move {
                        let mut comparison = planning::compare(root, revision, old, nodes);
                        comparison.structural_revision = structural_revision;
                        comparison
                    }));
                }
            }
            let mut occupied = 0;
            let preparing = runtime.candidate.is_some();
            let generation = runtime.generation;
            let updating_pose = runtime.transform_update.is_some();
            if let Some(batch) = runtime.active.as_mut() {
                // 准备新完整候选时活动实例只结清旧席位，暂停新区域申请，画面继续使用旧完整页。
                occupied = batch.leases.len();
                if !updating_pose && (!preparing || occupied > 0) { advance(world, batch, if !preparing { PREPARATION_SLOTS } else { occupied }, &mut status); }
                occupied = batch.leases.len();
            }
            let retired_empty = runtime.retired.is_empty();
            // 显式借用活动/候选两个字段，供初始化继承预览；作用域内World暂不含协调器资源。
            let runtime_state = &mut *runtime;
            if let Some(batch) = runtime_state.candidate.as_mut() {
                create_contexts(world, generation, batch, runtime_state.active.as_ref());
                advance(world, batch, PREPARATION_SLOTS.saturating_sub(occupied), &mut status);
                status.prepared = batch.prepared;
                if batch.revision == revision && batch.next == batch.graph.len() && batch.prepared == batch.contexts.len() && retired_empty {
                    super::authoring_target::transfer_volume_previews(world, &batch.index);
                    if let Some(mut old) = runtime_state.active.take() { hide(world, &mut old); runtime_state.retired.push_back(old); }
                    let batch = runtime_state.candidate.take().unwrap();
                    world.entity_mut(batch.root).insert((Visibility::Inherited, SubMapActive)).remove::<(RigidBodyDisabled, TnuaNotPlatform)>();
                    if let Some(hooks) = world.get_resource::<crate::SceneApplicationHooks>().copied() {
                        (hooks.expect_instances)(world, batch.graph.iter().skip(1).filter(|node| node.terrain.is_none()).count());
                    }
                    runtime_state.active = Some(batch); runtime_state.observed_revision = revision;
                    status.ready = true; status.applied_revision = revision; status.last_error = None; status.loading_detail = None;
                }
            }
            publish_fall(world, &runtime); // 最低合法子地形进入同一父世界死亡边界，不误杀低于父图的玩家。
        });
    });
}
