//! 子地图实体来源：MapId不变，实体只记录自己所属的独立上下文。

use bevy::prelude::*;

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapInstanceOwner(pub Entity);

/// 地形有独立源坐标，但所属地图的角色可以由其中任一地形承托。
#[derive(Component, Clone, Copy, Debug)]
pub struct MapTerrainOwner {
    pub map_instance: Option<Entity>, // None是父世界地图，Some是完整子地图实例。
}

/// 离屏预热仍归实例管理，但不挂到隐藏候选层级，才能进入材质专用化。
#[derive(Component)]
pub struct MapInstanceDetached;

#[derive(Resource, Default)]
pub struct MapDetachedRoots(pub Vec<Entity>); // 离屏根也参与有界退役，不能留在实例树外泄漏。
