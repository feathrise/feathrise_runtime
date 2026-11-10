//! 子场景实体来源：SceneId不变，实体只记录自己所属的独立上下文。

use bevy::prelude::*;

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneInstanceOwner(pub Entity);

/// 地形有独立源坐标，但所属场景的角色可以由其中任一地形承托。
#[derive(Component, Clone, Copy, Debug)]
pub struct SceneTerrainOwner {
    pub scene_instance: Option<Entity>, // None是父世界场景，Some是完整子场景实例。
}

/// 离屏预热仍归实例管理，但不挂到隐藏候选层级，才能进入材质专用化。
#[derive(Component)]
pub struct SceneInstanceDetached;

#[derive(Resource, Default)]
pub struct SceneDetachedRoots(pub Vec<Entity>); // 离屏根也参与有界退役，不能留在实例树外泄漏。
