//! GPU候选回读消费、逐字节比较与有界超时状态转换。

use super::super::resources::{
    PendingBladeDensityState, VegetationGpuMipCoordinator, VegetationGpuMipInbox,
    VegetationGpuMipOutcome, VegetationGpuMipRequest,
}; // 纯函数只推进当前票据候选状态，不访问ECS世界或图片资产。

/// 候选GPU作业允许等待的主世界更新次数。
const GPU_MIP_TIMEOUT_UPDATES: u64 = 1_800; // 正常渲染只需数帧，设备丢失时约三十秒后明确托底旧版本。

/// 消费RenderApp异步结果并只更新当前票据、版本和请求层。
pub(super) fn consume_gpu_results(
    coordinator: &mut VegetationGpuMipCoordinator, // 当前批次是所有结果唯一可采用目标。
    request: &mut VegetationGpuMipRequest,         // 当前层完成后清空以允许发布下一层。
    inbox: &mut VegetationGpuMipInbox,             // 本帧一次排空，迟到结果不积压。
) {
    for result in inbox.results.drain(..) {
        let Some(batch) = coordinator.pending.as_mut() else {
            continue; // 已取消批次的迟到回调不重建任何状态。
        };
        if batch.ticket != result.ticket || batch.revision != result.revision {
            continue; // 票据或版本不同的回调严格视为过时。
        }
        let Some(pending) = batch.layers.get_mut(&result.layer) else {
            continue; // 绑定集合变化后不存在的层不得进入当前批次。
        };
        if pending.state != PendingBladeDensityState::Requested {
            continue; // 重复回调或非当前串行层不得倒退阶段。
        }
        if request.job.as_ref().is_some_and(|job| {
            job.ticket == result.ticket
                && job.revision == result.revision
                && job.layer == result.layer // 只清除确实完成的当前请求。
        }) {
            request.job = None; // 下一次发布可以选择BTreeMap中的下一等待层。
        }
        match result.outcome {
            VegetationGpuMipOutcome::Complete(actual) => {
                match compare_gpu_mip_bytes(&pending.expected, &actual) {
                    Ok(()) => {
                        pending.state = PendingBladeDensityState::Validated; // 本层完整链严格相等后才进入通过态。
                    }
                    Err(reason) => {
                        batch.failure = Some(format!(
                            "vegetation GPU mip parity failed for revision {} layer {:?}: {reason}",
                            batch.revision, result.layer
                        )); // 无容差差异取消整个跨层批次。
                    }
                }
            }
            VegetationGpuMipOutcome::Failed(reason) => {
                batch.failure = Some(format!(
                    "vegetation GPU mip runtime failed for revision {} layer {:?}: {reason}",
                    batch.revision, result.layer
                )); // 映射、资源或解码失败统一保留上一活动布局。
            }
        }
    }
}

/// 比较全部紧凑RGBA Mip字节并返回首个明确差异。
pub(super) fn compare_gpu_mip_bytes(expected: &[u8], actual: &[u8]) -> Result<(), String> {
    if expected.len() != actual.len() {
        return Err(format!(
            "byte length differs: expected {}, got {}",
            expected.len(),
            actual.len()
        )); // 长度差异优先报告，避免后续索引歧义。
    }
    if let Some((index, (expected_byte, actual_byte))) = expected
        .iter()
        .zip(actual)
        .enumerate()
        .find(|(_, (expected_byte, actual_byte))| expected_byte != actual_byte)
    {
        return Err(format!(
            "first byte mismatch at {index}: expected {expected_byte}, got {actual_byte}"
        )); // 首个RGBA或Mip差异足以拒绝候选且便于定位。
    }
    Ok(()) // 所有基础级和GPU生成子级逐字节完全相同。
}

/// 在候选等待过久时写入一次整批失败原因。
pub(super) fn mark_timeout_if_needed(coordinator: &mut VegetationGpuMipCoordinator) {
    let update_index = coordinator.update_index; // 避免借用批次时再次访问协调器。
    let Some(batch) = coordinator.pending.as_mut() else {
        return; // 稳定布局没有超时状态。
    };
    if batch.failure.is_none()
        && update_index.wrapping_sub(batch.started_update) > GPU_MIP_TIMEOUT_UPDATES
    {
        batch.failure = Some(format!(
            "vegetation GPU mip batch ticket {} revision {} exceeded {} updates",
            batch.ticket, batch.revision, GPU_MIP_TIMEOUT_UPDATES
        )); // 超时只拒绝候选，不删除当前活动缓存。
    }
}
