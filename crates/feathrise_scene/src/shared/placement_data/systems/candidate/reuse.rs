//! 隐藏候选复用未变正式根；分帧索引与搬运，不重建玩法组件或World实例。

use super::super::super::components::{
    PlacementBuildingCollisionReady, PlacementWorldInstanceReady,
};
use super::*;
use crate::shared::world_asset_lifecycle::WorldAssetLifecycleIndex;
use std::hash::{Hash, Hasher};

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PlacementReuseInputs<'w, 's> {
    roots: InstanceQuery<
        'w,
        's,
        (
            &'static ScenePlacementInstance,
            Option<&'static DynamicWaterMeshOverrides>,
            Option<&'static WorldAssetRoot>,
            Option<&'static PlacementBuildingCollisionGeneration>,
            Option<&'static PlacementBuildingCollisionReady>,
        ),
    >,
    lifecycle: Res<'w, WorldAssetLifecycleIndex>,
}

const REUSE_INDEX_BUDGET: usize = 128;

fn id_hash(id: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    hasher.finish()
}

/// 只遍历原活动批次自己的已知根，独立作者预览与其他场景永远不会被接管。
pub(super) fn index_active_roots(
    state: &mut ScenePlacementRuntimeState,
    status: &mut ScenePlacementRuntimeStatus,
    roots: &PlacementReuseInputs,
) -> bool {
    let Some(active) = state.active.as_ref() else {
        return true;
    };
    // 旧批次标记清理完成后才给复用根装新候选身份，避免旧清理游标移除新票据。
    if active.marker_cleanup_cursor < active.roots.len() {
        return false;
    }
    let staging = state.staging.as_mut().expect("候选容器已建立");
    let end = (staging.reuse.cursor + REUSE_INDEX_BUDGET).min(active.roots.len());
    for &entity in &active.roots[staging.reuse.cursor..end] {
        let Ok((instance, ..)) = roots.roots.get(entity) else {
            continue;
        };
        if !instance.is_active() {
            continue;
        }
        staging
            .reuse
            .index
            .entry(id_hash(instance.id()))
            .and_modify(|entry| *entry = None)
            .or_insert(Some(entity));
        // Hash碰撞或重复身份一律不复用该桶；后续仍走原完整候选，不会少生成对象。
    }
    if staging.reuse.cursor != end {
        status.advance_progress();
    }
    staging.reuse.cursor = end;
    end == active.roots.len()
}

/// 命中ID仅是线索，正式作者记录与水覆盖必须完全一致才有权沿用原Entity。
pub(super) fn reuse_placement(
    commands: &mut Commands,
    map: &SceneId,
    plan: &PendingPlacementPlan,
    staging: &mut PlacementStaging,
    active: Option<&PlacementActiveBatch>,
    roots: &PlacementReuseInputs,
) -> Option<(Entity, bool)> {
    let active = active?;
    // 地形根依赖本批次洞口/碰撞发布票据，仍由原完整地形候选处理；普通对象沿用运行状态。
    if plan.record.terrain_patch.is_some()
        || plan.record.terrain_binding.is_some()
        || (plan.record.building_preset_id.is_some()
            && active.building_preset_revision != staging.building_preset_revision)
    {
        return None;
    }
    let entity = staging
        .reuse
        .index
        .get(&id_hash(&plan.record.id))
        .copied()
        .flatten()?;
    let (instance, water, asset, collision_generation, collision_ready) =
        roots.roots.get(entity).ok()?;
    if !instance.matches_record(
        map,
        &plan.record,
        water.map_or(&[], DynamicWaterMeshOverrides::surfaces),
    ) {
        return None;
    }
    let carried = instance.carried_to(staging.gate.clone())?;
    let (carry, _) = *staging.reuse.carry.get_or_insert_with(|| {
        let carry = commands
            .spawn((
                Name::new("未变摆放运行实例"),
                Transform::IDENTITY,
                Visibility::Inherited,
                ChildOf(active.root), // 候选未采用时保留旧画面与原共享刚体，不出现隐藏或双重实例。
            ))
            .id();
        (carry, active.root)
    });
    commands.entity(entity).insert((
        carried,
        ChildOf(carry),
        PlacementPendingInstance,
        PlacementPendingRevision {
            generation: staging.generation,
            revision: staging.revision,
        },
    ));
    let ready = if let Some(asset) = asset {
        // 复用不是伪造World/水Ready。当前真实World epoch重投原联合结算；热更间隙继续等真实observer。
        if let Some(epoch) = roots.lifecycle.current_ready_epoch(entity, asset.0.id()) {
            commands
                .entity(entity)
                .insert(PlacementWorldInstanceReady(epoch));
        }
        false
    } else if instance.is_building() {
        collision_generation
            .zip(collision_ready)
            .is_some_and(|(generation, ready)| {
                generation.world_ready > 0 && ready.0 == generation.world_ready
            })
    } else {
        true
    };
    Some((entity, ready))
}
