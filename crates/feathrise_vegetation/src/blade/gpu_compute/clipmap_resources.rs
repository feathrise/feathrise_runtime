//! 固定尺寸分页高度绑定资源；GLB 草窗使用合法空绑定而不启用分页采样。
use bevy::prelude::*; // 固定 Vec4 uniform 与 Resource。
use bevy::render::render_resource::{ShaderType, TextureView, UniformBuffer}; // GPU 轻量绑定资源。

#[derive(Clone, Copy, Default, ShaderType)] // 与 WGSL 五个参数vec4、缺页和十六环带严格对齐。
pub(super) struct ClipmapUniform {
    pub stitches: crate::components::GpuBladeClipmapStitches,
    pub holes: [Vec4; 32],
    pub hole_info: Vec4,
    // 不包含任何随场景格数增长的数组。
    pub scene_bounds: Vec4,           // xy/zw 保存有限世界边界。
    pub grid: Vec4,                 // xy 格距，zw 作者逻辑单元数。
    pub options: Vec4,              // x 最高页级，y 启用，z 环带数，w 首环几何Mip。
    pub view: Vec4,                 // xy 当前已采用最细中心，zw 保留。
    pub heights: Vec4,              // xy 保守高度包围，zw 保留。
    pub missing_pages: [UVec4; 16], // 每Mip固定四个待换入页；一次vec4查表即可拒绝局部缺页候选。
    pub bands: [UVec4; 16],         // xy内半径、zw外半径；相邻槽Mip严格递增一。
} // 完成固定5344字节采样参数，内存不随场景面积增长。

#[derive(Resource)] // 每个渲染世界只创建一次兼容绑定。
pub(super) struct ClipmapFallback {
    // 未启用分页高度的 GLB 路径也满足统一布局。
    pub atlas: TextureView,      // 一层二维数组的合法零纹理视图。
    pub page_table: TextureView, // 一像素 RGBA32Uint 合法零页表。
    pub uniform: UniformBuffer<ClipmapUniform>, // y 启用标记为零的固定 uniform。
} // 完成固定大小兼容资源。
