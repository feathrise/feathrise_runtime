//! 显式摆放JSON与其Scene0依赖的资产事件观察和候选采用阻断。

use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use bevy::asset::{Asset, AssetEvent, AssetId}; // 消息只携带类型化资产身份，不读取或同步遍历内容。
use bevy::prelude::*; // MessageReader、Res和资源可变借用。
use bevy::world_serialization::WorldAsset; // Scene0热更事件使用统一World资产类型。
use feathrise_scene_content::{ActiveDynamicScene, ScenePlacementsAsset}; // 当前场景目录把JSON事件缩小到唯一正式摆放文档。

use super::super::resources::{
    ScenePlacementRebuildRequest, ScenePlacementRuntimePhase, ScenePlacementRuntimeState,
    ScenePlacementRuntimeStatus,
}; // 事件只推进脏标记、重试门和当前候选阻断身份。

/// 从任一种资产生命周期消息取得统一类型化ID。
fn event_asset_id<A: Asset>(event: &AssetEvent<A>) -> AssetId<A> {
    match event {
        AssetEvent::Added { id }
        | AssetEvent::Modified { id }
        | AssetEvent::Removed { id }
        | AssetEvent::LoadedWithDependencies { id }
        | AssetEvent::Unused { id } => *id,
    }
}

/// 资产事件只设置脏标记；失败场景修复后按匹配句柄重新检查，稳定帧不遍历世界。
pub(crate) fn observe_placement_data_events(
    registry: Option<crate::scenes::dynamic_scene::sources::SceneRuntimeCatalog>,
    active_dynamic: InstanceRes<ActiveDynamicScene>, // 显式重建按实际当前动态场景接纳，不依赖可能仍滞后一帧的诊断状态。
    mut document_events: feathrise_scene_core::residency::ResidentAssetEventReader<ScenePlacementsAsset>,
    mut rebuild_requests: MessageReader<ScenePlacementRebuildRequest>, // 原样重试只为当前正式场景设置同一脏标记。
    mut state: InstanceResMut<ScenePlacementRuntimeState>,
    status: InstanceRes<ScenePlacementRuntimeStatus>, // 只读取尚未由ActiveDynamicScene发布的当前场景身份。
    author_preview: Option<crate::scenes::dynamic_scene::author_sources::TerrainAuthorRes<feathrise_scene_content::SceneAuthorPreview>>,
    lifecycle: Option<Res<crate::shared::world_asset_lifecycle::WorldAssetLifecycleIndex>>,
) {
    if let Some(index) = lifecycle { state.world_asset_events_caught_up = index.events_caught_up(); }
    let registry_changed = registry
        .as_deref()
        .is_some_and(|registry| registry.revision != state.catalog_revision);
    let current_scene_id = active_dynamic
        .scene_id
        .as_ref()
        .or(status.scene_id.as_ref())
        .cloned(); // 先冻结当前场景ID，后续才可安全修改公开进度证据。
    let current_document_id = registry.as_deref().and_then(|registry| {
        current_scene_id
            .as_ref()
            .and_then(|scene_id| registry.handles(scene_id))
            .map(|handles| {
                author_preview
                    .as_deref()
                    .and_then(|preview| preview.for_scene(current_scene_id.as_ref().unwrap()))
                    .map(|preview| preview.placements.id())
                    .unwrap_or(handles.placements.id())
            })
    });
    let mut document_changed = false; // 必须消费本帧全部消息，不让`any`短路留下过期事件。
    for event in document_events.read() {
        document_changed |= Some(event_asset_id(event)) == current_document_id; // 另一场景的保存事件不能为当前交接门制造无关新候选。
    }
    let mut rebuild_requested = false; // 原样重试消息同样全部消费。
    for request in rebuild_requests.read() {
        rebuild_requested |= active_dynamic.scene_id.as_ref() == Some(&request.scene_id); // 只有当前动态场景可重建自己的正式文件。
    }
    let preview_changed = state.observed_document != current_document_id;
    state.observed_document = current_document_id;
    if registry_changed || document_changed || rebuild_requested || preview_changed {
        state.dirty = true;
    }
}

