//! `WorldAsset`热重载的定向根epoch、固定容量索引与有界失效分发。
//!
//! 【功能设计】Bevy 0.19原地热重载会复用`InstanceId`，因此本域在`Update`
//! （严格早于`SpawnScene`）先推进资产epoch。旧水工作和隐藏候选立即失去采用资格；
//! 根级Pending/Ready集合再按固定预算定向撤销，新`WorldInstanceReady`必须携当前epoch重新授信。

mod index;

use bevy::asset::{AssetEvent, AssetId};
use bevy::diagnostic::FrameCount;
use bevy::ecs::message::{MessageCursor, Messages};
use bevy::ecs::schedule::ApplyDeferred;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};

use crate::shared::large_scene::{DynamicWaterMeshOverrideStatus, DynamicWaterMeshOverrides};
use crate::shared::placement_data::{
    ScenePlacementRuntimeState, ScenePlacementRuntimeStatus, complete_world_asset_invalidation,
    observe_world_asset_event, world_asset_event_is_relevant,
};

use index::WorldAssetInvalidationStep;
pub(crate) use index::{WorldAssetLifecycleIndex, WorldAssetRootRegistration};

/// 单帧最多分发的精确根失效数；与placement生成/结算预算一致。
pub(crate) const WORLD_ASSET_ROOT_INVALIDATION_BUDGET_PER_FRAME: usize = 128;
/// 单帧最多读取的Scene资产消息；超额项留在`MessageReader`并全局阻止旧工作采用。
pub(crate) const WORLD_ASSET_EVENT_BUDGET_PER_FRAME: usize = 128;
/// 单帧最多确认的临时Modified资产数；以真实WorldReady而非消息读取帧镜像Bevy debounce。
pub(crate) const WORLD_ASSET_MODIFICATION_RESOLUTION_BUDGET_PER_FRAME: usize = 128;

/// 相关Scene资产epoch已经推进、但该根尚未取得新WorldReady。
#[derive(Clone, Copy, Debug, EntityEvent)]
pub(crate) struct WorldAssetRootEpochInvalidated {
    pub(crate) entity: Entity,
    pub(crate) asset_id: AssetId<WorldAsset>,
    pub(crate) epoch: u64,
    pub(crate) ready_after_event: bool,
}

/// 供水体任务和placement结算建立显式调度依赖。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SystemSet)]
pub(crate) enum WorldAssetLifecycleSystems {
    CollectEvents,
    InvalidateRoots,
}

fn event_asset_id(event: &AssetEvent<WorldAsset>) -> AssetId<WorldAsset> {
    match event {
        AssetEvent::Added { id }
        | AssetEvent::Modified { id }
        | AssetEvent::Removed { id }
        | AssetEvent::LoadedWithDependencies { id }
        | AssetEvent::Unused { id } => *id,
    }
}

fn synchronize_placement_message_gate(
    index: &WorldAssetLifecycleIndex,
    state: &mut ScenePlacementRuntimeState,
) -> bool {
    let safe_caught_up = index.events_caught_up();
    let changed = state.world_asset_events_caught_up != safe_caught_up;
    state.world_asset_events_caught_up = safe_caught_up;
    changed
}

