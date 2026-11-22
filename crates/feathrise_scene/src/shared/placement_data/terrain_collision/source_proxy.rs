//! 普通 GLB 独立代理的作者、摆放批次与外层子场景共同参与门。

use super::{
    AuthorCollisionSuppression, ScenePlacementInstance, ScenePlacementRuntimeState,
    SourceGlbPhysicsRoot, TerrainColliderProxy, TerrainPatchCollisionRequest,
    sync_proxy_participation,
};
use crate::{SubsceneActive, SubsceneInstance};
use bevy::prelude::*;
use feathrise_scene_core::residency::{
    InstanceResourceTable, ResidentSceneEntity, SceneInstanceDetached, SceneInstanceOwner,
};

pub(super) fn tag_source_proxy(world: &mut World, placement: Entity, proxy: Entity) {
    let owner = world.get::<SceneInstanceOwner>(placement).copied();
    let resident = world.get::<ResidentSceneEntity>(placement).copied()
        .or_else(|| owner.and_then(|owner| world.get::<ResidentSceneEntity>(owner.0).copied()));
    let mut entity = world.entity_mut(proxy);
    if let Some(owner) = owner { entity.insert(owner); }
    // 代理顶点按世界缩放烘焙。采集来源照旧指向真实子图，暖切则跟随实际地图的驻留票据。
    entity.insert((SceneInstanceDetached, SourceGlbPhysicsRoot));
    if let Some(resident) = resident { entity.insert(resident); }
}

pub(super) fn refresh_source_proxy(world: &mut World, root: Entity) {
    let Some(proxy) = world.get::<TerrainColliderProxy>(root).map(|proxy| proxy.0) else { return; };
    if world.get::<SourceGlbPhysicsRoot>(proxy).is_none() { return; }

    // 热更期间仍使用上一已采用代。只查当前地址和双层活动门，不拿候选代次撤销旧承托。
    let enabled = world.get::<ScenePlacementInstance>(root).is_some_and(ScenePlacementInstance::is_active)
        && world.get::<TerrainPatchCollisionRequest>(root).is_some()
        && world.get::<AuthorCollisionSuppression>(root).is_none()
        && world.get::<SceneInstanceOwner>(root).is_none_or(|owner| {
            world.get_entity(owner.0).is_ok() && world.get::<SubsceneInstance>(owner.0)
                .is_none_or(|instance| world.get::<SubsceneActive>(instance.batch).is_some())
        });
    sync_proxy_participation(world, proxy, enabled);
}

/// 外层独占切换复用已采用摆放的正式碰撞根表，只有各自 Source GLB 确认参与。
pub(crate) fn set_subscene_placement_collision_activity(world: &mut World, batch: Entity, instance: Entity) {
    if world.get::<SubsceneInstance>(instance).is_none_or(|source| source.batch != batch) { return; }
    let root_at = |world: &World, index: usize| {
        world.get_resource::<InstanceResourceTable<ScenePlacementRuntimeState>>()?.values
            .get(&instance)?.active.as_ref()?.primitive_collision_roots.get(index).copied()
    };
    let mut index = 0;
    while let Some(root) = root_at(world, index) {
        if world.get::<SceneInstanceOwner>(root) == Some(&SceneInstanceOwner(instance)) {
            refresh_source_proxy(world, root);
        }
        index += 1;
    }
}
