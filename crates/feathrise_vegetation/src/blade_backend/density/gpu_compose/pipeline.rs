//! 正式基础级GPU竞争管线的RenderStartup创建。

use std::borrow::Cow; // 固定WGSL入口无需运行时字符串分配。

use bevy::prelude::{Commands, Res, Resource, UVec4, Vec2}; // 插入管线资源并声明存储数组最小元素类型。
use bevy::render::render_resource::{
    BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedComputePipelineId,
    ComputePipelineDescriptor, PipelineCache, ShaderStages, StorageTextureAccess, TextureFormat,
    binding_types::{storage_buffer_read_only, texture_storage_2d, uniform_buffer},
}; // 四个只读输入缓冲、基础级存储纹理和固定参数组成唯一group0。

use super::{GPU_COMPOSE_SHADER_HANDLE, GpuCompetitionUniform}; // 管线与运行时使用同一正式Shader和uniform ABI。

/// 正式基础级GPU竞争固定管线。
#[derive(Resource)]
pub(in crate::blade_backend::density) struct VegetationGpuComposePipeline {
    pub(in crate::blade_backend::density) bind_group_layout: BindGroupLayoutDescriptor, // Prepare取得真实设备布局并建立当前目标绑定组。
    pub(in crate::blade_backend::density) pipeline: CachedComputePipelineId, // 异步缓存中的`compose_density`入口。
}

/// 渲染设备建立后排队唯一正式基础级竞争入口。
pub(in crate::blade_backend::density) fn init_vegetation_gpu_compose_pipeline(
    mut commands: Commands, // 插入Prepare和RenderGraph共同读取的管线资源。
    pipeline_cache: Res<PipelineCache>, // 通过Bevy缓存异步编译内部WGSL。
) {
    let bind_group_layout = BindGroupLayoutDescriptor::new(
        "vegetation_gpu_compose_layout", // 图形调试器中识别正式竞争输入。
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE, // 六个绑定只在计算阶段可见。
            (
                storage_buffer_read_only::<UVec4>(false), // binding0稳定层描述数组。
                storage_buffer_read_only::<UVec4>(false), // binding1提交块输出原点与样本起点。
                storage_buffer_read_only::<Vec2>(false),  // binding2逐纹素逐层实际/所有权需求。
                storage_buffer_read_only::<Vec2>(false),  // binding3逐纹素选择值与足迹影响。
                texture_storage_2d(TextureFormat::Rgba8Unorm, StorageTextureAccess::WriteOnly), // binding4只写候选基础级。
                uniform_buffer::<GpuCompetitionUniform>(false), // binding5固定三十二字节参数。
            ),
        ),
    ); // 与`vegetation_layout_compose.wgsl` group0逐项一致。
    let pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("vegetation_gpu_compose_density".into()), // GPU分析器中标识加载/换版竞争作业。
        layout: vec![bind_group_layout.clone()],              // 一个组包含当前作业全部不可变输入。
        shader: GPU_COMPOSE_SHADER_HANDLE.clone(),            // 使用随crate注册的唯一正式源码。
        entry_point: Some(Cow::Borrowed("compose_density")),  // 显式锁定稳定入口名。
        ..Default::default() // 不使用shader_defs、覆盖常量或push constants。
    });
    commands.insert_resource(VegetationGpuComposePipeline {
        bind_group_layout, // Prepare据此取得真实BindGroupLayout。
        pipeline,          // RenderGraph只在缓存明确就绪后执行。
    });
}
