//! 动态场景植被运行时实体标记。

use bevy::prelude::*;

/// 当前完整植被修订的唯一父根；热更只替换该根，不遍历其他世界实体。
#[derive(Component, Clone, Copy, Debug)]
pub(super) struct DynamicVegetationRoot;

/// 一个由玩法相机驱动、但候选位置保持世界格稳定的GPU叶片窗口。
#[derive(Component, Clone, Copy, Debug)]
pub(in crate::scenes::dynamic_scene) struct DynamicBladeWindow {
    /// 窗口跨过多少候选格后再整体吸附，避免逐帧滑动和植株洗牌。
    pub(in crate::scenes::dynamic_scene) snap_cells: u32,
}
