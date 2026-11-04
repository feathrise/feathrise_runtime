//! 表面天气共享GPU数据与上传顺序，所有地图和材质均不增加状态槽位。

use bevy::prelude::{Handle, Image, Resource, SystemSet, Vec4}; // 固定资源、完整遮罩句柄和宿主排序标签。
use bevy::render::render_resource::ShaderType; // 按WGSL四向量布局编码。

/// 由宿主解析天气和当前积水遮罩后一次性发布的64字节参数。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, ShaderType)] // 零值是无天气、无积水的安全回退。
pub struct SurfaceWeatherGpu {
    // 独立于项目数据层，便于环境crate复用。
    pub weather: Vec4,      // 湿润度、积水量、连续降雨量、暂停感知视觉秒。
    pub wind_quality: Vec4, // 世界风向X/Z、归一化风强、低0/中1/高2/极高3。
    pub bounds: Vec4,       // 当前完整积水图的世界最小X/Z与最大X/Z。
    pub options: Vec4,      // 积水图启用、作者潜力预览、雨纹距离米、雨纹密度倍率。
} // 完成固定GPU契约。

/// 主世界宿主必须在该PostUpdate上传阶段前完成完整状态发布。
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)] // 不反向依赖项目天气SystemSet。
pub struct SurfaceWeatherUpload; // 每帧仅有一个固定缓冲写侧。

/// 当前完整积水遮罩；宿主只能在图片与身份均验证后替换。
#[derive(Resource, Clone, Debug, Default, PartialEq)] // None复用Bevy一像素回退，不为旧图生成整图纹理。
pub struct SurfacePuddleMask {
    // 纹理变化与逐帧天气数据分离。
    pub image: Option<Handle<Image>>, // 强持有当前正式或作者完整预览图片。
} // 完成独立遮罩资源。
