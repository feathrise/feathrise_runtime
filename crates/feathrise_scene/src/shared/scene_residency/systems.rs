//! 驻留事务与原状态机交接；准备期间非阻塞推进，完整资源和实体就绪才原子切换。

use super::{
    entities::{self, ENTITY_BUDGET},
    resources::*,
};
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, futures::check_ready},
};
use feathrise_map_content::{ActiveDynamicMap, MapId};
use feathrise_scene_core::residency::{
    SceneResidencyActivity, restore_resident_resources, take_resident_resources,
};
use feathrise_scene_core::state::{ScenePhase, LoadingProgress, PendingSceneTransition};

/// 作者入口先冻结源地图，后台历史与实体暂停并行准备；此时还不更换正式地图身份。
pub fn prepare_resident_scene(world: &mut World, target: MapId, scene: ScenePhase) {
    if !world
        .get_resource::<SceneResidencyActivity>()
        .is_some_and(|state| state.enabled)
    {
        return;
    }
    let mut scenes = world.remove_resource::<ResidentScenes>().unwrap();
    if scenes.transfer.is_none() {
        if let Some(token) = scenes
            .active
            .as_ref()
            .and_then(|map| scenes.maps.get(map))
            .map(|entry| entry.token)
        {
            world.trigger(feathrise_scene_core::residency::ResidentSceneParking(token));
        }
        scenes.ensure(target.clone(), scene);
        let warm = scenes.sleeping(&target);
        scenes.transfer = Some(ResidentTransfer {
            source: scenes.active.clone(),
            target: Some((target, scene)),
            phase: TransferPhase::Parking,
            cursor: 0,
            warm,
            deferred_next: None,
            transition_scene: None,
            loading_transaction: None,
        });
        scenes.restored = None;
        world.resource_mut::<SceneResidencyActivity>().switching = true;
    }
    world.insert_resource(scenes);
}

/// 原新游戏/读档仍执行完整冷启动，不能把上一局缓存的NPC、玩家或死亡状态当作新存档。
pub(super) fn begin_fresh_world(
    pending: Option<Res<PendingSceneTransition>>,
    mut scenes: ResMut<ResidentScenes>,
) {
    if pending.is_some_and(|pending| pending.fresh_world) {
        scenes.world_epoch = scenes.world_epoch.wrapping_add(1);
        scenes.restored = None;
    }
}

/// 驻留场景只跳过已经做过的进入/退出构建；正式状态、作者采用和完整就绪门继续使用原契约。
pub fn arm_resident_scene_transition(world: &mut World) {
    if let Some(mut scenes) = world.get_resource_mut::<ResidentScenes>()
        && let Some(transfer) = scenes.transfer.as_mut()
    {
        transfer.phase = TransferPhase::Armed;
    }
}

pub fn close_resident_scene(world: &mut World, map: MapId) {
    let Some(mut scenes) = world.get_resource_mut::<ResidentScenes>() else {
        return;
    };
    if let Some(entry) = scenes.maps.get_mut(&map) {
        entry.document_open = false;
    }
    if !scenes.closing.contains(&map) {
        scenes.closing.push_back(map);
    }
    if scenes.active.as_ref() == scenes.closing.back() {
        world.resource_mut::<SceneResidencyActivity>().switching = true; // 关闭采用点立即冻结作者派生清理，随后按实体预算回收。
    }
}

pub fn mark_resident_document_open(world: &mut World, map: MapId) {
    if let Some(mut scenes) = world.get_resource_mut::<ResidentScenes>()
        && let Some(entry) = scenes.maps.get_mut(&map)
    {
        entry.document_open = true;
    }
}

pub(super) fn record_active_scene(
    current: Res<State<ScenePhase>>,
    active: Option<Res<ActiveDynamicMap>>,
    mut scenes: ResMut<ResidentScenes>,
    mut activity: ResMut<SceneResidencyActivity>,
) {
    if !activity.enabled {
        return;
    }
    let Some(map) = entities::map_for_scene(*current.get(), active.as_deref()) else {
        return;
    };
    let token = scenes.ensure(map.clone(), *current.get());
    scenes.active = Some(map);
    activity.active = Some(token);
}

