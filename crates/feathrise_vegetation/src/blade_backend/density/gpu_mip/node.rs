//! 根RenderGraph中的相邻Mip生成与完整链回读复制系统。

use std::sync::atomic::Ordering; // 单作业CAS确保同一请求只录入一次GPU命令。

use bevy::prelude::Res; // 根图系统只读当前准备作业和固定管线资源。
use bevy::render::render_resource::{
    ComputePassDescriptor, Extent3d, Origin3d, PipelineCache, TexelCopyBufferInfo,
    TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect,
}; // 记录Compute通道和每级纹理到缓冲复制。
use bevy::render::renderer::RenderContext; // 复用当前根图命令编码器。

use super::super::gpu_compose::pipeline::VegetationGpuComposePipeline; // 正式基础级竞争在相邻Mip前执行。
use super::resources::{
    JOB_COPY_RECORDED, JOB_READY, JOB_RECORDING, VegetationGpuMipPipeline,
    VegetationGpuMipRenderState,
}; // 节点只消费完整READY作业并发布复制已录入状态。

/// 无内部可变状态的候选Mip根图系统。
pub(super) fn run_vegetation_gpu_mip(
    mut render_context: RenderContext, // 在当前帧根命令编码器录入一次作业。
    state: Option<Res<VegetationGpuMipRenderState>>, // RenderStartup或资源初始化前允许安全快返。
    pipeline_cache: Res<PipelineCache>, // 固定管线在Prepare阶段已确认就绪。
    pipeline_resource: Res<VegetationGpuMipPipeline>, // 取得相邻Mip缓存ID。
    compose_resource: Option<Res<VegetationGpuComposePipeline>>, // 只有正式基础级竞争作业才消费该管线。
) {
    let Some(state) = state else {
        return; // RenderStartup或资源初始化前安全快返。
    };
    let Some(job) = state.current.as_ref().cloned() else {
        return; // 稳定布局没有任何GPU命令或缓冲访问。
    };
    let pipeline = if job.dispatches.is_empty() {
        None // 1×1候选无需Compute，仍允许直接复制基础级。
    } else {
        let Some(pipeline) = pipeline_cache.get_compute_pipeline(pipeline_resource.pipeline) else {
            return; // 防御同帧缓存热重载，保持READY供下一帧重试。
        };
        Some(pipeline) // 非空链必须取得可执行管线后才改变作业状态。
    };
    let compose_pipeline = if job.compose.is_some() {
        let Some(compose_resource) = compose_resource.as_deref() else {
            return; // 正式竞争管线资源尚未建立时保持READY供下一帧重试。
        }; // 取得正式基础级竞争缓存ID。
        let Some(pipeline) = pipeline_cache.get_compute_pipeline(compose_resource.pipeline) else {
            return; // 防御同帧竞争管线热重载，保持READY供下一帧重试。
        };
        Some(pipeline) // 有提交块时必须取得可执行竞争管线。
    } else {
        None // 旧基础级上传烟雾或空层不执行竞争Compute。
    };
    if job
        .state
        .compare_exchange(
            JOB_READY,         // 只有从未录入的作业可以执行。
            JOB_RECORDING,     // 当前节点立即独占所有纹理和缓冲命令录入。
            Ordering::AcqRel,  // 获取Prepare发布的资源并发布独占状态。
            Ordering::Acquire, // 失败表示同作业已录入或映射。
        )
        .is_err()
    {
        return; // 同一提取请求在后续帧根图零开销快返。
    }
    if let Some(previous_texture) = &job.previous_texture {
        render_context.command_encoder().copy_texture_to_texture(
            TexelCopyTextureInfo {
                texture: previous_texture,  // 只读取上一已验证候选图。
                mip_level: 0,               // 局部更新只继承基础级。
                origin: Origin3d::ZERO,     // 完整同尺寸基础级从零原点复制。
                aspect: TextureAspect::All, // RGBA四通道保持同源八位值。
            },
            TexelCopyTextureInfo {
                texture: &job.texture,      // 写入与当前活动图分离的候选。
                mip_level: 0,               // 子Mip稍后由正式相邻级管线重新生成。
                origin: Origin3d::ZERO,     // 完整同尺寸基础级从零原点复制。
                aspect: TextureAspect::All, // 目标同为RGBA8Unorm。
            },
            Extent3d {
                width: job.identity.width,   // 几何已在Prepare阶段与旧图核对。
                height: job.identity.height, // 非正方形图集独立复制真实高度。
                depth_or_array_layers: 1,    // 每个无限层使用普通二维纹理。
            },
        );
    }
    if let (Some(dispatch), Some(pipeline)) = (&job.compose, compose_pipeline) {
        let mut pass =
            render_context
                .command_encoder()
                .begin_compute_pass(&ComputePassDescriptor {
                    label: Some("vegetation_gpu_competition_pass"), // GPU分析器中标识只在加载/换版执行的基础级竞争。
                    ..Default::default() // 本阶段不增加额外时间戳feature要求。
                });
        pass.set_pipeline(pipeline); // 使用正式Additive/Weighted/Priority统一入口。
        pass.set_bind_group(0, &dispatch.bind_group, &[]); // 当前目标层读取共享组输入并只写提交块Mip0。
        pass.dispatch_workgroups(
            dispatch.workgroups_x, // 每块固定覆盖64个X纹素。
            dispatch.workgroups_y, // 每块固定覆盖64个Y纹素。
            dispatch.workgroups_z, // 每个提交块一个Z切片。
        );
        let _keep_uniform_alive = &dispatch.uniform; // 明确保留uniform到竞争命令录入结束。
    } // 同一编码器保证后续Mip读取刚写完的候选基础级。
    if let Some(pipeline) = pipeline {
        for dispatch in &job.dispatches {
            // 父级 sampled view 与子级 storage view 虽然是不重叠Mip，但 wgpu 29 不会在同一
            // compute pass 的连续dispatch之间插入逐子资源布局屏障。每级独立pass让Vulkan
            // 在 SHADER_READ_ONLY_OPTIMAL 与 GENERAL 之间建立明确转换，同时仍只提交一次编码器。
            let mut pass =
                render_context
                    .command_encoder()
                    .begin_compute_pass(&ComputePassDescriptor {
                        label: Some("vegetation_gpu_mip_level_pass"), // 每一级形成可验证的相邻Mip同步边界。
                        ..Default::default() // P5.2b2a不引入额外时间戳feature要求。
                    });
            pass.set_pipeline(pipeline); // 全部相邻级共享同一正式WGSL入口。
            pass.set_bind_group(0, &dispatch.bind_group, &[]); // 当前绑定只读父级并只写相邻子级。
            pass.dispatch_workgroups(dispatch.workgroups_x, dispatch.workgroups_y, 1); // 二维纹理Z固定一。
            let _keep_uniform_alive = &dispatch.uniform; // 明确保留uniform到当前作业GPU录入结束。
        }
    } // 全部级pass结束后，后续复制按同一命令编码器顺序看到完整写入结果。
    for level in job.readback_plan.levels() {
        render_context.command_encoder().copy_texture_to_buffer(
            TexelCopyTextureInfo {
                texture: &job.texture,      // 读取刚生成的候选完整链纹理。
                mip_level: level.mip_level, // 基础级和所有GPU子级逐项复制。
                origin: Origin3d::ZERO,     // 每级从左上角开始。
                aspect: TextureAspect::All, // RGBA颜色纹理复制全部通道。
            },
            TexelCopyBufferInfo {
                buffer: &job.readback, // 当前层全部级共享一次MAP_READ缓冲。
                layout: TexelCopyBufferLayout {
                    offset: level.offset,                        // 使用预先对齐的独立区域。
                    bytes_per_row: Some(level.padded_row_bytes), // 满足256字节行对齐。
                    rows_per_image: Some(level.height),          // 当前二维Mip完整有效行数。
                },
            },
            Extent3d {
                width: level.width,       // 复制当前级全部有效列。
                height: level.height,     // 复制当前级全部有效行。
                depth_or_array_layers: 1, // 单个二维纹理层。
            },
        );
    }
    job.state.store(JOB_COPY_RECORDED, Ordering::Release); // 下一帧ExtractSchedule只在全部命令录入后发起map_async。
    // 系统返回后根图继续执行草地Compute、相机驱动和正常叶片绘制。
}
