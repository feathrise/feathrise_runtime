//! 相机临时水下外观所有权：仅备份实际控制的两项白平衡和距离雾，保留玩家曝光设置。
use bevy::prelude::*;

#[derive(Component)]
pub(in super::super) struct UnderwaterCameraOverride {
    pub fog_before: Option<DistanceFog>,
    pub fog_applied: DistanceFog,
    pub balance_before: Vec2,
    pub balance_applied: Vec2,
}
