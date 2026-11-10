//! 候选GpuImage就绪后的逐Mip视图、uniform、绑定组与回读缓冲准备。

use std::sync::{
    Arc,              // RenderGraph、ExtractSchedule和映射回调共享不可变准备作业。
    atomic::AtomicU8, // 新作业从READY状态开始。
};

use bevy::ecs::system::SystemParam; // 把渲染资源组合成具名参数，避免长位置系统签名。
use bevy::prelude::{Res, ResMut}; // 读取提取请求和渲染资源并替换当前作业。
use bevy::render::render_asset::RenderAssets; // 从强图片句柄取得已上传GpuImage。
use bevy::render::render_resource::{
    BindGroupEntries, BufferDescriptor, BufferUsages, CachedPipelineState, PipelineCache,
    TextureFormat, TextureViewDescriptor, UniformBuffer,
}; // 创建每级绑定、尺寸uniform和动态MAP_READ缓冲。
use bevy::render::renderer::{RenderDevice, RenderQueue}; // 分配GPU资源并上传十六字节uniform。
use bevy::render::texture::GpuImage; // 访问候选纹理、格式、尺寸和完整Mip级数。
use bevy::shader::ShaderCacheError; // 区分可重试Shader等待和必须返回主世界的编译错误。

use super::super::gpu_compose::pipeline::VegetationGpuComposePipeline; // 正式基础级竞争管线先于相邻Mip准备。
use super::super::resources::{
    VegetationGpuMipOutcome, VegetationGpuMipRequest, VegetationGpuMipResult,
};
use super::build_gpu_mip_dispatches; // 运行时与P5.2b1真实GPU测试共享相邻级调度ABI。
use super::plan::GpuMipReadbackPlan; // 回读缓冲尺寸和全部复制区一次构造。
use super::resources::{
    JOB_READY, VegetationGpuMipDispatch, VegetationGpuMipPipeline, VegetationGpuMipPreparedJob,
    VegetationGpuMipRenderState, VegetationGpuMipShared,
}; // 当前作业、固定管线和回调完成队列。

mod compose; // 大正式输入缓冲、旧图复制源和Mip0目标绑定独立于相邻Mip视图准备。

use compose::{GpuComposePrepareOutcome, prepare_gpu_compose}; // 主入口只处理等待、成功或明确失败三态。

/// 候选Mip准备阶段的全部RenderApp资源。
#[derive(SystemParam)]
pub(super) struct VegetationGpuMipPrepareResources<'w> {
    request: Res<'w, VegetationGpuMipRequest>, // ExtractSchedule复制来的当前唯一请求。
    images: Res<'w, RenderAssets<GpuImage>>,   // PrepareAssets完成后解析候选GPU纹理。
    compose_pipeline: Res<'w, VegetationGpuComposePipeline>, // 正式基础级竞争布局和缓存管线ID。
    pipeline: Res<'w, VegetationGpuMipPipeline>, // 声明式布局和缓存管线ID。
    pipeline_cache: Res<'w, PipelineCache>,    // 查询编译状态并取得真实设备布局。
    render_device: Res<'w, RenderDevice>,      // 创建视图、uniform、绑定组和回读缓冲。
    render_queue: Res<'w, RenderQueue>,        // 在根图执行前上传每级尺寸uniform。
    shared: Res<'w, VegetationGpuMipShared>,   // 资源或管线硬错误直接发布版本化失败。
    state: ResMut<'w, VegetationGpuMipRenderState>, // 同请求只准备一次且保留当前Arc作业。
}