/// 作者按钮以外的传送/返回菜单也保护已打开文档；不属于标签的普通场景沿用原卸载。
pub(super) fn intercept_scene_transition(world: &mut World) {
    if !world.resource::<SceneResidencyActivity>().enabled {
        return;
    }
    let requested_scene = match world.get_resource::<NextState<ScenePhase>>() {
        Some(NextState::PendingIfNeq(scene))
            if world
                .get_resource::<State<ScenePhase>>()
                .is_some_and(|state| state.get() == scene) =>
        {
            return;
        }
        Some(NextState::Pending(scene) | NextState::PendingIfNeq(scene)) => *scene,
        _ => return,
    };
    let mut scenes = world.remove_resource::<ResidentScenes>().unwrap();
    if scenes.transfer.is_none() {
        let source_open = scenes
            .active
            .as_ref()
            .and_then(|id| scenes.maps.get(id))
            .is_some_and(|entry| entry.document_open)
            || scenes
                .active
                .as_ref()
                .is_some_and(|id| scenes.closing.contains(id));
        let target =
            entities::map_for_scene(requested_scene, world.get_resource::<ActiveDynamicMap>())
                .filter(|id| {
                    scenes.sleeping(id)
                        && scenes
                            .maps
                            .get(id)
                            .is_some_and(|entry| entry.scene == requested_scene)
                });
        if source_open || target.is_some() {
            if let Some(token) = scenes
                .active
                .as_ref()
                .and_then(|map| scenes.maps.get(map))
                .map(|entry| entry.token)
            {
                world.trigger(feathrise_scene_core::residency::ResidentSceneParking(token));
            }
            let next = std::mem::take(&mut *world.resource_mut::<NextState<ScenePhase>>());
            scenes.transfer = Some(ResidentTransfer {
                source: scenes.active.clone(),
                target: target.clone().map(|id| (id, requested_scene)),
                phase: TransferPhase::Parking,
                cursor: 0,
                warm: target.is_some(),
                deferred_next: Some(next),
                transition_scene: None,
                loading_transaction: None,
            });
            world.resource_mut::<SceneResidencyActivity>().switching = true;
        }
    }
    if let Some(transfer) = scenes.transfer.as_mut() {
        if !matches!(transfer.phase, TransferPhase::Armed) {
            // 外部请求只暂存一次，Entity分帧准备期间不能提前执行状态退出。
            let next = std::mem::take(&mut *world.resource_mut::<NextState<ScenePhase>>());
            if transfer.deferred_next.is_none() {
                transfer.deferred_next = Some(next);
            }
        } else {
            transfer.transition_scene = Some(requested_scene);
            transfer.loading_transaction = world.get_resource::<LoadingProgress>().map(LoadingProgress::transaction);
            let source_scene = transfer
                .source
                .as_ref()
                .and_then(|id| scenes.maps.get(id))
                .map(|entry| entry.scene);
            let mut schedules = world.resource_mut::<Schedules>();
            if let Some(source) = source_scene {
                if let Some(schedule) = schedules.remove(OnExit(source)) {
                    scenes.saved_schedules.push(schedule);
                }
            }
            if transfer.warm {
                if let Some(schedule) = schedules.remove(OnEnter(requested_scene)) {
                    scenes.saved_schedules.push(schedule);
                }
            }
        }
    }
    world.insert_resource(scenes);
}

