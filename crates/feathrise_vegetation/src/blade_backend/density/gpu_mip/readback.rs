//! 候选完整Mip的版本化非阻塞异步映射与主世界发布。

use std::sync::{
    Arc,              // map_async回调独立持有作业和GPU缓冲直到完成。
    atomic::Ordering, // COPY_RECORDED到MAPPING到COMPLETE的无锁状态转换。
};

use bevy::prelude::{Res, ResMut}; // 读取当前作业、共享队列并访问主世界。
use bevy::render::MainWorld;
use bevy::render::render_resource::MapMode; // staging缓冲只执行异步只读映射。 // ExtractSchedule官方跨世界入口。

use super::super::resources::{
    VegetationGpuMipInbox, VegetationGpuMipOutcome, VegetationGpuMipResult,
};
use super::resources::{
    JOB_COMPLETE, JOB_COPY_RECORDED, JOB_MAPPING, VegetationGpuMipRenderState,
    VegetationGpuMipShared,
}; // 当前Arc作业和回调完成队列。 // 主世界收件箱与版本化成功/失败载荷。

/// 下一帧对已提交复制发起map_async，并发布此前已经完成的回调结果。
pub(super) fn begin_maps_and_publish_vegetation_gpu_mips(
    mut main_world: ResMut<MainWorld>, // 只在ExtractSchedule写主世界私有收件箱。
    state: Res<VegetationGpuMipRenderState>, // 当前作业可能仍在READY、复制或映射阶段。
    shared: Res<VegetationGpuMipShared>, // 回调完成队列不等待GPU。
) {
    if let Some(job) = state.current.as_ref()
        && job
            .state
            .compare_exchange(
                JOB_COPY_RECORDED, // 上一根图已经录入并提交全部复制。
                JOB_MAPPING,       // 从现在起禁止节点或系统重复使用该staging缓冲。
                Ordering::AcqRel,  // 获取节点发布的命令完成边界。
                Ordering::Acquire, // 非目标状态直接继续发布已完成队列。
            )
            .is_ok()
    {
        let callback_job = Arc::clone(job); // 回调结束前保持纹理、计划和缓冲全部存活。
        let callback_buffer = job.readback.clone(); // 成功时读取并在释放视图后unmap。
        let map_request_buffer = callback_buffer.clone(); // 单独句柄发起map，原句柄移入静态回调。
        let callback_shared = (*shared).clone(); // 回调只持有Arc内部短队列。
        map_request_buffer
            .slice(..)
            .map_async(MapMode::Read, move |result| {
                let outcome = match result {
                    Ok(()) => {
                        let decoded = callback_buffer.slice(..).get_mapped_range()
                            .map_err(|error| format!("get_mapped_range failed: {error:?}"))
                            .and_then(|mapped| callback_job.readback_plan.decode(&mapped)); // 范围视图先释放，才可解除映射。
                        callback_buffer.unmap(); // 解除映射后GPU缓冲生命周期可正常结束。
                        match decoded {
                            Ok(bytes) => VegetationGpuMipOutcome::Complete(Arc::from(bytes)), // 主世界稍后执行无容差比较。
                            Err(reason) => VegetationGpuMipOutcome::Failed(reason), // 解码不完整拒绝整个候选批次。
                        }
                    }
                    Err(error) => {
                        VegetationGpuMipOutcome::Failed(format!("map_async failed: {error:?}"))
                    } // 映射失败不修改上一活动布局。
                };
                callback_shared.publish(VegetationGpuMipResult {
                    ticket: callback_job.identity.ticket, // 取消后的迟到回调由主世界票据拒绝。
                    revision: callback_job.identity.revision, // 换版后旧结果不得倒退缓存。
                    layer: callback_job.identity.layer,   // 串行层身份必须完全一致。
                    outcome,                              // 完整紧凑字节或明确失败原因。
                });
                callback_job.state.store(JOB_COMPLETE, Ordering::Release); // 同一作业生命周期正式结束。
            }); // 调用立即返回，运行时没有同步poll、wait或通道阻塞。
    }

    let completed = shared.drain(); // 只取回回调已经完成的短结果队列。
    if completed.is_empty() {
        return; // 常见帧不借用主世界收件箱。
    }
    if let Some(mut inbox) = main_world.get_resource_mut::<VegetationGpuMipInbox>() {
        inbox.results.extend(completed); // 主世界下一Update验证当前票据和逐字节内容。
    } else {
        main_world.insert_resource(VegetationGpuMipInbox {
            results: completed, // 防御首次提取前资源缺失并保留全部完成结果。
        });
    }
}
