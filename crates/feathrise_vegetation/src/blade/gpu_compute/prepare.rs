//! GPU草地紧凑实例缓冲、间接状态和逐圈计算绑定组准备。

use bevy::ecs::system::SystemParam; // 把相关渲染资源组合成具名参数集，避免超长系统签名。
use bevy::pbr::RenderMeshInstances;
use bevy::prelude::*; // Commands、Query与渲染资源系统参数。
use bevy::render::extract_component::{ComponentUniforms, DynamicUniformIndex}; // 复用每窗288字节动态uniform。
use bevy::render::mesh::{RenderMesh, RenderMeshBufferInfo, allocator::MeshAllocator}; // 从共享网格缓冲取得间接绘制切片。
use bevy::render::render_asset::RenderAssets; // 解析已经上传GPU的草原型和地表纹理。
use bevy::render::render_resource::{
    // 创建持久缓冲、计算绑定组并声明跨阶段用途。
    BindGroupEntries,
    BufferDescriptor,
    BufferUsages,
    CachedPipelineState,
    PipelineCache,
    UniformBuffer, // 每窗只上传固定 336 字节分页高度参数。
};
use bevy::render::renderer::{RenderDevice, RenderQueue}; // 分配GPU缓冲并刷新每帧80字节状态模板。
use bevy::render::sync_world::MainEntity; // 由渲染实体定位Bevy共享草原型网格实例。
use bevy::render::texture::{FallbackImageZero, GpuImage}; // 未启用地表字段时提供合法零纹理绑定。 // 取得当前草窗Mesh3d对应的GPU网格资产编号。
use bevy::shader::ShaderCacheError; // 区分可重试Shader等待与必须暴露的Compute管线错误。

use crate::components::{
    GpuBladeClipmapHeight, // 与地形共享的已采用分页资源。
    // 每圈输入、地表句柄和跨帧GPU输出组件。
    GpuBladeDensityMap,
    GpuBladeHeightMap,
    GpuBladePatch,
    GpuBladePreparedBuffers,
    GpuBladeVolumeSource,
};
use crate::constants::{
    GPU_GRASS_COMPACT_INSTANCE_BYTES, // 单株紧凑输出步长。
    GPU_GRASS_DEPTH_BUCKETS_BYTES,    // 八个径向桶的间接参数与原子状态总大小。
    GPU_GRASS_DRAW_STATE_BYTES,       // 聚合诊断状态大小。
    GPU_GRASS_TILE_DISPATCH_BYTES,    // GPU瓦片间接派发状态大小。
    GPU_GRASS_TILE_INDEX_BYTES,       // 活跃瓦片列表单元素步长。
}; // 锁定Rust、WGSL和间接API共用字节布局。
use crate::resources::{
    // 当前帧计算作业、计算布局和F3阶段计数器。
    GpuBladeComputeJobs,
    GpuBladeComputePipeline,
    GpuBladeRenderFrameCounters,
};
use crate::types::GpuBladeComputeJob; // RenderGraph消费的单窗轻量作业记录。

use super::culling::GpuBladeViewCulling; // 全部草窗计算绑定同一份玩法相机六平面uniform。
use super::helpers::{
    GpuBladeDrawStateTemplate, compact_buffer_bytes, compact_output_capacity,
    encode_depth_bucket_draws, encode_draw_state, encode_tile_dispatch,
    encode_volume_depth_bucket_draws, tile_count, tile_cull_group_count, tile_dimensions,
}; // 复用可测试的容量、瓦片派发和状态编码规则。

/// 单个渲染世界草窗准备作业需要读取的组件集合。
pub(super) type GpuBladeWindowQuery<'a> = (
    Entity,                                 // 当前渲染实体是持久输出组件的生命周期所有者。
    &'a MainEntity,                         // 主世界草窗实体用于查询RenderMeshInstances。
    &'a GpuBladePatch,                      // 候选数量、格距和外半径决定派发量与安全容量。
    &'a DynamicUniformIndex<GpuBladePatch>, // group0 binding0的动态字节偏移。
    Option<&'a GpuBladeClipmapHeight>,      // 明确区分分页高度与 GLB 整图高度。
    Option<&'a GpuBladeHeightMap>,          // 可选正式高度纹理句柄。
    Option<&'a GpuBladeDensityMap>,         // 可选正式密度纹理句柄。
    Option<&'a GpuBladeVolumeSource>,
    Option<&'a crate::components::GpuBladeSourceRevision>,
    Option<&'a mut GpuBladePreparedBuffers>, // 容量不变时跨帧复用几十MiB输出并更新是否已生成有效结果。
); // 完成草窗组件查询别名，避免系统入口暴露难读的七元组。

