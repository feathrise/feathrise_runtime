//! 当前脚下水体身份。
use bevy::prelude::*; // 小型资源类型。
#[derive(Resource, Default)] // 当前帧由既有水体遍历选择唯一脚下水体。
pub(in super::super) struct SplashWaterTarget(pub(in super::super) Option<Entity>); // 切图无水时自动清空。
