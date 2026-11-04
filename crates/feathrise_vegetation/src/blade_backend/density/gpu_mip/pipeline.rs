//! 正式相邻Mip计算管线的RenderStartup创建。

use std::borrow::Cow; // 固定WGSL入口点无需运行时分配。

use bevy::prelude::{Commands, Res}; // 向渲染世界插入管线资源并读取缓存。
use bevy::render::render_resource::{
    BindGroupLayoutDescriptor, BindGroupLayoutEntries, ComputePipelineDescriptor, PipelineCache,
    ShaderStages, StorageTextureAccess, TextureFormat, TextureSampleType,
    binding_types::{texture_2d, texture_storage_2d, uniform_buffer},
}; // 描述只读父Mip、存储写子Mip和尺寸uniform三个绑定。

use super::resources::VegetationGpuMipPipeline;
use super::{GPU_MIP_SHADER_HANDLE, GpuMipUniform}; // 管线使用crate内置正式WGSL与共享Rust布局。 // 保存声明式布局和异步缓存ID。

/// 渲染设备建立后排队唯一GPU Mip入口。
pub(super) fn init_vegetation_gpu_mip_pipeline(
    mut commands: Commands, // 插入后续Prepare和RenderGraph读取的管线资源。
    pipeline_cache: Res<PipelineCache>, // Bevy异步编译内部WGSL。
) {
    let bind_group_layout = BindGroupLayoutDescriptor::new(
        "vegetation_gpu_mip_layout", // 图形调试器中识别布局。
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE, // 三个绑定只在计算阶段可见。
            (
                texture_2d(TextureSampleType::Float { filterable: false }), // binding0只读当前父Mip。
                texture_storage_2d(TextureFormat::Rgba8Unorm, StorageTextureAccess::WriteOnly), // binding1只写相邻子Mip。
                uniform_buffer::<GpuMipUniform>(false), // binding2固定十六字节且不用动态偏移。
            ),
        ),
    ); // 与`vegetation_layout_mip.wgsl` group0逐项一致。
    let pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("vegetation_gpu_mip_downsample".into()), // GPU分析器中标识加载/换版作业。
        layout: vec![bind_group_layout.clone()],             // 每级复用同一真实设备布局。
        shader: GPU_MIP_SHADER_HANDLE.clone(),               // 使用随crate注册的唯一正式源码。
        entry_point: Some(Cow::Borrowed("downsample_density")), // 显式锁定稳定入口名。
        ..Default::default() // 不使用shader_defs、覆盖常量或push constants。
    });
    commands.insert_resource(VegetationGpuMipPipeline {
        bind_group_layout, // Prepare据此取得真实BindGroupLayout。
        pipeline,          // RenderGraph等待缓存明确就绪后才触碰候选。
    });
}