/// 草窗准备阶段共享的渲染资源参数组。
#[derive(SystemParam)]
pub(super) struct GpuBladePrepareResources<'w> {
    clipmap_fallback: Res<'w, super::clipmap_resources::ClipmapFallback>, // 旧地图共享合法禁用绑定。
    pipeline: Res<'w, GpuBladeComputePipeline>, // 提供计算group0的声明式布局。
    pipeline_cache: Res<'w, PipelineCache>,     // 把声明式布局解析为当前RenderDevice真实布局。
    uniforms: Res<'w, ComponentUniforms<GpuBladePatch>>, // 所有草窗共享的动态uniform缓冲。
    images: Res<'w, RenderAssets<GpuImage>>,    // 解析每圈高度与密度纹理视图。
    fallback_image: Res<'w, FallbackImageZero>, // 未启用纹理输入时仍满足固定计算布局。
    meshes: Res<'w, RenderAssets<RenderMesh>>,  // 读取grass.glb原型索引数量与索引格式。
    render_mesh_instances: Res<'w, RenderMeshInstances>, // 从MainEntity取得草原型资产ID。
    mesh_allocator: Res<'w, MeshAllocator>,     // 读取共享顶点和索引缓冲的绝对切片起点。
    render_device: Res<'w, RenderDevice>,       // 创建STORAGE、VERTEX与INDIRECT复合用途缓冲。
    render_queue: Res<'w, RenderQueue>,         // 每帧只上传每圈小状态模板并清零原子计数。
    view_culling: Res<'w, GpuBladeViewCulling>, // 读取唯一玩法相机六平面uniform。
    render_stats: Res<'w, GpuBladeRenderFrameCounters>, // 发布准备阶段诊断。
    compute_jobs: ResMut<'w, GpuBladeComputeJobs>, // 重建最多近中远三项的本帧作业列表。
} // 完成共享资源参数分组，系统签名只保留命令、资源与草窗查询三项职责。

/// 当本帧不能安全重算时，只统计真正拥有完整历史结果的驻留窗口。
fn record_resident_reuse_for_valid_outputs(
    grass_windows: &mut Query<GpuBladeWindowQuery<'_>>, // 遍历当前渲染世界全部草窗及其可选持久输出。
    render_stats: &GpuBladeRenderFrameCounters,         // 把复用数量发布到下一帧主世界诊断快照。
) {
    for (_, _, _, _, _, _, _, _, _, prepared_buffers) in grass_windows.iter_mut() {
        if prepared_buffers
            .as_deref()
            .is_some_and(|buffers| buffers.output_valid)
        {
            render_stats.record_resident_reused_window(); // 首次未生成过结果的缓冲不能伪装成可驻留绘制。
        } // 完成单窗历史输出有效性检查。
    } // 完成当前等待帧的驻留复用统计。
}

/// 即使 Shader 或 uniform 仍在等待，也只允许与当前完整高度快照一致的历史输出。
fn output_matches_clipmap_snapshot(
    output: Option<([u64; 3], [UVec4; 16])>,
    current: Option<([u64; 3], [UVec4; 16], bool)>,
) -> bool {
    // 统一处理进入、退出与更新分页高度。
    match current {
        // 高度模式切换同样属于完整输出身份变化。
        Some((revision, missing_pages, ready)) => {
            ready && output == Some((revision, missing_pages))
        } // 任一地图/高度/视图变化或未就绪都拒绝复用。
        None => output.is_none(), // GLB 窗口不得继承之前分页窗口的实例高度。
    } // 完成完整身份判定。
} // 完成与 GPU 设备无关的原子复用规则。
/// 四个固定Compute阶段必须全部就绪，才允许Prepare覆盖上一份完整间接绘制状态。
fn compute_pipeline_chain_is_ready(stage_readiness: [bool; 4]) -> bool {
    stage_readiness.into_iter().all(|is_ready| is_ready) // 任一false都让本帧原子保留旧输出。
}

