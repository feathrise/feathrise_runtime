//! 在所有玩法相机绘制前执行草地分类、压缩和间接参数收尾的根RenderGraph系统。

use bevy::prelude::Res; // 根图系统通过只读资源参数取得当前帧短作业和固定管线。
use bevy::render::render_resource::{ComputePassDescriptor, PipelineCache}; // 创建计算通道并取得Prepare阶段已确认就绪的四个固定管线。
use bevy::render::renderer::RenderContext; // 在根图命令编码器上记录GPU计算命令。

use crate::gpu_timing::{
    CANDIDATE_CLASSIFY_END_TIMESTAMP, // 标记活跃候选逐草分类压缩完成边界。
    COMPUTE_BEGIN_TIMESTAMP,          // 标记全部草窗粗瓦片剔除开始边界。
    COMPUTE_FINALIZE_END_TIMESTAMP,   // 标记间接draw参数收尾完成边界。
    TILE_CULL_END_TIMESTAMP,          // 标记粗剔除与活跃瓦片派发参数完成边界。
    compute_timestamp_index,          // 把普通草、麦田等内容编号映射到彼此独立的四阶段查询槽。
    record_compute_timestamp,         // 支持时写GPU通道内时间戳，不支持时零成本快返。
};
use crate::resources::{
    // 读取分类管线、近中远作业与可选GPU计时查询。
    GpuBladeComputeJobs,
    GpuBladeComputePipeline,
    GpuBladeTimingQueries,
}; // 完成计算节点资源导入。