/// Source/target都按固定预算移动Entity资格；资源槽只移动所有权，不扫描完整作者源。
pub(super) fn advance_scene_residency(world: &mut World) {
    let mut scenes = world.remove_resource::<ResidentScenes>().unwrap();
    let Some(mut transfer) = scenes.transfer.take() else {
        world.insert_resource(scenes);
        return;
    };
    match transfer.phase {
        TransferPhase::Parking => {
            let token = transfer
                .source
                .as_ref()
                .and_then(|id| scenes.maps.get(id))
                .map(|entry| entry.token);
            if let Some(token) = token {
                for _ in 0..ENTITY_BUDGET {
                    let entity = world
                        .resource::<ResidentEntityIndex>()
                        .maps
                        .get(&token)
                        .and_then(|entities| entities.get(transfer.cursor))
                        .copied();
                    let Some(entity) = entity else {
                        break;
                    };
                    transfer.cursor += 1;
                    entities::park_entity(world, entity);
                }
                let count = world
                    .resource::<ResidentEntityIndex>()
                    .maps
                    .get(&token)
                    .map_or(0, Vec::len);
                let parked = world
                    .resource::<ResidentEntityIndex>()
                    .parked_counts
                    .get(&token)
                    .copied()
                    .unwrap_or(0);
                if parked < count
                    || !world
                        .resource::<ResidentEntityIndex>()
                        .descendants
                        .is_empty()
                {
                    if transfer.cursor >= count {
                        transfer.cursor = 0;
                    } // 热重建的swap_remove不能漏掉交换到游标之前的实体。
                    scenes.transfer = Some(transfer);
                    world.insert_resource(scenes);
                    return;
                }
                let entry = scenes
                    .maps
                    .get_mut(transfer.source.as_ref().unwrap())
                    .unwrap();
                entry.resources = Some(take_resident_resources(world));
                entry.sleeping = true;
            }
            transfer.cursor = 0;
            if transfer.warm {
                let entry = scenes
                    .maps
                    .get_mut(&transfer.target.as_ref().unwrap().0)
                    .unwrap();
                if let Some(mut snapshot) = entry.resources.take() {
                    restore_resident_resources(world, &mut snapshot);
                }
                transfer.phase = TransferPhase::Restoring;
            } else {
                transfer.phase = TransferPhase::Parked;
            }
        }
        TransferPhase::Restoring => {
            let entry = scenes
                .maps
                .get(&transfer.target.as_ref().unwrap().0)
                .unwrap();
            for _ in 0..ENTITY_BUDGET {
                let entity = world
                    .resource::<ResidentEntityIndex>()
                    .maps
                    .get(&entry.token)
                    .and_then(|entities| entities.get(transfer.cursor))
                    .copied();
                let Some(entity) = entity else {
                    break;
                };
                transfer.cursor += 1;
                entities::restore_entity(world, entity);
            }
            let count = world
                .resource::<ResidentEntityIndex>()
                .maps
                .get(&entry.token)
                .map_or(0, Vec::len);
            let parked = world
                .resource::<ResidentEntityIndex>()
                .parked_counts
                .get(&entry.token)
                .copied()
                .unwrap_or(0);
            if parked == 0
                && world
                    .resource::<ResidentEntityIndex>()
                    .descendants
                    .is_empty()
            {
                transfer.phase = TransferPhase::Ready;
            } else if transfer.cursor >= count {
                transfer.cursor = 0;
            }
        }
        TransferPhase::Parked => {
            if let Some(next) = transfer.deferred_next.take() {
                *world.resource_mut::<NextState<ScenePhase>>() = next;
                transfer.phase = TransferPhase::Armed;
            }
        }
        TransferPhase::Ready => {
            if let Some(next) = transfer.deferred_next.take() {
                *world.resource_mut::<NextState<ScenePhase>>() = next;
                transfer.phase = TransferPhase::Armed;
            }
        }
        TransferPhase::Armed => {}
    }
    scenes.transfer = Some(transfer);
    world.insert_resource(scenes);
}

pub(super) fn finish_scene_transition(world: &mut World) {
    let mut scenes = world.remove_resource::<ResidentScenes>().unwrap();
    {
        let mut schedules = world.resource_mut::<Schedules>();
        for schedule in scenes.saved_schedules.drain(..) {
            schedules.insert(schedule);
        }
    }
    let complete = scenes.transfer.as_ref().is_some_and(|transfer| {
        matches!(transfer.phase, TransferPhase::Armed)
            && transfer.transition_scene.is_some_and(|scene| {
                world
                    .get_resource::<State<ScenePhase>>()
                    .is_some_and(|state| *state.get() == scene)
            })
    });
    let mut activated = None;
    if complete {
        let transfer = scenes.transfer.take().unwrap();
        // 冷载只先进入LoadingScene，目标文档此时可已打开，但完整场景尚不存在。
        // 只有实际进入的状态与目标一致才发布活动身份，避免加载门再次暂停目标并搬走新启动门。
        if let Some((target, scene)) = transfer
            .target
            .filter(|(_, scene)| transfer.transition_scene == Some(*scene))
        {
            let token = scenes.ensure(target.clone(), scene);
            scenes.active = Some(target.clone());
            if let Some(entry) = scenes.maps.get_mut(&target) {
                entry.sleeping = false;
            }
            let mut activity = world.resource_mut::<SceneResidencyActivity>();
            activity.active = Some(token);
            activity.restored = transfer.warm.then_some(token);
            scenes.restored = transfer.warm.then_some(target);
            if transfer.warm {
                activated = Some(token);
                if let Some(transaction) = transfer.loading_transaction
                    && let Some(mut progress) = world.get_resource_mut::<LoadingProgress>()
                {
                    // 原门可能已在首次加载完成后移除；完整暖采用发布当前事务的独立完成证明。
                    progress.complete_resident_restore(transaction, scene, token);
                }
                if let Some(cameras) = world
                    .resource_mut::<ResidentEntityIndex>()
                    .cameras
                    .remove(&token)
                {
                    for (entity, active) in cameras {
                        if let Some(mut camera) = world.get_mut::<Camera>(entity) {
                            // 驻留相机已初始化过，但Disabled期间可能错过resize/DPI消息。
                            // 只在完整暖采用时撤销旧目标摘要，由正式camera_system按当前Window/Image重建投影。
                            if active { camera.computed.target_info = None; }
                            camera.is_active = active;
                        }
                    }
                } // 只有完整场景采用后才开启该图视图；原加载遮罩自行处理完成阶段。
            }
        } else {
            scenes.active = None;
            world.resource_mut::<SceneResidencyActivity>().active = None;
            scenes.restored = None;
            world.resource_mut::<SceneResidencyActivity>().restored = None;
        }
        if let Some(source) = transfer.source
            && scenes
                .maps
                .get(&source)
                .is_some_and(|entry| !entry.document_open)
            && !scenes.closing.contains(&source)
        {
            scenes.closing.push_back(source);
        }
        world.resource_mut::<SceneResidencyActivity>().switching = false;
    }
    world.insert_resource(scenes);
    if let Some(token) = activated {
        world.trigger(feathrise_scene_core::residency::ResidentSceneActivated(token));
    }
}