/// 在图片资产和固定Compute管线都就绪后准备一次新票据层作业。
pub(super) fn prepare_vegetation_gpu_mip_job(
    resources: VegetationGpuMipPrepareResources, // 具名资源参数组保持系统入口单一职责。
) {
    let VegetationGpuMipPrepareResources {
        request,          // 当前主世界提取请求。
        images,           // 已上传GPU图片表。
        compose_pipeline, // 正式基础级竞争管线描述。
        pipeline,         // 固定Mip管线描述。
        pipeline_cache,   // 异步编译状态与真实布局。
        render_device,    // GPU资源创建入口。
        render_queue,     // uniform上传队列。
        shared,           // 版本化完成队列。
        mut state,        // 当前渲染作业状态。
    } = resources; // 后续主体继续使用短语义名称。
    let Some(job) = request.job.as_ref() else {
        state.current = None; // 主世界稳定或取消后不保留根图可执行作业。
        state.competition = None; // 整批结束后释放共享大输入GPU缓冲。
        state.last_key = None; // 暂停场景后恢复的在途请求允许重新提交，不把已切走的回调当作该图已收到。
        return;
    };
    let key = (job.ticket, job.layer); // 同批次不同层必须分别处理。
    if state.last_key == Some(key) {
        return; // 已准备、映射或失败的同一层请求不得重复生成和回读。
    }
    match pipeline_cache.get_compute_pipeline_state(pipeline.pipeline) {
        CachedPipelineState::Ok(_) => {} // 固定WGSL已经可执行，允许建立候选作业。
        CachedPipelineState::Queued | CachedPipelineState::Creating(_) => {
            return; // 首次异步编译等待期间不触碰候选纹理。
        }
        CachedPipelineState::Err(
            ShaderCacheError::ShaderNotLoaded(_) | ShaderCacheError::ShaderImportNotYetAvailable,
        ) => {
            return; // 内部Shader资产尚未进入缓存时下一帧自动重试。
        }
        CachedPipelineState::Err(error) => {
            publish_prepare_failure(
                job,
                format!("mip compute pipeline compilation failed: {error:?}"),
                &shared,
                &mut state,
            ); // 编译硬错误不得等到主世界超时才暴露。
            return;
        }
    }
    let Some(gpu_image) = images.get(&job.image) else {
        return; // Image提取或上传尚未完成时保留旧活动布局并下一帧重试。
    };
    if gpu_image.texture_descriptor.format != TextureFormat::Rgba8Unorm {
        publish_prepare_failure(
            job,
            format!(
                "candidate format {:?} is not Rgba8Unorm",
                gpu_image.texture_descriptor.format
            ),
            &shared,
            &mut state,
        ); // 正式WGSL存储纹理格式不允许隐式重解释。
        return;
    }
    if gpu_image.texture_descriptor.size.width != job.width
        || gpu_image.texture_descriptor.size.height != job.height
        || gpu_image.texture_descriptor.size.depth_or_array_layers != 1
    {
        publish_prepare_failure(
            job,
            format!(
                "candidate GPU extent {}x{}x{} does not match requested {}x{}x1",
                gpu_image.texture_descriptor.size.width,
                gpu_image.texture_descriptor.size.height,
                gpu_image.texture_descriptor.size.depth_or_array_layers,
                job.width,
                job.height
            ),
            &shared,
            &mut state,
        ); // 错尺寸纹理不得产生可比较但空间错位的结果。
        return;
    }
    if gpu_image.texture_descriptor.mip_level_count != job.mip_level_count {
        publish_prepare_failure(
            job,
            format!(
                "candidate GPU mip levels {} do not match requested {}",
                gpu_image.texture_descriptor.mip_level_count, job.mip_level_count
            ),
            &shared,
            &mut state,
        ); // 缺级或多级纹理不能满足完整链门禁。
        return;
    }
    let (previous_texture, compose) = match prepare_gpu_compose(
        job,               // 当前票据、版本、层和基础级来源。
        &images,           // 查询候选与可选上一有效GpuImage。
        &compose_pipeline, // 正式基础级竞争固定管线。
        &pipeline_cache,   // 查询竞争管线状态并取得真实布局。
        &render_device,    // 创建共享输入缓冲和目标绑定组。
        &render_queue,     // 上传当前目标层uniform。
        &mut state,        // 跨同组串行层复用大输入缓冲。
    ) {
        GpuComposePrepareOutcome::Waiting => return, // Shader或图片仍在异步准备时下一帧重试。
        GpuComposePrepareOutcome::Ready {
            previous_texture,
            dispatch,
        } => (previous_texture, dispatch), // 根图按复制、竞争、Mip顺序执行。
        GpuComposePrepareOutcome::Failed(reason) => {
            publish_prepare_failure(job, reason, &shared, &mut state); // 硬错误进入原候选托底路径。
            return;
        }
    };
    let dispatches = match build_gpu_mip_dispatches(job.width, job.height) {
        Ok(dispatches) => dispatches, // 计划与CPU及独立WGPU验收共享同一函数。
        Err(reason) => {
            publish_prepare_failure(job, reason, &shared, &mut state); // 零尺寸或溢出立即失败。
            return;
        }
    };
    if dispatches.len() + 1 != job.mip_level_count as usize {
        publish_prepare_failure(
            job,
            format!(
                "dispatch chain has {} levels but candidate declares {}",
                dispatches.len() + 1,
                job.mip_level_count
            ),
            &shared,
            &mut state,
        ); // 防止调度和图片描述符分叉。
        return;
    }
    let readback_plan = match GpuMipReadbackPlan::new(job.width, job.height, job.mip_level_count) {
        Ok(plan) => plan, // 全部级共享一次动态大小MAP_READ缓冲。
        Err(reason) => {
            publish_prepare_failure(job, reason, &shared, &mut state); // 对齐或容量错误保留旧活动图。
            return;
        }
    };
    let real_layout = pipeline_cache.get_bind_group_layout(&pipeline.bind_group_layout); // 当前设备解析后的唯一group0布局。
    let mut prepared_dispatches = Vec::with_capacity(dispatches.len()); // 每一项只暴露相邻且不重叠的Mip视图。
    for dispatch in dispatches {
        let source_view = gpu_image.texture.create_view(&TextureViewDescriptor {
            label: Some("vegetation_gpu_mip_source_view"), // 图形调试器中标识当前只读父级。
            base_mip_level: dispatch.source_level,         // 精确选择一个父Mip。
            mip_level_count: Some(1),                      // 禁止视图意外包含写入目标级。
            ..Default::default()                           // 保持RGBA8二维单层默认方面。
        });
        let target_view = gpu_image.texture.create_view(&TextureViewDescriptor {
            label: Some("vegetation_gpu_mip_target_view"), // 图形调试器中标识当前只写子级。
            base_mip_level: dispatch.target_level,         // 精确选择相邻目标Mip。
            mip_level_count: Some(1),                      // 禁止读写视图子资源重叠。
            ..Default::default()                           // 保持RGBA8二维单层默认方面。
        });
        let mut uniform = UniformBuffer::from(dispatch.uniform()); // Bevy按ShaderType编码两个UVec2。
        uniform.set_label(Some("vegetation_gpu_mip_uniform")); // 每级固定十六字节便于GPU调试识别。
        uniform.write_buffer(&render_device, &render_queue); // 根图执行前上传当前父子尺寸。
        let Some(uniform_buffer) = uniform.buffer().cloned() else {
            publish_prepare_failure(
                job,
                "failed to allocate mip uniform buffer".to_string(),
                &shared,
                &mut state,
            ); // 资源分配失败时整层候选不可采用。
            return;
        };
        let bind_group = render_device.create_bind_group(
            "vegetation_gpu_mip_bind_group", // 一个绑定组只服务一次相邻级调度。
            &real_layout,
            &BindGroupEntries::sequential((
                &source_view,                       // binding0只读父Mip。
                &target_view,                       // binding1只写子Mip。
                uniform_buffer.as_entire_binding(), // binding2完整十六字节尺寸参数。
            )),
        );
        let (workgroups_x, workgroups_y) = dispatch.workgroups(); // 覆盖目标有效范围。
        prepared_dispatches.push(VegetationGpuMipDispatch {
            bind_group,              // RenderGraph无需重新查询资源。
            uniform: uniform_buffer, // 显式延长uniform生命周期到作业完成。
            workgroups_x,            // X尾线程由WGSL边界判断裁掉。
            workgroups_y,            // Y尾线程使用相同规则。
        });
    }
    let readback = render_device.create_buffer(&BufferDescriptor {
        label: Some("vegetation_gpu_mip_readback"), // 图形调试器中标识版本化全Mip回读。
        size: readback_plan.buffer_size(),          // 已包含每级行填充和级间对齐。
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ, // GPU复制后只由异步CPU回调读取。
        mapped_at_creation: false,                  // 运行时必须统一经过map_async状态机。
    });
    state.current = Some(Arc::new(VegetationGpuMipPreparedJob {
        identity: job.clone(),              // 回调发布原票据、版本、层和候选句柄身份。
        texture: gpu_image.texture.clone(), // 全部视图、Compute和复制使用同一纹理。
        previous_texture,                   // 局部提交在正式竞争前只从上一有效GPU图复制Mip0。
        compose,                            // 正式作业执行提交块竞争；旧基础级上传烟雾合法为空。
        readback,                           // 当前层独占动态staging缓冲。
        readback_plan,                      // 节点和回调共享不可变复制/解码计划。
        dispatches: prepared_dispatches,    // 1×1候选为空但仍会回读基础级。
        state: AtomicU8::new(JOB_READY),    // 根图下一次运行取得独占录入权。
    }));
    state.last_key = Some(key); // 同一请求以后只等待节点和回调，不重新准备。
}

/// 把Prepare阶段硬错误作为当前身份的一次失败结果发布。
fn publish_prepare_failure(
    job: &super::super::resources::VegetationGpuMipJob, // 保留当前请求完整身份。
    reason: String,                                     // 可读资源或管线错误。
    shared: &VegetationGpuMipShared,                    // 异步通道同样承载同步Prepare失败。
    state: &mut VegetationGpuMipRenderState,            // 标记同请求已经处理避免重复发布。
) {
    shared.publish(VegetationGpuMipResult {
        ticket: job.ticket,                               // 主世界拒绝迟到票据。
        revision: job.revision,                           // 主世界拒绝过时活动版本。
        layer: job.layer,                                 // 只让当前串行层进入失败路径。
        outcome: VegetationGpuMipOutcome::Failed(reason), // 保留旧活动图片并拒绝整批。
    });
    state.current = None; // 硬错误没有根图可执行作业。
    state.last_key = Some((job.ticket, job.layer)); // 同一提取请求只发布一次失败。
}
