//! 正式基础级GPU竞争缓冲、旧图复制源和目标绑定组准备。

use std::sync::Arc; // 同一票据同组输入通过Arc身份复用GPU缓冲。

use bevy::prelude::UVec2; // 当前候选基础级尺寸进入竞争uniform。
use bevy::render::render_asset::RenderAssets; // 查找候选与上一有效GpuImage。
use bevy::render::render_resource::{
    BindGroupEntries, BufferInitDescriptor, BufferUsages, CachedPipelineState, PipelineCache,
    Texture, TextureFormat, TextureViewDescriptor, UniformBuffer,
}; // 创建只读存储缓冲、Mip0写视图和当前目标uniform。
use bevy::render::renderer::{RenderDevice, RenderQueue}; // 分配并上传竞争冷路径资源。
use bevy::render::texture::GpuImage; // 核对上一有效GPU图与候选几何完全相同。
use bevy::shader::ShaderCacheError; // 区分可重试Shader等待和必须拒绝的编译错误。

use crate::blade_backend::density::gpu_compose::{
    GPU_COMPETITION_TILE_WORKGROUPS, GpuCompetitionUniform, pipeline::VegetationGpuComposePipeline,
}; // Prepare与正式WGSL共享模式、尺寸和工作组ABI。
use crate::blade_backend::density::resources::{VegetationGpuBaseJob, VegetationGpuMipJob}; // 基础级来源明确区分旧Mip冒烟和正式GPU竞争。

use super::super::resources::{
    VegetationGpuCompetitionBuffers, VegetationGpuComposeDispatch, VegetationGpuMipRenderState,
}; // 渲染世界复用大输入缓冲并把目标调度交给根图。

/// 基础级竞争准备可能等待资源、成功或明确失败。
pub(super) enum GpuComposePrepareOutcome {
    Waiting, // Shader、候选或上一有效图片尚未进入渲染资产表。
    Ready {
        previous_texture: Option<Texture>, // 局部提交在竞争前复制上一基础级。
        dispatch: Option<VegetationGpuComposeDispatch>, // 空块作业合法地不执行Compute。
    },
    Failed(String), // 配置、资源、设备限制或管线硬错误。
}