/// 在渲染资源全部就绪后复用或创建每圈GPU输出，并组装本帧计算作业。
pub(super) fn prepare_gpu_grass_compute_jobs(
    mut commands: Commands, // 仅在容量首次建立或变化时把持久缓冲附到渲染实体。
    resources: GpuBladePrepareResources, // 具名资源参数集避免十五项位置签名和相邻类型误读。
    mut grass_windows: Query<GpuBladeWindowQuery<'_>>, // 只遍历草窗并维护持久GPU输出的有效生命周期。
) {
    let GpuBladePrepareResources {
        clipmap_fallback,      // 解构分页采样禁用资源。
        pipeline,              // 解构计算绑定布局资源。
        pipeline_cache,        // 解构当前设备管线缓存。
        uniforms,              // 解构草窗动态uniform缓冲。
        images,                // 解构GPU图片资产表。
        fallback_image,        // 解构未启用地表输入时的零纹理。
        meshes,                // 解构GPU原型网格资产表。
        render_mesh_instances, // 解构主世界网格实例映射。
        mesh_allocator,        // 解构共享网格缓冲分配器。
        render_device,         // 解构当前GPU设备。
        render_queue,          // 解构当前GPU提交队列。
        view_culling,          // 解构玩法视锥uniform。
        render_stats,          // 解构本帧草地诊断计数器。
        mut compute_jobs,      // 解构并取得可写本帧Compute作业列表。
    } = resources; // 后续主体继续使用短语义名称并保持既有准备顺序。
    compute_jobs.jobs.clear(); // RenderGraph不得重复执行上一帧已经失效的窗口或绑定组。
    for (_, _, _, _, height, _, _, _, _, buffers) in &mut grass_windows {
        // 必须先于管线、uniform、视图等待的全部早退。
        if let Some(mut buffers) = buffers {
            // 首次没有输出的窗口不需要失效操作。
            let current =
                height.map(|height| (height.revision, height.missing_pages, height.ready)); // 捕获完整地图/高度/视图身份与采用状态。
            if !output_matches_clipmap_snapshot(
                buffers
                    .clipmap_revision
                    .map(|revision| (revision, buffers.clipmap_missing_pages)),
                current,
            ) {
                // Shader 热更新也不能借旧输出绕过高度换代。
                buffers.output_valid = false; // DrawCommand 再次核对该值，即使 Queue 早于本阶段也不显示旧草。
            } // 完成单窗历史输出失效。
        } // 完成已有缓冲检查。
    } // 之后任何早退都只能保留仍合法的历史实例。
    let window_count = grass_windows.iter_mut().count() as u32; // 当前地图可同时包含多种内容的近中远窗口，仅用于等待诊断与空场景快返。
    if window_count == 0 {
        return; // 没有草窗时不查询异步管线状态，也不产生虚假的四阶段就绪指标。
    } // 完成空场景快速路径。
    let pipeline_ids = [
        pipeline.cull_tiles_pipeline, // 第一阶段执行保守瓦片常驻裁剪与玩法视锥粗剔除。
        pipeline.finalize_tiles_pipeline, // 第二阶段生成候选分类间接派发参数。
        pipeline.classify_pipeline,   // 第三阶段逐候选执行距离、密度、高度与压缩。
        pipeline.finalize_pipeline,   // 第四阶段钳制实例数并为当前窗口生成八条间接绘制参数。
    ]; // Prepare阶段必须先确认整条链路可执行，之后才允许清零持久间接状态。
    let mut pipeline_stage_readiness = [false; 4]; // 默认全部不可用，只有缓存明确返回Ok才逐项打开。
    for (stage_index, pipeline_id) in pipeline_ids.into_iter().enumerate() {
        pipeline_stage_readiness[stage_index] =
            match pipeline_cache.get_compute_pipeline_state(pipeline_id) {
                CachedPipelineState::Ok(_) => {
                    render_stats.record_compute_pipeline_ready(); // 已编译阶段允许稍后组装并执行作业。
                    true // 当前固定阶段可以进入本帧完整链路。
                }
                CachedPipelineState::Queued | CachedPipelineState::Creating(_) => {
                    render_stats.record_compute_pipeline_waiting(); // 首次加载和热重载明确表现为异步等待。
                    false // 不完整链路不得提前覆盖旧间接参数。
                }
                CachedPipelineState::Err(
                    ShaderCacheError::ShaderNotLoaded(_)
                    | ShaderCacheError::ShaderImportNotYetAvailable,
                ) => {
                    render_stats.record_compute_pipeline_waiting(); // Shader资产尚未进入缓存时下一帧自动重试。
                    false // 等待期间继续消费上一份有效GPU输出。
                }
                CachedPipelineState::Err(_) => {
                    render_stats.record_compute_pipeline_error(); // 编译或设备错误必须进入F3而不是伪装成资源等待。
                    false // 失败链路同样不得破坏最后一份完整结果。
                }
            }; // 完成单个固定Compute阶段状态分类并保存原子门槛输入。
    } // 完成四阶段原子就绪门槛检查。
    if !compute_pipeline_chain_is_ready(pipeline_stage_readiness) {
        record_resident_reuse_for_valid_outputs(&mut grass_windows, &render_stats); // 热重载只复用真实生成过的结果。
        return; // 关键修复：在任何write_buffer清零之前退出。
    } // 完成Compute链路未就绪快返。
    let Some(uniform_binding) = uniforms.uniforms().binding() else {
        render_stats.record_uniform_waiting(window_count); // uniform尚未上传时明确记录全部等待窗口。
        record_resident_reuse_for_valid_outputs(&mut grass_windows, &render_stats); // uniform重建期间继续绘制已有结果。
        return; // 没有动态uniform就不能安全运行任何计算入口。
    };
    let Some(view_culling_binding) = view_culling.binding() else {
        render_stats.record_uniform_waiting(window_count); // 共享视锥缓冲尚未建立时沿用统一准备等待计数。
        record_resident_reuse_for_valid_outputs(&mut grass_windows, &render_stats); // 相机uniform等待期间继续绘制已有结果。
        return; // 缺失六平面绑定时不允许创建与管线布局不完整的计算作业。
    };
    let real_layout = pipeline_cache.get_bind_group_layout(&pipeline.bind_group_layout); // 当前设备上两个计算入口共用同一真实布局。
    let limits = render_device.limits(); // 设备限制只在准备阶段读取一次，避免为每圈重复查询。
    let maximum_storage_instances = (limits.max_storage_buffer_binding_size
        / GPU_GRASS_COMPACT_INSTANCE_BYTES)
        .min(limits.max_buffer_size / GPU_GRASS_COMPACT_INSTANCE_BYTES)
        .min(u32::MAX as u64) as u32; // 单窗输出同时满足storage绑定、buffer总大小和u32间接实例上限。

    for (
        entity,
        main_entity,
        patch,
        uniform_index,
        clipmap_height,
        height_map,
        density_map,
        volume_source,
        source_revision,
        existing_buffers,
    ) in &mut grass_windows
    {
        if patch.retains_resident_output_for_render_frame() {
            if existing_buffers
                .as_deref()
                .is_some_and(|buffers| buffers.output_valid)
            {
                render_stats.record_resident_reused_window(); // 纹理等待只复用已经完整生成过的旧草窗结果。
            } // 首次加载没有合法输出时保持零草并等待下一帧资源就绪。
            continue; // 禁止fallback纹理覆盖驻留的真实高度、密度和间接绘制参数。
        } // 完成地形高度或植被密度输入等待时的驻留复用分支。
        if patch.instance_count() == 0 {
            continue; // 地形高度或植被密度纹理未就绪时前置系统已把本帧副本候选数清零。
        }
        let clipmap_images = clipmap_height.and_then(|height| {
            images
                .get(&height.atlas)
                .zip(images.get(&height.page_table))
        }); // 两张图必须完整就绪。
        if clipmap_height.is_some_and(|height| !height.ready || !height.stitches.valid())
            || (clipmap_height.is_some() && clipmap_images.is_none())
        {
            // 双重门防止布局变更时意外读取兼容零页。
            continue; // 不清零也不派发缺少真实输入的作业。
        } // 完成分页输入原子检查。
        let mut clipmap_uniform = clipmap_height
            .map(|height| UniformBuffer::from(super::clipmap::encode_clipmap(height))); // 固定大小参数不复制页数据。
        if let Some(uniform) = clipmap_uniform.as_mut() {
            // GLB 路径共享永久禁用 uniform。
            uniform.write_buffer(&render_device, &render_queue); // 上传固定 336 字节，图集继续复用地形资源。
        } // 完成每窗轻量参数上传。
        let clipmap_binding = clipmap_uniform
            .as_ref()
            .and_then(UniformBuffer::binding)
            .unwrap_or_else(|| {
                clipmap_fallback
                    .uniform
                    .binding()
                    .expect("初始化分页兼容 uniform")
            }); // 两种高度路径均有合法布局。
        let (clipmap_atlas, clipmap_table) = clipmap_images.map_or(
            (&clipmap_fallback.atlas, &clipmap_fallback.page_table),
            |(atlas, table)| (&atlas.texture_view, &table.texture_view),
        ); // 只克隆视图引用，不分配高度纹理。
        let tile_dimensions = tile_dimensions(patch); // CPU只计算当前草窗固定的二维瓦片布局，不接触逐草候选。
        let tile_count = tile_count(tile_dimensions); // 远圈约三万五千瓦片，远小于八百八十一万逐草候选。
        let tile_cull_groups = tile_cull_group_count(tile_count); // 每六十四个瓦片只执行一个粗判工作组。
        if tile_cull_groups == 0 || maximum_storage_instances == 0 {
            continue; // 空瓦片或不支持最小二十字节storage绑定的设备无法建立安全作业。
        }
        let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(*main_entity) else {
            continue; // 草原型提取尚未完成时等待下一帧，排队系统会单独记录同类原因。
        };
        let mesh_asset_id = mesh_instance.mesh_asset_id(); // Bevy 0.19通过共享实例访问器读取原子保存的资产身份。
        let Some(gpu_mesh) = meshes.get(mesh_asset_id) else {
            continue; // grass.glb网格尚未上传GPU时不创建无效间接参数。
        };
        let RenderMeshBufferInfo::Indexed { count, .. } = &gpu_mesh.buffer_info else {
            continue; // 当前正式草原型必须是索引网格，间接状态固定使用DrawIndexedIndirectArgs。
        };
        let Some(vertex_slice) = mesh_allocator.mesh_vertex_slice(&mesh_asset_id) else {
            continue; // 共享顶点缓冲尚未分配时等待下一帧。
        };
        let Some(index_slice) = mesh_allocator.mesh_index_slice(&mesh_asset_id) else {
            continue; // 共享索引缓冲尚未分配时等待下一帧。
        };

        let requested_capacity = compact_output_capacity(patch); // 圆形常驻裁剪只为可能存活区域预留紧凑输出，而非复制全部候选。
        let capacity = requested_capacity.min(maximum_storage_instances).max(1); // 极端设备上宁可由overflow报警也禁止非法大绑定。
        let generation = existing_buffers.as_deref().map_or(1, |buffers| {
            if buffers.capacity == capacity && buffers.tile_count == tile_count {
                buffers.generation // 容量完全一致时保持代次并跨帧复用同一GPU内存。
            } else {
                buffers.generation.saturating_add(1) // 重建后递增代次，后续异步回读可拒绝旧样本。
            }
        });
        let prepared_buffers = match existing_buffers {
            Some(mut buffers)
                if buffers.capacity == capacity && buffers.tile_count == tile_count =>
            {
                buffers.clipmap_missing_pages =
                    clipmap_height.map_or([UVec4::ZERO; 16], |height| height.missing_pages); // 同次发布已采用的完整局部缺页掩码。
                buffers.clipmap_revision = clipmap_height.map(|height| height.revision); // 记录本次完整输出身份。
                buffers.output_valid = true; // 管线已通过原子就绪门槛，本帧RenderGraph会在相机绘制前完整覆盖输出。
                buffers.source_revision = source_revision.map_or(0, |revision| revision.0);
                buffers.clone() // 正常帧只克隆轻量wgpu句柄，不重新分配显存。
            }
            _ => {
                let buffers = GpuBladePreparedBuffers {
                    instances: render_device.create_buffer(&BufferDescriptor {
                        label: Some("gpu_grass_compact_instances"), // 图形调试器中识别每圈紧凑输出。
                        size: compact_buffer_bytes(capacity),       // 二十字节乘安全存活上限。
                        usage: BufferUsages::STORAGE | BufferUsages::VERTEX, // Compute写完后直接作为实例顶点流读取。
                        mapped_at_creation: false, // 禁止CPU映射和逐草上传路径。
                    }),
                    draw_state: render_device.create_buffer(&BufferDescriptor {
                        label: Some("gpu_grass_aggregate_draw_state"), // 图形调试器中识别每圈聚合存活与瓦片诊断。
                        size: GPU_GRASS_DRAW_STATE_BYTES, // 八十字节继续保存压缩、瓦片和容量诊断。
                        usage: BufferUsages::STORAGE
                            | BufferUsages::COPY_DST
                            | BufferUsages::COPY_SRC, // CPU清零模板、GPU计算和异步回读共用，不再承担实际draw参数。
                        mapped_at_creation: false, // 状态通过RenderQueue写入，不建立持久CPU映射。
                    }),
                    depth_bucket_draws: render_device.create_buffer(&BufferDescriptor {
                        label: Some("gpu_grass_depth_bucket_draws"), // 图形调试器中识别八个由近到远的径向间接draw。
                        size: GPU_GRASS_DEPTH_BUCKETS_BYTES, // 八条三十二字节记录总计二百五十六字节。
                        usage: BufferUsages::STORAGE
                            | BufferUsages::INDIRECT
                            | BufferUsages::COPY_DST, // CPU清零小模板、GPU分类收尾和AlphaMask间接绘制共用。
                        mapped_at_creation: false, // 禁止CPU映射，实例数量始终由GPU生成。
                    }),
                    active_tiles: render_device.create_buffer(&BufferDescriptor {
                        label: Some("gpu_grass_active_tiles"), // 图形调试器中识别GPU粗剔除生成的短工作列表。
                        size: u64::from(tile_count).saturating_mul(GPU_GRASS_TILE_INDEX_BYTES), // 最坏情况允许全部瓦片进入细分类。
                        usage: BufferUsages::STORAGE, // 粗阶段写线性编号，细阶段读取同一列表。
                        mapped_at_creation: false,    // 列表完全由GPU每帧重建，CPU不上传瓦片内容。
                    }),
                    tile_dispatch: render_device.create_buffer(&BufferDescriptor {
                        label: Some("gpu_grass_tile_dispatch"), // 前十二字节直接作为细候选间接派发参数。
                        size: GPU_GRASS_TILE_DISPATCH_BYTES, // XYZ加活跃瓦片原子计数固定十六字节。
                        usage: BufferUsages::STORAGE
                            | BufferUsages::INDIRECT
                            | BufferUsages::COPY_DST, // CPU只清零十六字节，GPU写X并由后续Compute间接消费。
                        mapped_at_creation: false, // 禁止CPU读取活跃瓦片数，诊断统一走draw_state异步回读。
                    }),
                    classify_dispatch_scratch: render_device.create_buffer(&BufferDescriptor {
                        label: Some("gpu_grass_classify_dispatch_scratch"), // wgpu 29用资源身份区分分类阶段Storage绑定与真实Indirect参数。
                        size: GPU_GRASS_TILE_DISPATCH_BYTES, // 保持binding7声明的十六字节最小尺寸。
                        usage: BufferUsages::STORAGE, // shader入口不读取该占位值，只需满足固定绑定组布局。
                        mapped_at_creation: false, // 缓冲随草窗跨帧复用，不建立CPU映射或逐帧初始化。
                    }),
                    capacity,   // Draw与Compute共享同一安全实例上限。
                    tile_count, // 列表容量与当前候选网格瓦片数量完全一致。
                    generation, // 记录这次容量分配代次。
                    clipmap_missing_pages: clipmap_height
                        .map_or([UVec4::ZERO; 16], |height| height.missing_pages), // 不同缺页集合不得复用未抑制的旧草根。
                    clipmap_revision: clipmap_height.map(|height| height.revision), // 新输出绑定当前完整地图/高度/视图身份。
                    output_valid: true, // 四阶段已就绪且作业将在相机前执行，因此本帧排队可安全消费该输出。
                    source_revision: source_revision.map_or(0, |revision| revision.0),
                };
                commands.entity(entity).insert(buffers.clone()); // 让AlphaMask绘制命令在同一渲染实体读取这组输出。
                buffers // 本帧计算绑定组无需等待组件命令应用即可直接使用新缓冲。
            }
        };

        let state_bytes = encode_draw_state(GpuBladeDrawStateTemplate {
            index_count: *count,                          // grass.glb当前primitive索引数。
            first_index: index_slice.range.start,         // Bevy共享索引缓冲内的绝对first_index。
            base_vertex: vertex_slice.range.start as i32, // Bevy共享顶点缓冲内的有符号base_vertex。
            capacity,                                     // Compute写保护与Finalize钳制的共同上限。
            candidate_count: patch.instance_count(),      // 尾工作组线程不得越过本圈真实候选数。
            tile_count,                                   // 保存粗瓦片总量供F3计算剔除比例。
            lod_index: patch.lod_level(),                 // 保存近中远材质级别供GPU状态诊断核对。
            buffer_generation: generation,                // 状态与实例输出使用相同代次。
            vegetation_profile: patch.profile_slot().index(), // 保存受检profile槽编号供回读拆分。
            vertex_count: vertex_slice
                .range
                .end
                .saturating_sub(vertex_slice.range.start), // 直接使用MeshAllocator当前primitive真实顶点切片长度。
        }); // 以具名字段构建八十字节聚合状态，防止相邻u32参数错位。
        render_queue.write_buffer(&prepared_buffers.draw_state, 0, &state_bytes); // 每圈每帧只写80字节并同时清零全部原子计数。
        let depth_bucket_bytes = (if patch.uses_volume_source() {
            encode_volume_depth_bucket_draws
        } else {
            encode_depth_bucket_draws
        })(
            *count,                          // 八个桶复用同一grass.glb原型索引数。
            index_slice.range.start,         // 八个桶复用同一共享索引切片。
            vertex_slice.range.start as i32, // 八个桶复用同一共享顶点基址。
            capacity,                        // 原实例缓冲容量被八个等面积桶完整瓜分。
        ); // 构建本帧近到远间接参数和清零后的分区预留状态。
        render_queue.write_buffer(
            &prepared_buffers.depth_bucket_draws, // 每圈独立的八桶间接参数缓冲。
            0,                                    // 第一桶始终从缓冲首字节开始。
            &depth_bucket_bytes,                  // 二百五十六字节固定模板不包含任何逐草数据。
        ); // 完成当前圈深度桶计数清零与分区恢复。
        render_queue.write_buffer(
            &prepared_buffers.tile_dispatch, // 同一持久缓冲每帧只重置XYZ与活跃计数。
            0,                               // 间接参数始终从缓冲首字节开始。
            &encode_tile_dispatch(),         // X和计数归零，Y/Z恢复为合法的一。
        ); // 完成无需CPU瓦片列表的GPU粗剔除初始状态。

        let gpu_height_image = height_map
            .and_then(|height_map| images.get(&height_map.image))
            .unwrap_or(&fallback_image); // 未启用高度场时shader不读取该零纹理。
        let gpu_density_image = density_map
            .and_then(|density_map| images.get(&density_map.image))
            .unwrap_or(&fallback_image); // 未启用密度场时shader直接返回完整概率。
        let volume_image = volume_source.and_then(|s| images.get(&s.0));
        if volume_source.is_some() && volume_image.is_none() {
            continue;
        }
        let volume_texture = &volume_image.unwrap_or(&fallback_image).texture_view;
        let tile_bind_group = render_device.create_bind_group(
            "gpu_grass_tile_compute_bind_group", // 粗剔除阶段绑定真实可写派发状态。
            &real_layout,
            &BindGroupEntries::sequential((
                uniform_binding.clone(), // binding0：共享动态GpuBladePatch uniform缓冲。
                &gpu_height_image.texture_view, // binding1：RGBA16高度单元纹理。
                &gpu_density_image.texture_view, // binding2：RGBA8密度概率纹理。
                prepared_buffers.instances.as_entire_binding(), // binding3：二十字节紧凑实例输出数组。
                prepared_buffers.draw_state.as_entire_binding(), // binding4：八十字节原子、间接与瓦片统计状态。
                view_culling_binding.clone(), // binding5：唯一玩法相机本帧左、右、上、下、近、远六平面。
                prepared_buffers.active_tiles.as_entire_binding(), // binding6：GPU粗剔除生成的活跃瓦片编号列表。
                prepared_buffers.tile_dispatch.as_entire_binding(), // binding7：GPU生成的候选间接派发参数与追加计数。
                prepared_buffers.depth_bucket_draws.as_entire_binding(), // binding8：八个等面积径向桶的分区预留和间接参数。
                clipmap_binding.clone(), // binding9 当前已采用分页采样参数。
                clipmap_atlas,           // binding10 共享地形高度数组。
                clipmap_table,           // binding11 共享地形紧凑页表。
                volume_texture,
            )),
        );
        let classify_bind_group = render_device.create_bind_group(
            "gpu_grass_classify_compute_bind_group", // 分类阶段必须让真实派发缓冲只以Indirect身份进入同一次dispatch。
            &real_layout,
            &BindGroupEntries::sequential((
                uniform_binding.clone(), // binding0：共享动态GpuBladePatch uniform缓冲。
                &gpu_height_image.texture_view, // binding1：RGBA16高度单元纹理。
                &gpu_density_image.texture_view, // binding2：RGBA8密度概率纹理。
                prepared_buffers.instances.as_entire_binding(), // binding3：二十字节紧凑实例输出数组。
                prepared_buffers.draw_state.as_entire_binding(), // binding4：八十字节原子、间接与瓦片统计状态。
                view_culling_binding.clone(), // binding5：唯一玩法相机本帧左、右、上、下、近、远六平面。
                prepared_buffers.active_tiles.as_entire_binding(), // binding6：GPU粗剔除生成的活跃瓦片编号列表。
                prepared_buffers
                    .classify_dispatch_scratch
                    .as_entire_binding(), // binding7：未被分类/绘制收尾入口读取的独立占位Storage，规避wgpu 29互斥用法。
                prepared_buffers.depth_bucket_draws.as_entire_binding(), // binding8：八个等面积径向桶的分区预留和间接参数。
                clipmap_binding.clone(), // binding9 当前已采用分页采样参数。
                clipmap_atlas,           // binding10 共享地形高度数组。
                clipmap_table,           // binding11 共享地形紧凑页表。
                volume_texture,
            )),
        );
        compute_jobs.jobs.push(GpuBladeComputeJob {
            tile_bind_group,                       // RenderGraph粗剔除阶段不再查询ECS或重建绑定。
            classify_bind_group, // 分类阶段使用隔离后的binding7，同时保留其他输出完全一致。
            uniform_offset: uniform_index.index(), // 选择当前内容当前几何档的288字节窗口记录。
            tile_cull_groups,    // 粗阶段工作量与瓦片数成正比，不再与一千万候选直接相等。
            tile_dispatch: prepared_buffers.tile_dispatch.clone(), // 细阶段直接消费GPU刚生成的活跃瓦片数。
            lod_priority: patch.lod_level(), // 零至三让近、中、远、超远窗口稳定按级别提交。
            vegetation_profile: patch.profile_slot().index(), // 各内容按稳定profile槽归属Compute计时与诊断。
        });
        render_stats.record_prepared_bind_group(
            patch.profile_slot().index(), // 不同profile槽的同级窗口必须占不同完整性位。
            patch.lod_level(),            // 同时保留近中远身份，防止重复窗口冒充三级完整。
        );
    }
    compute_jobs
        .jobs
        .sort_by_key(|job| (job.vegetation_profile, job.lod_priority)); // 先按内容分组，再让每种内容内部稳定按近、中、远提交。
}

