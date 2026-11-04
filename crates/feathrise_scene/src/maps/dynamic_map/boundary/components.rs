//! 动态地图区域边界运行时实体标记。

use bevy::prelude::*; // Component与实体标记派生。

/// 当前动态地图全部空气墙的原子替换根。
#[derive(Component, Clone, Copy, Debug, Default)]
pub(super) struct DynamicMapBoundaryRoot;

/// 一段由作者边界路径派生的静态碰撞盒；稳定路径和编号保存在实体Name中供物理调试查看。
#[derive(Component, Clone, Copy, Debug, Default)]
pub(super) struct DynamicMapBoundarySegment;