/// 关闭后的树按叶节点固定预算回收，禁止根despawn触发整图同步递归；大资源在唯一后台槽析构。
pub(super) fn retire_closed_scenes(world: &mut World) {
    let mut scenes = world.remove_resource::<ResidentScenes>().unwrap();
    if let Some(task) = scenes.retirement.as_mut()
        && check_ready(task).is_some()
    {
        scenes.retirement = None;
    }
    if scenes.retirement.is_none()
        && let Some(snapshot) = scenes.retired_resources.pop_front()
    {
        scenes.retirement = Some(AsyncComputeTaskPool::get().spawn(async move {
            drop(snapshot);
        }));
    }
    if let Some(token) = scenes.retired_scenes.front().map(|entry| entry.token) {
        if retire_entity_batch(world, token, &mut scenes) {
            let old = scenes.retired_scenes.pop_front().unwrap();
            if let Some(snapshot) = old.resources {
                scenes.retired_resources.push_back(Box::new(snapshot));
            }
        }
        world.insert_resource(scenes);
        return; // 新世代旧图与显式关闭共用一个预算，不能叠加成整图回收。
    }
    let Some(map) = scenes.closing.front().cloned() else {
        world.insert_resource(scenes);
        return;
    };
    let can_retire = scenes.maps.get(&map).is_none_or(|entry| entry.sleeping
        || (scenes.active.as_ref() != Some(&map) && scenes.transfer.is_none())); // LoadingScene取消的冷目标尚未成为活动图，也必须回收其占位槽。
    if can_retire {
        let token = scenes.maps.get(&map).map(|entry| entry.token);
        if let Some(token) = token {
            if retire_entity_batch(world, token, &mut scenes) {
                if let Some(entry) = scenes.maps.remove(&map) {
                    if let Some(snapshot) = entry.resources {
                        scenes.retired_resources.push_back(Box::new(snapshot));
                    }
                }
                scenes.closing.pop_front();
            }
        } else {
            scenes.closing.pop_front();
        }
    }
    world.insert_resource(scenes);
}

fn retire_entity_batch(
    world: &mut World,
    token: feathrise_scene_core::residency::ResidentMapToken,
    scenes: &mut ResidentScenes,
) -> bool {
    for _ in 0..ENTITY_BUDGET {
        let entity = world
            .resource::<ResidentEntityIndex>()
            .maps
            .get(&token)
            .and_then(|values| values.last())
            .copied();
        let Some(entity) = entity else {
            break;
        };
        if let Some(child) = world
            .get::<Children>(entity)
            .and_then(|children| children.first())
            .copied()
        {
            if let Ok(mut child) = world.get_entity_mut(child) {
                child.remove::<ChildOf>();
            }
            continue; // 先摘关系，子实体继续由同一索引回收，避免根despawn递归析构。
        }
        if !world.despawn(entity) {
            world.resource_mut::<ResidentEntityIndex>().remove(entity);
        }
    }
    let mut index = world.resource_mut::<ResidentEntityIndex>();
    let complete = index.maps.get(&token).is_none_or(Vec::is_empty);
    if complete {
        index.maps.remove(&token);
        index.parked_counts.remove(&token);
        index.cameras.remove(&token);
    }
    drop(index);
    if complete {
        super::assets::forget(world, token, scenes);
    }
    complete
}