/// 唯一WorldAsset消息读者同时驱动水体epoch和候选Scene加载状态，避免两个游标双重换代。
fn collect_world_asset_lifecycle_events(
    mut cursor: Local<MessageCursor<AssetEvent<WorldAsset>>>,
    messages: Res<Messages<AssetEvent<WorldAsset>>>,
    frame_count: Res<FrameCount>,
    mut index: ResMut<WorldAssetLifecycleIndex>,
    mut placement_state: Option<ResMut<ScenePlacementRuntimeState>>,
    mut placement_status: Option<ResMut<ScenePlacementRuntimeStatus>>,
) {
    let missed = cursor.missed_messages(&messages);
    if missed != 0 && index.record_missed_messages(missed) {
        error!(
            "WorldAsset生命周期消息丢失{missed}条：固定预算读者落后于Messages双缓冲，整网格水与placement采用已永久失败关闭"
        );
        if let Some(state) = placement_state.as_deref_mut() {
            state.world_asset_events_caught_up = false;
        }
        if let Some(status) = placement_status.as_deref_mut() {
            status.phase = crate::shared::placement_data::ScenePlacementRuntimePhase::Failed;
            status.detail = format!(
                "WorldAsset生命周期消息丢失{missed}条；请重启编辑器后减少同帧Scene热更突发"
            );
            status.advance_progress();
        }
    }
    let backlog_generation =
        index.begin_event_collection(cursor.len(&messages), WORLD_ASSET_EVENT_BUDGET_PER_FRAME);
    if let Some(state) = placement_state.as_deref_mut()
        && !index.events_caught_up()
    {
        state.world_asset_events_caught_up = false; // 必须早于本帧水工作和候选采用关闭全局门。
    }
    for event in cursor
        .read(&messages)
        .take(WORLD_ASSET_EVENT_BUDGET_PER_FRAME)
    {
        let asset_id = event_asset_id(event);
        let placement_relevant = placement_state
            .as_deref()
            .is_some_and(|state| world_asset_event_is_relevant(state, asset_id));
        let indexed = index.contains_asset(asset_id);
        let invalidation = match event {
            AssetEvent::Added { .. } => {
                None // 初载尚无旧WorldReady；Bevy也不会用Added重生既有实例。
            }
            AssetEvent::Modified { .. } => {
                (indexed || placement_relevant).then(|| {
                    index.begin_modified_invalidation(asset_id, backlog_generation, frame_count.0)
                }) // 是否真重生由下一Update观察WorldReady精确确认，不按续读帧猜Bevy debounce。
            }
            AssetEvent::Removed { .. } => {
                (indexed || placement_relevant).then(|| index.invalidate_removed_asset(asset_id))
            }
            AssetEvent::LoadedWithDependencies { .. } | AssetEvent::Unused { .. } => None,
        };
        if let (Some(state), Some(status)) = (
            placement_state.as_deref_mut(),
            placement_status.as_deref_mut(),
        ) {
            observe_world_asset_event(event, asset_id, invalidation, state, status);
        }
    }
    let caught_up = cursor.is_empty(&messages); // `take`只推进固定项，绝不clear丢弃尚未归属的Modified。
    index.finish_event_collection(caught_up);
    if let Some(state) = placement_state.as_deref_mut() {
        let changed = synchronize_placement_message_gate(&index, state); // missed即使游标已空也保持false。
        if changed && let Some(status) = placement_status.as_deref_mut() {
            status.advance_progress(); // 全局采用门开合属于真实生命周期进展，等待证据不会假停滞。
        }
    }
}

/// 固定预算把资产级O(1)失效投递到精确根observer；稳定空帧只检查一次队首。
fn process_world_asset_root_invalidations(
    mut commands: Commands,
    mut index: ResMut<WorldAssetLifecycleIndex>,
    mut placement_state: Option<ResMut<ScenePlacementRuntimeState>>,
    mut placement_status: Option<ResMut<ScenePlacementRuntimeStatus>>,
    frame_count: Res<FrameCount>,
) {
    index.resolve_provisional_modifications(
        frame_count.0,
        WORLD_ASSET_MODIFICATION_RESOLUTION_BUDGET_PER_FRAME,
    );
    for _ in 0..WORLD_ASSET_ROOT_INVALIDATION_BUDGET_PER_FRAME {
        let Some(step) = index.next_invalidation() else {
            break;
        };
        let completion = match step {
            WorldAssetInvalidationStep::Root(root) => {
                commands.trigger(WorldAssetRootEpochInvalidated {
                    entity: root.entity,
                    asset_id: root.asset_id,
                    epoch: root.epoch,
                    ready_after_event: root.ready_after_event,
                });
                root.sweep_complete.then_some((root.asset_id, root.epoch))
            }
            WorldAssetInvalidationStep::Complete { asset_id, epoch } => Some((asset_id, epoch)),
            WorldAssetInvalidationStep::Skipped => None,
        };
        if let (Some((asset_id, epoch)), Some(state), Some(status)) = (
            completion,
            placement_state.as_deref_mut(),
            placement_status.as_deref_mut(),
        ) {
            complete_world_asset_invalidation(state, status, asset_id, epoch);
        }
    }
}

