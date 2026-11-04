//! 版本化语义预览数据；坐标采用世界空间，颜色统一为线性RGBA，四元数统一按xyzw排列。

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorToolPreview {
    /// 完整替换当前工具集合；空列表明确清除旧显示，不表示沿用上一份。
    pub items: Vec<AuthorTool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "preview",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AuthorTool {
    Brush(BrushPreview),
    Transform(TransformPreview),
    Selection(SelectionPreview),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrushShape {
    Planar,
    Sphere,
    Box,
    Cylinder,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipmapBrushSurface {
    pub map_id: String,
    pub terrain_id: Option<String>,
    /// 稳定作者来源共同组成匹配门，禁止将旧落点绑定到新草稿或其他地形。
    pub height_path: String,
    pub authoring_height_revision: u64,
    // GPU实体、环带修订及缺页槽仅属于各自World；发送端本地核对，接收端从自己的真实快照取得。
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrushPreview {
    /// 原空间笔刷与常规跟随笔刷使用不同固定线宽/深度组，由实际渲染端统一选择。
    pub spatial: bool,
    /// 原作者工具的真实世界落点；平面抬高等显示偏移统一由接收端生成。
    pub center: [f32; 3],
    pub rotation: [f32; 4],
    pub radius: f32,
    /// 已钳制到0..1的强度比例；没有强度圈的工具为None。
    pub strength: Option<f32>,
    pub shape: BrushShape,
    pub half_height: f32,
    pub flatten: bool,
    pub color: [f32; 4],
    pub clipmap: Option<ClipmapBrushSurface>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TransformPreview {
    Move {
        /// 空间笔刷手柄保留原细线、地下可见及0.10箭头尖比例。
        spatial: bool,
        origin: [f32; 3],
        length: f32,
        axes: u8,
        active_axes: u8,
        plane: bool,
        plane_active: bool,
        plane_cross: bool,
        plane_color: [f32; 4],
        color_active: [f32; 4],
        axis_colors: [[f32; 4]; 3],
        anchor: Option<[f32; 3]>,
        marker_radius: Option<f32>,
    },
    Rotate {
        origin: [f32; 3],
        radius: f32,
        yaw_degrees: Option<f32>,
        before_yaw_degrees: Option<f32>,
        color: [f32; 4],
    },
    Scale {
        origin: [f32; 3],
        right: [f32; 3],
        up: [f32; 3],
        half_size: f32,
        color: [f32; 4],
        cube: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionPreview {
    /// 由来源对象的稳定路径附带身份；运行实体句柄或跨进程Entity不得进入此字段。
    pub target: String,
    pub center: [f32; 3],
    /// 三个世界半轴保留旋转、非均匀缩放、镜像与剪切；接收端只生成8角12边。
    pub half_axes: [[f32; 3]; 3],
    pub color: [f32; 4],
}
