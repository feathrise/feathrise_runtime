//! 两种作者视口共用工具几何；协议只描述笔刷、手柄和选择范围，不传递离散线段。

mod brush;
mod transform;
mod selection;

pub use brush::{clipmap_brush, draw_brush};
pub use transform::draw_transform;
pub use selection::draw_selection;
use bevy::{gizmos::config::{GizmoConfig, GizmoConfigGroup}, prelude::*};
use feathrise_author_tools::{AuthorTool, AuthorToolPreview, TransformPreview};

/// 显示和拾取使用作者系统已经确认的同一姿态、尺寸；此处不参与作者事务。
pub fn draw<T: GizmoConfigGroup>(gizmos: &mut Gizmos<T>, preview: &AuthorToolPreview) {
    for item in &preview.items {
        match item {
            AuthorTool::Brush(brush) => draw_brush(gizmos, brush),
            AuthorTool::Transform(handle) => draw_transform(gizmos, handle),
            AuthorTool::Selection(selection) => draw_selection(gizmos, selection),
        }
    }
}

/// 固定空间笔刷沿用原地下可见的细线；跟随笔刷、手柄和选框沿用原普通作者线配置。
pub fn configure(config: &mut GizmoConfig, spatial: bool) {
    config.line.width = if spatial { 2.0 } else { 3.0 };
    config.line.perspective = false;
    config.depth_bias = if spatial { -1.0 } else { -0.02 };
}

pub fn draw_layer<T: GizmoConfigGroup>(gizmos: &mut Gizmos<T>, preview: &AuthorToolPreview, spatial: bool) {
    for item in &preview.items {
        match item {
            AuthorTool::Brush(brush) if brush.spatial == spatial => draw_brush(gizmos, brush),
            AuthorTool::Transform(handle) if matches!(handle, TransformPreview::Move { spatial: true, .. }) == spatial => draw_transform(gizmos, handle),
            AuthorTool::Selection(selection) if !spatial => draw_selection(gizmos, selection),
            _ => {}
        }
    }
}
