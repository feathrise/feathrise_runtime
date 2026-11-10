//! 动态场景地表唯一预热事务组件。

use bevy::prelude::*;

/// 防止同一真实地形并发创建多个候选材质。
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct DynamicSurfaceSourceWarmup;

/// 同网格离屏候选的场景代次和资源修订身份。
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct DynamicSurfaceWarmup {
    pub(super) source_entity: Entity,
    pub(super) generation: u32,
    pub(super) revision: u32,
    pub(super) observed_frames: u8,
}
