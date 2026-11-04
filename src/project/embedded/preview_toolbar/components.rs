//! 预览工具条的窗口相机、根节点与三个按钮身份；不导出给游戏或作者工具。

use bevy::prelude::*;

#[derive(Component)]
pub(super) struct ToolbarRoot;

#[derive(Component)]
pub(super) struct ToolbarCamera;

#[derive(Component)]
pub(super) struct ToolbarPointer;

/// 只有抓手接收拖动；按钮仍沿原点击入口，避免拖动时误触返回或显示开关。
#[derive(Component)]
pub(super) struct ToolbarDragHandle;

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolbarAction {
    Return,
    Grid,
    Collision,
}
