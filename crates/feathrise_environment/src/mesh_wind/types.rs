//! Mesh 风 CPU 参数与固定 GPU ABI 数据类型。
//! 【功能设计】全部 GPU 字段按 `vec4` 对齐，使 Bevy 0.18 与后续 0.19 迁移只需调整接线而不重排业务参数。

use bevy::prelude::*; // 引入Vec2、Vec4与基础数学值类型。
use bevy::render::render_resource::ShaderType; // 让固定Uniform布局按WGSL规则编码。

/// 单份共享材质一帧使用的可读 CPU 风参数。
#[derive(Clone, Copy, Debug, Default, PartialEq)] // 纯值结构支持精确历史推进和单元测试比较。
pub(super) struct TreeWindFrame {
    pub(super) direction: Vec2,       // 世界X/Z风向，写入GPU前已经安全归一化。
    pub(super) strength: f32,         // 树梢宏观局部位移上限，实际世界幅度还乘实例缩放。
    pub(super) noise_scale: f32,      // 世界X/Z到周期噪声UV的缩放。
    pub(super) scroll_speed: f32,     // 程序风场沿世界风向滚动的速度。
    pub(super) micro_strength: f32,   // 详细叶簇的局部高频位移幅度。
    pub(super) s_curve_speed: f32,    // 详细叶簇横向S曲线动画速度。
    pub(super) s_curve_strength: f32, // 详细叶簇横向S曲线位移幅度。
    pub(super) s_curve_frequency: f32, // 沿高度增加的S曲线相位滞后。
    pub(super) bop_speed: f32,        // 详细叶簇轻微上下起伏速度。
    pub(super) bop_strength: f32,     // 详细叶簇轻微上下起伏幅度。
} // 完成CPU帧参数定义。

/// 单帧 GPU 风参数固定占用三个 `vec4`，不把Rust结构体自然填充暴露给Shader。
#[repr(C)] // Rust物理布局与三个连续WGSL vec4保持一致。
#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)] // Bevy负责生成统一缓冲编码规则。
pub(super) struct TreeWindGpuFrame {
    pub(super) direction_strength_noise: Vec4, // xy为方向，z为宏观强度，w为世界噪声比例。
    pub(super) motion: Vec4,                   // x为滚动速度，y为微摆强度，z/w为S曲线速度与强度。
    pub(super) secondary: Vec4,                // x为S曲线频率，y/z为bop速度与强度，w保留为零。
} // 完成单帧GPU布局定义。

/// 当前帧、上一帧和共用树高边界组成固定128字节材质Uniform。
#[repr(C)] // 两个三vec4帧加两个边界vec4严格合计八个vec4。
#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)] // AsBindGroup直接把该值写入binding 100。
pub(super) struct TreeWindUniform {
    pub(super) current: TreeWindGpuFrame, // 主渲染、阴影和当前深度预通道使用的风参数。
    pub(super) previous: TreeWindGpuFrame, // 运动向量预通道重建上一帧顶点使用的风参数。
    pub(super) aabb_min: Vec4,            // xyz保存根部固定后的局部参考最小边界，w固定为零。
    pub(super) aabb_max: Vec4,            // xyz保存完整树高参考最大边界，w固定为零。
} // 完成固定128字节材质Uniform定义。

/// 材质创建时固定的顶点细节专用化键。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)] // 键只决定是否编译叶簇微摆分支。
pub struct TreeWindMaterialKey {
    pub(super) detailed: bool, // 高中档叶簇为真；树干和低档叶簇为假。
} // 完成材质专用化键定义。