/// 为当前票据层准备基础级GPU来源。
#[allow(clippy::too_many_arguments)] // 七项均为RenderApp准备阶段的正交只读或缓存资源。
pub(super) fn prepare_gpu_compose(
    job: &VegetationGpuMipJob,       // 当前票据、版本、层和候选图片身份。
    images: &RenderAssets<GpuImage>, // 已完成PrepareAssets的GPU图片表。
    pipeline: &VegetationGpuComposePipeline, // 正式基础级竞争固定管线。
    pipeline_cache: &PipelineCache,  // 查询编译状态并取得真实绑定布局。
    render_device: &RenderDevice,    // 创建存储缓冲、uniform与绑定组。
    render_queue: &RenderQueue,      // 上传当前目标层uniform。
    state: &mut VegetationGpuMipRenderState, // 跨同组串行层复用大输入缓冲。
) -> GpuComposePrepareOutcome {
    let VegetationGpuBaseJob::Competition {
        input,              // 正式未决组输入。
        target_layer_index, // 当前目标层稳定索引。
        previous_image,     // 局部更新可选旧图。
    } = &job.base
    else {
        state.competition = None; // 旧P5.2b2a基础级上传烟雾不保留正式竞争缓冲。
        return GpuComposePrepareOutcome::Ready {
            previous_texture: None, // CPU基础级已经随候选Image上传。
            dispatch: None,         // 根图直接进入相邻Mip。
        };
    };
    if input.width != job.width || input.height != job.height {
        return GpuComposePrepareOutcome::Failed(format!(
            "competition input {}x{} does not match candidate {}x{}",
            input.width, input.height, job.width, job.height
        )); // 错图集输入不得写入看似合法的候选纹理。
    }
    let layer_count = match u32::try_from(input.layer_ids.len()) {
        Ok(layer_count) if layer_count > 0 => layer_count, // 受检组至少包含当前目标层。
        Ok(_) => {
            return GpuComposePrepareOutcome::Failed(
                "competition input contains no layers".to_owned(),
            );
        }
        Err(_) => {
            return GpuComposePrepareOutcome::Failed(format!(
                "competition layer count {} overflowed",
                input.layer_ids.len()
            ));
        }
    };
    if *target_layer_index >= layer_count {
        return GpuComposePrepareOutcome::Failed(format!(
            "competition target layer index {} exceeds layer count {}",
            target_layer_index, layer_count
        )); // WGSL不得越界读取目标层描述。
    }
    let tile_count = match input.tile_count() {
        Ok(tile_count) => tile_count, // Z工作组与稳定提交块一一对应。
        Err(reason) => return GpuComposePrepareOutcome::Failed(reason),
    };
    let previous_texture = if let Some(previous_image) = previous_image {
        let Some(previous) = images.get(previous_image) else {
            return GpuComposePrepareOutcome::Waiting; // 上一有效主世界资产尚未进入当前RenderApp时下一帧重试。
        };
        if previous.texture_descriptor.format != TextureFormat::Rgba8Unorm
            || previous.texture_descriptor.size.width != job.width
            || previous.texture_descriptor.size.height != job.height
            || previous.texture_descriptor.size.depth_or_array_layers != 1
            || previous.texture_descriptor.mip_level_count != job.mip_level_count
        {
            return GpuComposePrepareOutcome::Failed(format!(
                "previous GPU atlas format/extent/mips do not match candidate {}x{} with {} levels",
                job.width, job.height, job.mip_level_count
            )); // 范围变化不得误走局部基础级复制。
        }
        Some(previous.texture.clone()) // 根图只复制Mip0，旧完整链继续保持只读。
    } else {
        None // 整图加载从候选全零基础级开始。
    };
    if tile_count == 0 {
        return GpuComposePrepareOutcome::Ready {
            previous_texture, // 无脏块局部版本仍可完整复制上一基础级。
            dispatch: None,   // 空层零图不需要管线或存储缓冲。
        };
    }
    match pipeline_cache.get_compute_pipeline_state(pipeline.pipeline) {
        CachedPipelineState::Ok(_) => {} // 正式竞争WGSL可执行后才创建目标绑定。
        CachedPipelineState::Queued | CachedPipelineState::Creating(_) => {
            return GpuComposePrepareOutcome::Waiting;
        }
        CachedPipelineState::Err(
            ShaderCacheError::ShaderNotLoaded(_) | ShaderCacheError::ShaderImportNotYetAvailable,
        ) => {
            return GpuComposePrepareOutcome::Waiting; // 内部Shader资产下一帧可进入缓存。
        }
        CachedPipelineState::Err(error) => {
            return GpuComposePrepareOutcome::Failed(format!(
                "competition compute pipeline compilation failed: {error:?}"
            ));
        }
    }
    let device_limit = render_device.limits().max_storage_buffer_binding_size as usize; // 当前适配器可能低于CPU固定上限。
    for (label, length) in [
        ("layers", input.layer_bytes.len()),     // 稳定层描述缓冲。
        ("tiles", input.tile_bytes.len()),       // 提交块原点缓冲。
        ("samples", input.sample_bytes.len()),   // 未决实际与所有权需求缓冲。
        ("contexts", input.context_bytes.len()), // 选择值和足迹影响缓冲。
    ] {
        if length > device_limit {
            return GpuComposePrepareOutcome::Failed(format!(
                "competition {label} buffer {length} exceeds device storage binding limit {device_limit}"
            )); // 设备不支持当前固定计划时旧活动图片继续托底。
        }
    }
    let reuse_buffers = state
        .competition
        .as_ref()
        .is_some_and(|buffers| buffers.ticket == job.ticket && Arc::ptr_eq(&buffers.input, input)); // 同票据同组后续目标层不重复上传大输入。
    if !reuse_buffers {
        state.competition = Some(VegetationGpuCompetitionBuffers {
            ticket: job.ticket,   // 票据变化强制释放并重建输入。
            input: input.clone(), // CPU Arc保持到最后一个同组层完成。
            layers: create_storage_buffer(
                render_device,
                "vegetation_gpu_competition_layers",
                input.layer_bytes.as_ref(),
            ),
            tiles: create_storage_buffer(
                render_device,
                "vegetation_gpu_competition_tiles",
                input.tile_bytes.as_ref(),
            ),
            samples: create_storage_buffer(
                render_device,
                "vegetation_gpu_competition_samples",
                input.sample_bytes.as_ref(),
            ),
            contexts: create_storage_buffer(
                render_device,
                "vegetation_gpu_competition_contexts",
                input.context_bytes.as_ref(),
            ),
        }); // 四个缓冲全部创建后才替换上一组缓存。
    }
    let buffers = state
        .competition
        .as_ref()
        .expect("competition buffers were created or reused above"); // 当前票据Arc身份已核对。
    let Some(candidate) = images.get(&job.image) else {
        return GpuComposePrepareOutcome::Waiting; // 防御候选在调用者核对后被热重载移除。
    };
    let target_view = candidate.texture.create_view(&TextureViewDescriptor {
        label: Some("vegetation_gpu_competition_target_base"), // 图形调试器中标识正式Mip0写目标。
        base_mip_level: 0,                                     // 竞争只写基础级。
        mip_level_count: Some(1), // 禁止视图包含随后由另一管线写入的子Mip。
        ..Default::default()      // 保持RGBA8二维单层默认方面。
    });
    let mut uniform = UniformBuffer::from(GpuCompetitionUniform {
        atlas_size: UVec2::new(job.width, job.height), // 输出边界检查使用真实基础级尺寸。
        layer_count,                                   // 动态循环只扫描当前稳定组层数。
        target_layer_index: *target_layer_index,       // 当前串行层独立写出一张候选图。
        mode: input.mode,                              // Additive、Weighted或Priority稳定常量。
        tile_count,                                    // Z工作组与提交块数量一致。
        mode_value: input.mode_value,                  // 容量、阈值或零。
        padding: 0,                                    // 固定补齐字段不携带语义。
    });
    uniform.set_label(Some("vegetation_gpu_competition_uniform")); // GPU调试器中识别当前目标层参数。
    uniform.write_buffer(render_device, render_queue); // 根图执行前上传三十二字节。
    let Some(uniform_buffer) = uniform.buffer().cloned() else {
        return GpuComposePrepareOutcome::Failed(
            "failed to allocate competition uniform buffer".to_owned(),
        ); // 资源分配失败时整层候选不可采用。
    };
    let real_layout = pipeline_cache.get_bind_group_layout(&pipeline.bind_group_layout); // 当前设备解析后的唯一group0布局。
    let bind_group = render_device.create_bind_group(
        "vegetation_gpu_competition_bind_group", // 一个绑定组服务当前票据目标层。
        &real_layout,
        &BindGroupEntries::sequential((
            buffers.layers.as_entire_binding(),   // binding0层描述。
            buffers.tiles.as_entire_binding(),    // binding1提交块描述。
            buffers.samples.as_entire_binding(),  // binding2未决需求。
            buffers.contexts.as_entire_binding(), // binding3选择值与足迹影响。
            &target_view,                         // binding4只写候选Mip0。
            uniform_buffer.as_entire_binding(),   // binding5当前目标参数。
        )),
    );
    GpuComposePrepareOutcome::Ready {
        previous_texture, // 根图在Compute前先复制上一有效基础级。
        dispatch: Some(VegetationGpuComposeDispatch {
            bind_group,                                    // RenderGraph无需再次查找任何输入资源。
            uniform: uniform_buffer, // 显式延长uniform生命周期到命令录入结束。
            workgroups_x: GPU_COMPETITION_TILE_WORKGROUPS, // 覆盖块内64个X纹素。
            workgroups_y: GPU_COMPETITION_TILE_WORKGROUPS, // 覆盖块内64个Y纹素。
            workgroups_z: tile_count, // 每个提交块一个Z切片。
        }),
    }
}

/// 创建一次初始化完成的只读存储缓冲。
fn create_storage_buffer(
    render_device: &RenderDevice, // 当前RenderApp真实设备。
    label: &'static str,          // 图形调试器中区分四类正式输入。
    contents: &[u8],              // 编码器保证非空并满足元素步长。
) -> bevy::render::render_resource::Buffer {
    render_device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some(label),           // 保留稳定GPU调试标签。
        contents,                     // 一次上传不可变冷路径输入。
        usage: BufferUsages::STORAGE, // Shader只读且不需要复制或映射。
    })
}