/// 共享WorldAsset消息读者用O(1)集合命中当前规划/候选实际引用的Scene0。
pub(crate) fn world_asset_event_is_relevant(
    state: &ScenePlacementRuntimeState,
    asset_id: AssetId<WorldAsset>,
) -> bool {
    state
        .scene_load
        .as_ref()
        .is_some_and(|tracker| tracker.scene_ids.contains(&asset_id))
}

/// 处理一个已经由共享生命周期域判定的Scene事件，不再次推进资产epoch。
pub(crate) fn observe_world_asset_event(
    event: &AssetEvent<WorldAsset>,
    asset_id: AssetId<WorldAsset>,
    invalidation: Option<(u64, bool)>, // Some仅表示Bevy会重生实例或资产已被移除；bool说明已有索引根需扫尾。
    state: &mut ScenePlacementRuntimeState,
    status: &mut ScenePlacementRuntimeStatus,
) {
    if !world_asset_event_is_relevant(state, asset_id) {
        return;
    }
    let positive = matches!(
        event,
        AssetEvent::Added { .. }
            | AssetEvent::Modified { .. }
            | AssetEvent::LoadedWithDependencies { .. }
    );
    let ready_before = state
        .scene_load
        .as_ref()
        .is_some_and(|tracker| tracker.ready_scene_ids.contains(&asset_id));
    if let Some(tracker) = state.scene_load.as_mut() {
        super::loading::queue_scene_check(tracker, asset_id, invalidation.is_some()); // 只有真实World重生/移除撤销依赖信用。
    }
    let mut staging_blocked = false;
    if let Some((epoch, has_indexed_roots)) = invalidation
        && has_indexed_roots
    {
        let tracker_identity = state
            .scene_load
            .as_ref()
            .map(|tracker| (tracker.generation, tracker.revision));
        if let (Some((generation, revision)), Some(staging)) =
            (tracker_identity, state.staging.as_mut())
            && staging.generation == generation
            && staging.revision == revision
            && staging.blocked_scene_assets.insert(asset_id, epoch) != Some(epoch)
        {
            staging.eligibility_epoch = staging.eligibility_epoch.wrapping_add(1);
            staging.verification_cursor = 0; // 资产级O(1)阻断先于后续128根/帧扫尾生效。
            staging_blocked = true;
        }
    }
    let ready_revoked = ready_before
        && state
            .scene_load
            .as_ref()
            .is_some_and(|tracker| !tracker.ready_scene_ids.contains(&asset_id));
    if ready_revoked || staging_blocked {
        status.advance_progress(); // 反向换代与全局采用阻断都是真实生命周期推进。
    }
    if status.phase == ScenePlacementRuntimePhase::Failed && positive && !state.asset_retry_requested
    {
        state.asset_retry_requested = true;
        status.advance_progress();
    }
}

/// 精确资产的所有旧根信用已经撤销或被新WorldReady追平，解除本epoch全局采用阻断。
pub(crate) fn complete_world_asset_invalidation(
    state: &mut ScenePlacementRuntimeState,
    status: &mut ScenePlacementRuntimeStatus,
    asset_id: AssetId<WorldAsset>,
    epoch: u64,
) {
    let Some(staging) = state.staging.as_mut() else {
        return;
    };
    if staging.blocked_scene_assets.get(&asset_id) != Some(&epoch) {
        return; // 同资产更晚事件已接管阻断，旧扫尾完成值无资格解除。
    }
    staging.blocked_scene_assets.remove(&asset_id);
    staging.eligibility_epoch = staging.eligibility_epoch.wrapping_add(1);
    staging.verification_cursor = 0;
    status.advance_progress();
}