/// 覆盖组件插入时登记当前强Scene0句柄；同一bundle重复通知保持幂等。
fn register_inserted_mesh_override_root(
    insert: On<Insert<DynamicWaterMeshOverrides>>,
    roots: Query<&WorldAssetRoot>,
    mut index: ResMut<WorldAssetLifecycleIndex>,
    mut commands: Commands,
) {
    let Ok(root) = roots.get(insert.entity) else {
        return;
    };
    fail_registration_if_full(
        insert.entity,
        index.register(insert.entity, root.0.id()),
        &mut commands,
    );
}

/// `WorldAssetRoot`后插入或替换时把既有覆盖组件迁到新资产桶。
fn register_inserted_world_asset_root(
    insert: On<Insert<WorldAssetRoot>>,
    roots: Query<&WorldAssetRoot, Or<(With<DynamicWaterMeshOverrides>, With<crate::shared::placement_data::PlacementPendingInstance>, With<crate::shared::placement_data::ScenePlacementInstance>)>>,
    mut index: ResMut<WorldAssetLifecycleIndex>,
    mut commands: Commands,
) {
    let Ok(root) = roots.get(insert.entity) else {
        return;
    };
    fail_registration_if_full(
        insert.entity,
        index.register(insert.entity, root.0.id()),
        &mut commands,
    );
}

fn fail_registration_if_full(
    entity: Entity,
    result: WorldAssetRootRegistration,
    commands: &mut Commands,
) {
    if result != WorldAssetRootRegistration::Full {
        return;
    }
    let detail = format!(
        "WorldAsset整网格水根索引超过{}项固定上限",
        index::MAX_INDEXED_WORLD_ASSET_ROOTS,
    );
    error!("{detail}：root={entity:?}");
    commands
        .entity(entity)
        .try_insert(DynamicWaterMeshOverrideStatus::Failed(detail));
}

fn unregister_discarded_world_asset_root(
    discard: On<Discard<WorldAssetRoot>>,
    roots: Query<(), Or<(With<DynamicWaterMeshOverrides>, With<crate::shared::placement_data::PlacementPendingInstance>, With<crate::shared::placement_data::ScenePlacementInstance>)>>,
    mut index: ResMut<WorldAssetLifecycleIndex>,
) {
    if roots.contains(discard.entity) {
        index.unregister(discard.entity);
    }
}

fn unregister_removed_mesh_override_root(
    remove: On<Remove<DynamicWaterMeshOverrides>>,
    mut index: ResMut<WorldAssetLifecycleIndex>,
) {
    index.unregister(remove.entity);
}

/// 两个上层插件都会调用；首个调用者完成唯一资源、系统和observer安装。
pub(crate) fn install_world_asset_lifecycle(app: &mut App) {
    if app.world().contains_resource::<WorldAssetLifecycleIndex>() {
        return;
    }
    app.init_resource::<WorldAssetLifecycleIndex>()
        .configure_sets(
            Update,
            (
                WorldAssetLifecycleSystems::CollectEvents,
                WorldAssetLifecycleSystems::InvalidateRoots,
            )
                .chain(),
        )
        .add_systems(
            Update,
            collect_world_asset_lifecycle_events.in_set(WorldAssetLifecycleSystems::CollectEvents),
        )
        .add_systems(
            Update,
            (process_world_asset_root_invalidations, ApplyDeferred)
                .chain()
                .in_set(WorldAssetLifecycleSystems::InvalidateRoots), // observer命令在水处理与候选采用前显式落地。
        )
        .add_observer(register_inserted_mesh_override_root)
        .add_observer(register_inserted_world_asset_root)
        .add_observer(unregister_discarded_world_asset_root)
        .add_observer(unregister_removed_mesh_override_root);
}

#[cfg(test)]
mod tests;
