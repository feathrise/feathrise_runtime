//! 固定池水滴弹道组件。
use bevy::prelude::*; // 世界向量与组件。
#[derive(Component)] // 每槽只有固定小型弹道状态。
pub(in super::super) struct SplashDrop {
    pub(super) origin: Vec3,
    pub(super) velocity: Vec3,
    pub(super) age: f32,
    pub(super) active: bool,
} // 位置以世界米计量。