/// 无跨帧CPU状态的GPU草地计算系统。
pub(super) fn run_gpu_blade_compute(
    mut render_context: RenderContext, // 在Bevy当前帧根命令编码器上记录计算通道。
    jobs: Res<GpuBladeComputeJobs>,    // 读取PrepareBindGroups阶段完成的当前帧作业。
    pipeline_cache: Res<PipelineCache>, // Prepare阶段已原子确认四个异步计算管线全部可执行。
    pipeline: Res<GpuBladeComputePipeline>, // 取得瓦片、分类和收尾四个缓存ID。
    timing_queries: Option<Res<GpuBladeTimingQueries>>, // 无诊断插件或独立系统测试允许计时资源缺省。
) {
    if jobs.jobs.is_empty() {
        return; // 没有草窗或地表资源未就绪时不创建空计算通道。
    } // 完成空作业快返。
    let Some(cull_tiles_pipeline) =
        pipeline_cache.get_compute_pipeline(pipeline.cull_tiles_pipeline)
    else {
        return; // 防御同帧缓存异常；正常路径已在清零前通过Prepare原子就绪门槛。
    }; // 取得瓦片粗剔除管线。
    let Some(finalize_tiles_pipeline) =
        pipeline_cache.get_compute_pipeline(pipeline.finalize_tiles_pipeline)
    else {
        return; // 间接瓦片收尾未就绪时不能安全消费未完成的活跃计数。
    }; // 取得瓦片间接参数收尾管线。
    let Some(classify_pipeline) = pipeline_cache.get_compute_pipeline(pipeline.classify_pipeline)
    else {
        return; // 防御同帧缓存异常，首次管线等待不会再进入本系统作业列表。
    }; // 取得分类压缩管线。
    let Some(finalize_pipeline) = pipeline_cache.get_compute_pipeline(pipeline.finalize_pipeline)
    else {
        return; // 收尾管线未就绪时禁止留下未钳制的实例数量。
    }; // 取得间接参数收尾管线。
    let mut pass = render_context
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("gpu_grass_compute_pass"), // GPU分析器中把全部内容窗口的分类与收尾归为一个连续通道。
            ..Default::default()                   // 时间戳由固定查询资源显式写入各阶段边界。
        }); // 开始本帧草地计算通道。
    let timing_queries = timing_queries.as_deref(); // 统一转换为可选只读查询资源引用。
    let mut group_start = 0_usize; // Prepare已按内容profile与近中远顺序排序，从首组开始扫描。
    while group_start < jobs.jobs.len() {
        let vegetation_profile = jobs.jobs[group_start].vegetation_profile; // 当前连续组只处理普通草或麦田一种内容。
        let mut group_end = group_start + 1; // 至少包含当前首个作业。
        while group_end < jobs.jobs.len()
            && jobs.jobs[group_end].vegetation_profile == vegetation_profile
        {
            group_end += 1; // 扩张到下一种内容之前，确保四阶段只包围当前内容作业。
        }
        let profile_jobs = &jobs.jobs[group_start..group_end]; // 当前内容通常恰有近、中、远三个窗口。
        if let (Some(timing_queries), Some(query_index)) = (
            timing_queries, // 独立节点测试允许计时资源缺省。
            compute_timestamp_index(vegetation_profile, COMPUTE_BEGIN_TIMESTAMP), // 普通草零号槽或麦田四号槽。
        ) {
            record_compute_timestamp(timing_queries, &mut pass, query_index); // 记录当前内容粗瓦片工作的真实GPU开始边界。
        }
        pass.set_pipeline(cull_tiles_pipeline); // 第一阶段只按瓦片执行低成本常驻裁剪与视锥粗判。
        for job in profile_jobs {
            pass.set_bind_group(0, &job.tile_bind_group, &[job.uniform_offset]); // 真实tile_dispatch在本次直接派发中只作为可写Storage。
            pass.dispatch_workgroups(job.tile_cull_groups, 1, 1); // 每六十四瓦片一个工作组。
        } // 完成当前内容近中远活跃瓦片列表生成。
        pass.set_pipeline(finalize_tiles_pipeline); // 第二阶段把GPU追加计数变成合法DispatchIndirectArgs。
        for job in profile_jobs {
            pass.set_bind_group(0, &job.tile_bind_group, &[job.uniform_offset]); // 复用真实派发状态并写入合法XYZ参数。
            pass.dispatch_workgroups(1, 1, 1); // 每草窗一个线程发布活跃瓦片数与扫描候选统计。
        } // 完成当前内容细分类间接派发参数生成。
        if let (Some(timing_queries), Some(query_index)) = (
            timing_queries, // 查询资源存在时才记录当前内容边界。
            compute_timestamp_index(vegetation_profile, TILE_CULL_END_TIMESTAMP), // 当前内容四槽组第二项。
        ) {
            record_compute_timestamp(timing_queries, &mut pass, query_index); // 瓦片耗时包含派发参数生成。
        }
        pass.set_pipeline(classify_pipeline); // 第三阶段只扫描GPU判定为可能可见的当前内容瓦片候选。
        for job in profile_jobs {
            pass.set_bind_group(0, &job.classify_bind_group, &[job.uniform_offset]); // binding7改绑独立Storage，使真实tile_dispatch在本次派发中只有Indirect用法。
            pass.dispatch_workgroups_indirect(&job.tile_dispatch, 0); // GPU直接决定本帧工作组X，CPU不回读也不等待。
        } // 完成当前内容近中远活跃候选分类压缩。
        if let (Some(timing_queries), Some(query_index)) = (
            timing_queries, // 查询资源存在时才记录当前内容边界。
            compute_timestamp_index(
                vegetation_profile,               // 普通草、麦田等稳定内容编号。
                CANDIDATE_CLASSIFY_END_TIMESTAMP, // 当前内容逐株分类阶段结束偏移。
            ),
        ) {
            record_compute_timestamp(timing_queries, &mut pass, query_index); // 记录当前内容全部候选分类完成边界。
        }
        pass.set_pipeline(finalize_pipeline); // 当前内容全部分类dispatch完成后进入间接数量收尾。
        for job in profile_jobs {
            pass.set_bind_group(0, &job.classify_bind_group, &[job.uniform_offset]); // 收尾入口同样不访问binding7，继续使用隔离绑定组。
            pass.dispatch_workgroups(1, 1, 1); // 每草窗只需一个线程钳制实例数并记录溢出。
        } // 完成当前内容近中远间接参数收尾。
        if let (Some(timing_queries), Some(query_index)) = (
            timing_queries, // 查询资源存在时才记录当前内容最终边界。
            compute_timestamp_index(
                vegetation_profile,             // 当前连续作业组的稳定内容编号。
                COMPUTE_FINALIZE_END_TIMESTAMP, // 当前内容四阶段末尾偏移。
            ),
        ) {
            record_compute_timestamp(timing_queries, &mut pass, query_index); // 与当前内容开始边界组成独立Compute总耗时。
        }
        group_start = group_end; // 推进到下一种内容；不存在的profile不会写任何查询槽。
    }
    // 系统返回后根图继续执行相机驱动和各玩法相机子图。
}
