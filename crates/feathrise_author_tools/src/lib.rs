//! 笔刷、变换手柄与选择框的纯语义合同；不依赖Bevy、作者World、操作系统或渲染资源。
//! 发送语义后由实际显示该场景的一端生成固定预算几何，不能退回任意线段镜像。

#![forbid(unsafe_code)]

mod types;
mod validation;

// 显示与原拾取数学共用固定参数，避免跨进程渲染手柄与作者命中区域各自漂移。
pub const AUTHOR_ROTATION_SEGMENTS: u32 = 64;
pub const AUTHOR_PLANE_INNER_FRACTION: f32 = 0.20;
pub const AUTHOR_PLANE_OUTER_FRACTION: f32 = 0.52;

pub use types::*;
pub use validation::{
    AuthorToolValidationError, MAX_AUTHOR_BRUSHES, MAX_AUTHOR_SELECTIONS, MAX_AUTHOR_TOOL_SEGMENTS,
    MAX_AUTHOR_TOOLS, MAX_AUTHOR_TRANSFORMS,
};
/// 圆刷共享轨迹间距；调用者负责提供同一坐标空间中的实际距离。
/// 后台散布消费完整路径，帧内绘制可另行限制工作量，不改变采样间距。
pub fn brush_path_dab_count(distance: f32, radius: f32) -> usize {
    let spacing = (radius * 0.35).max(0.001);
    (distance / spacing).ceil().max(1.0) as usize
}
