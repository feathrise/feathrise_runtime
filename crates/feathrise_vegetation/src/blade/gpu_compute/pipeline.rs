//! GPU草地分类压缩与间接参数收尾计算管线创建。

use std::borrow::Cow; // 为两个WGSL入口点提供无需分配的静态名称。
use std::num::NonZeroU64; // 显式声明storage binding最小字节，提前发现状态布局错误。

use bevy::prelude::*; // Commands和Res基础类型。
use bevy::render::render_resource::{
    // 创建布局并向Bevy异步管线缓存排队。
    BindGroupLayoutDescriptor, // 计算group0声明式布局。
    BindGroupLayoutEntries,    // 连续绑定号辅助构造器。
    ComputePipelineDescriptor, // WGSL计算管线描述符。
    PipelineCache,             // Bevy异步计算管线缓存。
    ShaderStages,              // 全部九项绑定只在计算阶段可见。
    TextureSampleType,         // 高度和密度纹理均为不可过滤float读取。
    binding_types::{
        // uniform、storage和纹理绑定类型辅助函数。
        storage_buffer_sized, // 输出实例与状态都是运行时数组或原子结构。
        texture_2d,           // 两张地表输入均为二维纹理。
        texture_2d_array,     // 分页高度图集使用数组视图。
        uniform_buffer,       // 复用GpuBladePatch动态uniform。
    },
};

use crate::components::GpuBladePatch; // 计算shader group0 binding0的288字节动态uniform类型。
use crate::constants::{
    // 最小绑定字节统一来源。
    GPU_GRASS_COMPACT_INSTANCE_BYTES, // 输出数组至少容纳一个紧凑实例。
    GPU_GRASS_DEPTH_BUCKET_STATE_BYTES, // binding8运行时数组至少容纳一个三十二字节桶状态。
    GPU_GRASS_DRAW_STATE_BYTES,       // 状态缓冲固定八十字节。
    GPU_GRASS_TILE_DISPATCH_BYTES,    // GPU生成的间接派发状态固定二十字节。
    GPU_GRASS_TILE_INDEX_BYTES,       // 活跃瓦片列表至少容纳一个u32编号。
};
use crate::resources::GpuBladeComputePipeline; // 保存布局和两个缓存管线ID供准备与RenderGraph节点使用。
use crate::shaders::GPU_GRASS_COMPUTE_SHADER_HANDLE; // crate 内部固定句柄消除宿主资产目录耦合。

use super::culling::GpuBladeViewCullingUniform; // binding5由全部内容窗口共享唯一玩法相机的六平面视锥。

/// 无草的空工程/新地形不应抢占首屏Shader编译；首次真实草窗出现后仅初始化一次。
pub(super) fn needs_gpu_grass_compute_pipeline(
    pipeline: Option<Res<GpuBladeComputePipeline>>,
    grass_windows: Query<(), With<GpuBladePatch>>,
) -> bool {
    pipeline.is_none() && !grass_windows.is_empty()
}

