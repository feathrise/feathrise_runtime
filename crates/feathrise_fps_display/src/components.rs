use bevy::prelude::*;

use crate::resources::{ResourceMonitorGroup, ResourceMonitorPage}; // 顶层组、二级按钮和正文共享唯一身份。

#[derive(Component)]
pub(super) struct FpsText;

#[derive(Component)]
pub(super) struct ResourceMonitorPanel;

/// F3 顶层功能组按钮。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ResourceMonitorGroupButton {
    pub group: ResourceMonitorGroup, // 点击后切换组并打开该组默认叶子页。
}

/// F3 二级叶子分页按钮。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ResourceMonitorTabButton {
    pub page: ResourceMonitorPage, // 点击后写入资源监控当前页并触发立即刷新。
}

/// F3 二级页签行。
#[derive(Component)]
pub(super) struct ResourceMonitorSubTabRow; // 总览与普通草组使用 Display::None 完全退出布局。

/// F3单个分页的正文文字。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ResourceMonitorPageText {
    pub page: ResourceMonitorPage, // 只有与状态匹配的正文参与布局并接收本轮报告写回。
}
