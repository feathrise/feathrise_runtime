//! 实例运行状态按上下文实体保存；源图身份、作者路径由地图领域另行管理。

use bevy::prelude::*;
use std::collections::BTreeMap;

/// 完整三轴姿态与正缩放的源坐标桥；相机/玩家输入逆变换，查询与显示结果正变换。
/// 作为来源实体组件时保存本次采用的完整世界姿态，叶片跟随在Transform传播前即可读取，避免借用上一帧GlobalTransform。
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct MapSourceFrame {
    pub source_to_world: Mat4,
    pub world_to_source: Mat4,
}

impl Default for MapSourceFrame {
    fn default() -> Self {
        Self { source_to_world: Mat4::IDENTITY, world_to_source: Mat4::IDENTITY }
    }
}

impl MapSourceFrame {
    pub fn from_transform(transform: Transform) -> Self {
        let source_to_world = transform.to_matrix();
        Self { source_to_world, world_to_source: source_to_world.inverse() }
    }
    pub fn to_local(&self, world: Vec3) -> Vec3 {
        self.world_to_source.transform_point3(world)
    }
    pub fn to_world(&self, local: Vec3) -> Vec3 {
        self.source_to_world.transform_point3(local)
    }
}

/// 有界域调度的明确参数；None保留原父地图的资源和坐标行为。
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct MapRuntimeScope {
    pub instance: Option<Entity>,
    pub frame: MapSourceFrame,
}

/// 每个实例拥有独立任务、票据与采用值；仅不可变源资产句柄可以共享。
#[derive(Resource)]
pub struct InstanceResourceTable<T: Resource> {
    pub values: BTreeMap<Entity, T>,
}

impl<T: Resource> Default for InstanceResourceTable<T> {
    fn default() -> Self { Self { values: BTreeMap::new() } }
}