/// 渲染设备就绪且有真实草窗需求后，创建布局并异步排队四个入口点。
pub(super) fn init_gpu_grass_compute_pipeline(
    mut commands: Commands,             // 向RenderApp插入计算管线资源。
    pipeline_cache: Res<PipelineCache>, // 向Bevy异步管线缓存提交描述符。
) {
    let bind_group_layout = BindGroupLayoutDescriptor::new(
        "gpu_grass_compute_layout", // 图形调试器中的稳定布局名称。
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE, // 全部输入输出只由计算节点访问。
            (
                uniform_buffer::<GpuBladePatch>(true), // binding0以动态offset选择近中远草窗参数。
                texture_2d(TextureSampleType::Float { filterable: false }), // binding1读取RGBA16高度单元。
                texture_2d(TextureSampleType::Float { filterable: false }), // binding2读取RGBA8密度概率。
                storage_buffer_sized(false, NonZeroU64::new(GPU_GRASS_COMPACT_INSTANCE_BYTES)), // binding3写二十字节实例数组。
                storage_buffer_sized(false, NonZeroU64::new(GPU_GRASS_DRAW_STATE_BYTES)), // binding4写间接参数与统计状态。
                uniform_buffer::<GpuBladeViewCullingUniform>(false), // binding5只上传一次共享六平面，不使用动态offset。
                storage_buffer_sized(false, NonZeroU64::new(GPU_GRASS_TILE_INDEX_BYTES)), // binding6写后读活跃瓦片编号列表。
                storage_buffer_sized(false, NonZeroU64::new(GPU_GRASS_TILE_DISPATCH_BYTES)), // binding7生成细阶段间接派发参数。
                storage_buffer_sized(false, NonZeroU64::new(GPU_GRASS_DEPTH_BUCKET_STATE_BYTES)), // binding8写八个径向桶间接参数与预留状态。
                uniform_buffer::<super::clipmap_resources::ClipmapUniform>(false), // binding9 独立分页采样参数。
                texture_2d_array(TextureSampleType::Float { filterable: false }), // binding10 共享 R16 高度页。
                texture_2d(TextureSampleType::Uint), // binding11 共享紧凑整数页表。
                texture_2d(TextureSampleType::Float { filterable: false }), // binding12 三维作者样本，旧草窗绑定零纹理。
            ),
        ),
    ); // 完成与gpu_grass_compute.wgsl单个group0完全一致的布局。
    let shader = GPU_GRASS_COMPUTE_SHADER_HANDLE.clone(); // 四个入口点共享已经由插件注册的 crate 内置源码。
    let cull_tiles_pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("gpu_grass_cull_tiles".into()), // 图形调试器中标识每六十四瓦片一次的粗剔除阶段。
        layout: vec![bind_group_layout.clone()],    // 与后续阶段共享当前草窗全部资源。
        shader: shader.clone(), // 复用同一WGSL以保持瓦片坐标与候选坐标单一实现。
        entry_point: Some(Cow::Borrowed("cull_tiles")), // 只执行常驻裁剪AABB和保守视锥球测试。
        ..default()             // 不启用额外shader_defs或push constants。
    }); // 排队GPU瓦片粗剔除管线。
    let finalize_tiles_pipeline =
        pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("gpu_grass_finalize_tiles".into()), // 把活跃瓦片原子数转换为间接派发X。
            layout: vec![bind_group_layout.clone()], // 继续复用同一绑定组，避免阶段间重新描述资源。
            shader: shader.clone(),                  // 复用同一WGSL资产。
            entry_point: Some(Cow::Borrowed("finalize_tiles")), // 每草窗一个线程生成安全派发参数。
            ..default()                              // 保持默认计算管线配置。
        }); // 排队GPU瓦片派发收尾管线。
    let classify_pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("gpu_grass_classify_and_compact".into()), // 图形调试器中标识候选分类压缩阶段。
        layout: vec![bind_group_layout.clone()],              // 只使用一个计算绑定组。
        shader: shader.clone(),                               // 复用同一WGSL资产。
        entry_point: Some(Cow::Borrowed("classify_and_compact")), // 每候选一次执行的主入口。
        ..default() // 不启用额外shader_defs或push constants。
    }); // 排队候选分类压缩管线。
    let finalize_pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("gpu_grass_finalize_indirect".into()), // 图形调试器中标识间接数量收尾阶段。
        layout: vec![bind_group_layout.clone()],           // 与分类阶段共享同一绑定组对象。
        shader,                                            // 最后一个管线取得WGSL句柄所有权。
        entry_point: Some(Cow::Borrowed("finalize_indirect")), // 每草窗仅一个线程执行的收尾入口。
        ..default()                                        // 保持默认计算管线配置。
    }); // 排队间接参数收尾管线。
    commands.insert_resource(GpuBladeComputePipeline {
        bind_group_layout,       // 准备系统据此创建真实设备绑定组。
        cull_tiles_pipeline,     // RenderGraph先生成当前相机真正可能可见的瓦片短列表。
        finalize_tiles_pipeline, // RenderGraph把短列表长度写成合法间接派发参数。
        classify_pipeline,       // RenderGraph取得已编译分类管线。
        finalize_pipeline,       // RenderGraph取得已编译收尾管线。
    }); // 完成GPU草地计算资源初始化。
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{ecs::system::RunSystemOnce, render::render_resource::CachedComputePipelineId};

    #[test]
    fn empty_project_defers_grass_compilation_until_first_render_window_and_never_requeues() {
        let mut world = World::new();
        assert!(!world.run_system_once(needs_gpu_grass_compute_pipeline).unwrap(),
            "空工程和无草的首个Clipmap不排队计算Shader");
        let grass = world.spawn(GpuBladePatch::square(Vec3::ZERO, 64., 65, 1)).id();
        assert!(world.run_system_once(needs_gpu_grass_compute_pipeline).unwrap(),
            "首次真实渲染草窗仍须启动原四阶段管线");
        // 此回归只验证排队所有权，不创建GPU设备或把无效ID当成可渲染管线。
        world.insert_resource(GpuBladeComputePipeline {
            bind_group_layout: BindGroupLayoutDescriptor::new("test_grass_layout", &[]),
            cull_tiles_pipeline: CachedComputePipelineId::INVALID,
            finalize_tiles_pipeline: CachedComputePipelineId::INVALID,
            classify_pipeline: CachedComputePipelineId::INVALID,
            finalize_pipeline: CachedComputePipelineId::INVALID,
        });
        assert!(!world.run_system_once(needs_gpu_grass_compute_pipeline).unwrap());
        world.despawn(grass);
        world.spawn(GpuBladePatch::square(Vec3::ZERO, 64., 65, 2));
        assert!(!world.run_system_once(needs_gpu_grass_compute_pipeline).unwrap(),
            "重建草窗复用本进程的管线，不能重排编译");
    }
}