#[cfg(test)]
mod tests {
    use super::{compute_pipeline_chain_is_ready, output_matches_clipmap_snapshot};
    use bevy::prelude::UVec4; // 完整输出身份包含固定大小缺页掩码。 // 只验证清零前的四阶段原子门槛，不依赖真实GPU设备。

    #[test] // 取消、迟到、视图切换和 Shader 等待均不得复用不同高度版本的实例。
    fn clipmap_output_reuse_requires_complete_current_identity() {
        // 纯身份测试无需创建 GPU 缓冲。
        let current = [7, 11, 19]; // 地图、高度、视图三个独立代次。
        assert!(output_matches_clipmap_snapshot(
            Some((current, [UVec4::ZERO; 16])),
            Some((current, [UVec4::ZERO; 16], true))
        )); // 同版本可跨管线等待复用。
        assert!(!output_matches_clipmap_snapshot(
            Some((current, [UVec4::ZERO; 16])),
            Some((current, [UVec4::ZERO; 16], false))
        )); // 取消/未采用禁止复用。
        for changed in [[8, 11, 19], [7, 12, 19], [7, 11, 20]] {
            // 每个身份字段都独立构成失效条件。
            assert!(!output_matches_clipmap_snapshot(
                Some((current, [UVec4::ZERO; 16])),
                Some((changed, [UVec4::ZERO; 16], true))
            )); // 迟到地图、高度或视图结果全部拒绝。
        } // 完成三种换代覆盖。
        assert!(!output_matches_clipmap_snapshot(
            None,
            Some((current, [UVec4::ZERO; 16], true))
        )); // 首次分页窗口不得复用未知平面输出。
        assert!(!output_matches_clipmap_snapshot(
            Some((current, [UVec4::ZERO; 16])),
            None
        )); // 返回 GLB 也不得继承分页输出。
        let mut missing = [UVec4::ZERO; 16]; // 保持地图、高度、视图三个修订不变，仅切换待页集合。
        missing[3].x = 4; // 模拟同内容远景父页尚未换入。
        assert!(!output_matches_clipmap_snapshot(
            Some((current, [UVec4::ZERO; 16])),
            Some((current, missing, true))
        )); // mask变化也必须使历史GPU实例失效。
        assert!(output_matches_clipmap_snapshot(
            Some((current, missing)),
            Some((current, missing, true))
        )); // 完整相同mask仍可合法复用。
        assert!(output_matches_clipmap_snapshot(None, None)); // 原始 GLB 驻留复用保持兼容。
    } // 完成管线早退前的原子失效回归。
    #[test] // 全部四阶段就绪时才允许Prepare刷新间接状态。
    fn compute_pipeline_chain_requires_every_stage() {
        assert!(compute_pipeline_chain_is_ready([true, true, true, true])); // 完整链路允许生成本帧新输出。
        assert!(!compute_pipeline_chain_is_ready([false, true, true, true])); // 粗瓦片阶段等待必须保留旧输出。
        assert!(!compute_pipeline_chain_is_ready([true, false, true, true])); // 派发收尾等待必须保留旧输出。
        assert!(!compute_pipeline_chain_is_ready([true, true, false, true])); // 候选分类等待必须保留旧输出。
        assert!(!compute_pipeline_chain_is_ready([true, true, true, false])); // 间接绘制收尾等待必须保留旧输出。
    }
}
