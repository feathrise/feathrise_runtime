//! Scene消息积压、真实WorldReady确认与稳定空帧复杂度回归。

use bevy::asset::{AssetEvent, AssetId, uuid::Uuid};
use bevy::ecs::message::{MessageCursor, Messages};
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;

use super::*;

fn asset_id(value: u128) -> AssetId<WorldAsset> {
    AssetId::Uuid {
        uuid: Uuid::from_u128(value),
    }
}

#[test]
fn suppressed_modified_restores_previous_ready_with_bounded_root_step() {
    let mut index = WorldAssetLifecycleIndex::default();
    let root = Entity::from_raw_u32(81).expect("测试根必须合法");
    let asset_id = asset_id(81);
    assert_eq!(
        index.register(root, asset_id),
        WorldAssetRootRegistration::Registered
    );
    assert_eq!(index.mark_world_ready(root, asset_id), Ok(0));

    assert_eq!(
        index.begin_modified_invalidation(asset_id, None, 10),
        (1, true)
    );
    assert_eq!(index.current_ready_epoch(root, asset_id), None); // epoch O(1)推进后旧信用立即失效。
    let WorldAssetInvalidationStep::Root(first) = index.next_invalidation().unwrap() else {
        panic!("首次定向步骤必须撤销根信用");
    };
    assert!(!first.ready_after_event);
    assert!(!first.sweep_complete); // Modified尚未由下一SpawnScene的Ready结果确认。

    index.resolve_provisional_modifications(11, 1); // 没有WorldReady即精确对应Bevy debounce抑制。
    let WorldAssetInvalidationStep::Root(restored) = index.next_invalidation().unwrap() else {
        panic!("第二轮定向步骤必须恢复此前Scene信用");
    };
    assert!(restored.ready_after_event);
    assert!(restored.sweep_complete);
    assert_eq!(index.current_ready_epoch(root, asset_id), Some(1));
}

#[test]
fn confirmed_modified_uses_world_ready_instead_of_reader_frame_debounce_guess() {
    let mut index = WorldAssetLifecycleIndex::default();
    let root = Entity::from_raw_u32(82).expect("测试根必须合法");
    let asset_id = asset_id(82);
    index.register(root, asset_id);
    index.mark_world_ready(root, asset_id).unwrap();

    index.begin_modified_invalidation(asset_id, None, 20);
    assert_eq!(index.mark_world_ready(root, asset_id), Ok(1)); // 模拟同帧SpawnScene真实重生完成。
    index.resolve_provisional_modifications(21, 1);
    assert!(matches!(
        index.next_invalidation(),
        Some(WorldAssetInvalidationStep::Complete { epoch: 1, .. })
    ));
    assert_eq!(index.current_ready_epoch(root, asset_id), Some(1));
}

#[test]
fn event_129_blocks_old_and_new_ready_credit_until_reader_catches_up() {
    let mut index = WorldAssetLifecycleIndex::default();
    let root = Entity::from_raw_u32(83).expect("测试根必须合法");
    let asset_id = asset_id(83);
    index.register(root, asset_id);
    index.mark_world_ready(root, asset_id).unwrap();

    let backlog = index
        .begin_event_collection(
            WORLD_ASSET_EVENT_BUDGET_PER_FRAME + 1,
            WORLD_ASSET_EVENT_BUDGET_PER_FRAME,
        )
        .expect("第129项必须建立保守积压代");
    assert!(!index.events_caught_up());
    index.mark_world_ready(root, asset_id).unwrap(); // 即使浅工作已看到Ready，全局消息门仍关闭。
    let (epoch, _) = index.begin_modified_invalidation(asset_id, Some(backlog), 30);
    let Some(WorldAssetInvalidationStep::Root(promoted)) = index.next_invalidation() else {
        panic!("积压期新Ready必须由定向根步骤追授当前epoch");
    };
    assert!(promoted.ready_after_event);
    assert!(!index.request_epoch_is_current(root, asset_id, epoch));

    index.finish_event_collection(true);
    assert!(index.events_caught_up());
    assert!(index.request_epoch_is_current(root, asset_id, epoch)); // 追平后才允许当前epoch请求推进。
}

#[test]
fn message_cursor_loss_is_detectable_and_permanently_fail_closed() {
    let mut messages = Messages::<AssetEvent<WorldAsset>>::default();
    let cursor = MessageCursor::<AssetEvent<WorldAsset>>::default();
    for value in 0..129u128 {
        messages.write(AssetEvent::Modified {
            id: asset_id(value + 1),
        });
    }
    messages.update();
    for value in 0..129u128 {
        messages.write(AssetEvent::Modified {
            id: asset_id(value + 1_000),
        });
    }
    messages.update(); // 双缓冲淘汰第一批，而测试游标从未消费。
    let missed = cursor.missed_messages(&messages);
    assert_eq!(missed, 129);

    let mut index = WorldAssetLifecycleIndex::default();
    assert!(index.record_missed_messages(missed));
    index.finish_event_collection(true);
    assert!(!index.events_caught_up());
    assert_eq!(index.missed_message_count(), 129); // 未知AssetId不能静默恢复或clear。
    let mut placement = ScenePlacementRuntimeState::default();
    assert!(synchronize_placement_message_gate(&index, &mut placement));
    assert!(!placement.world_asset_events_caught_up); // 游标空也不能把候选采用门反写为true。
}

#[test]
fn stable_frame_source_contains_no_world_root_changed_scan() {
    let lifecycle = include_str!("mod.rs");
    let water = include_str!("../large_scene/water/mesh_overrides/runtime.rs");
    let settlement = include_str!("../placement_data/settlement.rs");
    assert!(!water.contains(concat!("Changed<", "WorldInstance>")));
    assert!(!water.contains(concat!("Changed<", "DynamicWaterMeshOverrides>")));
    assert!(!settlement.contains(concat!("Changed<", "WorldInstance>")));
    assert!(lifecycle.contains("MessageCursor<AssetEvent<WorldAsset>>"));
    assert!(lifecycle.contains("ApplyDeferred"));
}
