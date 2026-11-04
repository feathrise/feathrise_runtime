//! 当前一次预览的显示租约；保存进入前的实际值，返回编辑后恢复，不写工程设置或作者文档。

use bevy::prelude::*;
use feathrise_scene::author_view::ShadingView;

/// UI保留真实按住状态，玩法只见屏蔽后的输入；下一帧交回InputPlugin处理真正的松开边沿。
#[derive(Resource, Default)]
pub(super) struct ToolbarPointerInput {
    pub saved: Option<ButtonInput<MouseButton>>,
    pub capturing: bool,
}

#[derive(Clone, Copy)]
pub(super) struct DisplayBaseline {
    pub collision: Option<bool>,
}

#[derive(Resource, Default)]
pub(super) struct PreviewToolbarState {
    pub baseline: Option<DisplayBaseline>,
    pub grid: bool,
    pub collision: bool,
    /// 只租用当前游戏相机的模型线框显示；关闭、返回及相机换代完整归还原组件与附件。
    pub mesh_camera: Option<(Entity, Option<ShadingView>)>,
    /// 按钮只产生可靠返回意图；真正模式仍由原 SetMode / ModeApplied 事务确认。
    pub return_requested: bool,
}
