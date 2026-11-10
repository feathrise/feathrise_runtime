//! 初始化分页高度的固定空绑定，并把已采用快照编码成小 uniform。
use super::clipmap_resources::{ClipmapFallback, ClipmapUniform};
use crate::components::GpuBladeClipmapHeight; // 消费无场景依赖的公共快照。
use bevy::prelude::*; // ECS 初始化与 Vec4。
use bevy::render::render_resource::*; // 固定纹理描述和 uniform 缓冲。
use bevy::render::renderer::{RenderDevice, RenderQueue}; // 只在初始化建立兼容 GPU 资源。 // 独立资源及 ABI 类型。

pub(super) fn init_clipmap_fallback(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    // 初始化一次固定空绑定。
    let create = |format, dimension| {
        // 两张一像素纹理仅用于未启用分支的合法绑定。
        device
            .create_texture(&TextureDescriptor {
                // WebGPU 保证未写入资源初始化为零。
                label: Some("blade_clipmap_disabled_texture"), // 标记兼容路径资源。
                size: Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                }, // 固定最小分配。
                mip_level_count: 1,                            // 分页高度不依赖硬件 mip。
                sample_count: 1,                               // Compute 只读单样本纹理。
                dimension: TextureDimension::D2,               // 数组通过视图维度声明。
                format,                                        // 浮点数组或整数页表。
                usage: TextureUsages::TEXTURE_BINDING,         // 不需要 CPU 像素上传。
                view_formats: &[],                             // 不使用额外格式重解释。
            })
            .create_view(&TextureViewDescriptor {
                dimension: Some(dimension),
                ..default()
            }) // 显式数组视图支持单层页。
    }; // 完成固定兼容视图生成器。
    let mut uniform = UniformBuffer::from(ClipmapUniform::default()); // 禁用位保持零。
    uniform.write_buffer(&device, &queue); // 上传一次固定 336 字节。
    commands.insert_resource(ClipmapFallback {
        // 所有旧场景窗口共享同一兼容资源。
        atlas: create(TextureFormat::R32Float, TextureViewDimension::D2Array), // 无需额外设备特性。
        page_table: create(TextureFormat::Rgba32Uint, TextureViewDimension::D2), // 匹配紧凑整数页表类型。
        uniform, // 兼容路径永远不读取上述纹理。
    }); // 完成初始化。
} // 完成固定资源初始化系统。

pub(super) fn encode_clipmap(height: &GpuBladeClipmapHeight) -> ClipmapUniform {
    // 只编码参数，不复制高度页。
    let mut bands = [UVec4::ZERO; 16]; // 固定uniform槽不会随作者层数分配。
    let band_count = usize::try_from(height.band_count)
        .unwrap_or(usize::MAX)
        .min(height.bands.len());
    for (encoded, band) in bands.iter_mut().zip(height.bands.iter()).take(band_count) {
        *encoded = UVec4::new(
            band.inner_half_cells.x,
            band.inner_half_cells.y,
            band.outer_half_cells.x,
            band.outer_half_cells.y,
        ); // Mip由首环指数加槽序号恢复，编译计划保证相邻严格2:1。
    }
    let first_mip = height
        .bands
        .first()
        .filter(|_| band_count > 0)
        .map_or(0, |band| band.geometry_mip);
    ClipmapUniform {
        stitches: *height.stitches,
        holes: height.holes,
        hole_info: Vec4::new(height.hole_count.min(32) as f32, 0.0, 0.0, 0.0),
        // 一次性打包完整已采用快照。
        scene_bounds: Vec4::new(
            height.world_min.x,
            height.world_min.y,
            height.world_max.x,
            height.world_max.y,
        ), // 与地形矩形相同。
        grid: Vec4::new(
            height.cell_size.x,
            height.cell_size.y,
            height.logical_cells.x as f32,
            height.logical_cells.y as f32,
        ), // 保留各轴格距。
        options: Vec4::new(
            height.max_mip as f32,
            1.0,
            band_count as f32,
            first_mip as f32,
        ), // 不引用草叶 LOD。
        view: height.fine_center.extend(0.0).extend(0.0), // 只使用已采用视图中心。
        missing_pages: height.missing_pages, // 只传固定大小待页掩码，不复制或扫描全图页表。
        bands,                               // 可变矩形环带与正式组合Mesh同代采用。
        heights: Vec4::new(height.height_min, height.height_max, 0.0, 0.0), // 包围和高度修订同步。
    } // 返回固定大小值。
} // 完成纯参数转换。
