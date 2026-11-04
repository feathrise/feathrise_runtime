//! 按真实时间对已有DLSS/FSR累计Present采样，包含生成帧，不推算固定倍率。

use super::resources::PresentationFpsSample; // 采样只修改固定大小Local状态。
use super::types::PresentationCounter; // 完整后端身份与实际计数作为同一份固定大小输入。

pub(super) fn sample_present_fps(
    // 每主世界帧执行常量次算术，无分配、GPU查询或等待。
    sample: &mut PresentationFpsSample, // 保存同一计数来源的相邻采样边界。
    now: f64,                           // Time<Real>秒数，不受游戏暂停或时间缩放影响。
    source: Option<PresentationCounter>, // 当前有效代理的身份与实际累计Present。
) -> Option<f64> {
    // 尚未收集完整半秒时显示占位值。
    let Some(PresentationCounter { identity, count }) = source.filter(|_| now.is_finite()) else {
        // 代理失效时立即清除过期FPS。
        *sample = PresentationFpsSample::default(); // 下次恢复从新计数重新开始。
        return None; // 不沿用旧代理或上次游戏的呈现速率。
    }; // 有效来源解析结束。
    let reset = sample.baseline.is_none_or(|(_, _, previous)| previous != identity) // 后端、代次或尺寸变化均不能跨交换链相减。
        || sample.last_observed.is_some_and(|(time, previous)| count < previous || now <= time || now - time > 2.0); // 计数回零、时钟异常或长暂停均重建基线。
    if reset {
        // 首次启动或来源变化只登记起点。
        sample.baseline = Some((now, count, identity)); // 当前计数不得被算成当前半秒内产生的帧。
        sample.fps = None; // 明确撤销旧数字，等待新窗口。
    } else if let Some((time, previous, _)) = sample.baseline {
        // 同一来源才允许计算速率。
        let elapsed = now - time; // 使用实际经过时间，不假定定时器恰好半秒触发。
        if elapsed >= 0.5 {
            // 固定低频样本平滑异步呈现的短时成批更新。
            sample.fps = Some((count - previous) as f64 / elapsed); // 唯一公式为实际Present增量除以真实秒数；不乘2。
            sample.baseline = Some((now, count, identity)); // 窗口首尾连续，不遗漏或重复计数。
        } // 未满半秒时保留最近完整区间的测量。
    } // 采样更新结束。
    sample.last_observed = Some((now, count)); // 每帧检查倒退，防止两次文本刷新之间漏掉重置。
    sample.fps // 零增量自然显示0，不沿用停顿之前的高FPS。
} // sample_present_fps结束。

#[cfg(test)] // 直接测试生产采样函数，无GPU或SDK启动。
mod tests {
    // 回归实际速率、计数重置、尺寸变化和暂停恢复。
    use super::*; // 使用生产采样器与状态。
    use crate::types::{PresentationBackend, PresentationIdentity}; // 测试两种来源以及同尺寸换代。

    fn fsr_counter(extent: (u32, u32), count: u64) -> Option<PresentationCounter> {
        // 原FSR用例保留尺寸与计数输入。
        Some(PresentationCounter {
            // 使用与生产相同的固定身份。
            identity: PresentationIdentity {
                backend: PresentationBackend::Fsr,
                epoch: 0,
                extent,
            }, // FSR以已有尺寸/计数变化识别重建。
            count, // 测试指定实际Present总数。
        }) // 返回有效代理来源。
    } // fsr_counter结束。

    fn dlss_counter(epoch: u64, count: u64) -> Option<PresentationCounter> {
        // 不需要渲染FPS或SDK运行库。
        Some(PresentationCounter {
            // 模拟已发布的原生DLSS计数。
            identity: PresentationIdentity {
                backend: PresentationBackend::Dlss,
                epoch,
                extent: (1920, 1080),
            }, // 固定尺寸用于独立验证epoch变化。
            count, // 单次输出可以1帧、2帧或0帧，计数是唯一速率依据。
        }) // 返回可信DLSS来源。
    } // dlss_counter结束。

    #[test] // 73个实际Present发生在半秒内，应显示146，而不是主循环FPS的固定倍数。
    fn uses_elapsed_time_and_actual_count_including_stalls() {
        // 不需要任何基础FPS输入。
        let mut sample = PresentationFpsSample::default(); // 新采样器。
        assert_eq!(
            sample_present_fps(&mut sample, 0.0, fsr_counter((1920, 1080), 200)),
            None
        ); // 历史200张不能冒充当前输出。
        assert_eq!(
            sample_present_fps(&mut sample, 0.5, fsr_counter((1920, 1080), 273)),
            Some(146.0)
        ); // 验证真实计数/秒。
        assert_eq!(
            sample_present_fps(&mut sample, 1.0, fsr_counter((1920, 1080), 273)),
            Some(0.0)
        ); // 没有新呈现必须变为0。
    } // 实际速率测试结束。
    #[test] // reset可能发生在两个半秒刷新之间，必须每帧捕获。
    fn counter_reset_between_refreshes_discards_old_interval() {
        // 回零后迅速增长也不能与旧基线相减。
        let mut sample = PresentationFpsSample::default(); // 新采样器。
        sample_present_fps(&mut sample, 0.0, fsr_counter((1920, 1080), 100)); // 旧代理基线。
        sample_present_fps(&mut sample, 0.2, fsr_counter((1920, 1080), 120)); // 尚未刷新文本。
        assert_eq!(
            sample_present_fps(&mut sample, 0.25, fsr_counter((1920, 1080), 0)),
            None
        ); // 同尺寸重建清空基线。
        assert_eq!(
            sample_present_fps(&mut sample, 0.75, fsr_counter((1920, 1080), 80)),
            Some(160.0)
        ); // 只计算新代理的80张。
    } // 计数重置测试结束。
    #[test] // 尺寸变化即使新计数更大也必须重建基线。
    fn resize_and_missing_source_clear_measurement() {
        // 失效期间不能显示之前的帧生成速率。
        let mut sample = PresentationFpsSample::default(); // 新采样器。
        sample_present_fps(&mut sample, 0.0, fsr_counter((1920, 1080), 0)); // 首个规格。
        sample_present_fps(&mut sample, 0.5, fsr_counter((1920, 1080), 80)); // 产生有效160FPS。
        assert_eq!(
            sample_present_fps(&mut sample, 0.6, fsr_counter((1280, 720), 90)),
            None
        ); // 新规格撤销旧FPS。
        assert_eq!(sample_present_fps(&mut sample, 0.7, None), None); // 代理消失仍无样本。
        assert_eq!(
            sample_present_fps(&mut sample, 0.8, fsr_counter((1280, 720), 130)),
            None
        ); // 恢复不能包含失效期间的帧。
    } // 来源切换测试结束。
    #[test] // 最小化或长暂停回来不能把旧边界混入新FPS。
    fn long_pause_starts_fresh_sample() {
        // 使用真实时间而非游戏缩放时间。
        let mut sample = PresentationFpsSample::default(); // 新采样器。
        sample_present_fps(&mut sample, 0.0, fsr_counter((1920, 1080), 0)); // 暂停前基线。
        assert_eq!(
            sample_present_fps(&mut sample, 5.0, fsr_counter((1920, 1080), 100)),
            None
        ); // 超过两秒的采样间断重新预热。
        assert_eq!(
            sample_present_fps(&mut sample, 5.5, fsr_counter((1920, 1080), 175)),
            Some(150.0)
        ); // 只采用恢复后的半秒。
    } // 暂停恢复测试结束。

    #[test] // DLSS实际输出在1帧和2帧之间变化时不得套固定倍率。
    fn dlss_uses_mixed_actual_present_counts() {
        // 30个宿主帧里15次双帧、15次单帧，实际共45张。
        let mut sample = PresentationFpsSample::default(); // 初始无历史样本。
        assert_eq!(
            sample_present_fps(&mut sample, 0.0, dlss_counter(1, 100)),
            None
        ); // 首次只建立起点。
        assert_eq!(
            sample_present_fps(&mut sample, 0.5, dlss_counter(1, 145)),
            Some(90.0)
        ); // 真实45张/半秒，不假设30个宿主帧一定输出60张。
        assert_eq!(
            sample_present_fps(&mut sample, 1.0, dlss_counter(1, 175)),
            Some(60.0)
        ); // 连续单帧也应显示实际60FPS。
    } // DLSS混合实际输出测试结束。

    #[test] // 同尺寸重建可能保留单调累计数，必须由交换链epoch清除旧样本。
    fn dlss_epoch_change_discards_old_interval_without_counter_reset() {
        // 验证新后端发布的代次被实际消费。
        let mut sample = PresentationFpsSample::default(); // 初始采样状态。
        sample_present_fps(&mut sample, 0.0, dlss_counter(1, 100)); // 旧交换链基线。
        assert_eq!(
            sample_present_fps(&mut sample, 0.5, dlss_counter(1, 160)),
            Some(120.0)
        ); // 已形成有效旧速率。
        assert_eq!(
            sample_present_fps(&mut sample, 0.75, dlss_counter(2, 180)),
            None
        ); // 代次变化立即撤销旧数字。
        assert_eq!(
            sample_present_fps(&mut sample, 1.25, dlss_counter(2, 220)),
            Some(80.0)
        ); // 新区间只包含40张。
    } // 交换链换代测试结束。

    #[test] // 两个SDK都可能从0开始且计数更大，后端身份必须独立。
    fn backend_change_discards_previous_counter_even_at_same_extent() {
        // 特意使用相同尺寸证明不是尺寸变化造成重置。
        let mut sample = PresentationFpsSample::default(); // 初始采样状态。
        sample_present_fps(&mut sample, 0.0, fsr_counter((1920, 1080), 100)); // 第一来源为FSR。
        assert_eq!(
            sample_present_fps(&mut sample, 0.5, fsr_counter((1920, 1080), 160)),
            Some(120.0)
        ); // 旧后端已有有效数字。
        assert_eq!(
            sample_present_fps(&mut sample, 0.75, dlss_counter(0, 200)),
            None
        ); // 相同尺寸/epoch且计数更大仍必须撤销。
        assert_eq!(
            sample_present_fps(&mut sample, 1.25, dlss_counter(0, 250)),
            Some(100.0)
        ); // 只采用DLSS自己的50张。
        assert_eq!(
            sample_present_fps(&mut sample, 1.5, fsr_counter((1920, 1080), 300)),
            None
        ); // 返回FSR同样重新预热。
    } // 后端切换测试结束。

    #[test] // 原生层失去可信Present后恢复不能包含失效期间的计数。
    fn dlss_invalid_source_restarts_sampling_when_ready_returns() {
        // ready=false在生产选择器中映射为None。
        let mut sample = PresentationFpsSample::default(); // 初始采样状态。
        sample_present_fps(&mut sample, 0.0, dlss_counter(1, 100)); // 首次有效起点。
        sample_present_fps(&mut sample, 0.5, dlss_counter(1, 160)); // 形成120FPS。
        assert_eq!(sample_present_fps(&mut sample, 0.75, None), None); // SDK状态或代理失效时清空。
        assert_eq!(
            sample_present_fps(&mut sample, 1.0, dlss_counter(1, 250)),
            None
        ); // 即使epoch未变也从恢复帧重新计时。
        assert_eq!(
            sample_present_fps(&mut sample, 1.5, dlss_counter(1, 280)),
            Some(60.0)
        ); // 仅恢复后的真实增量生效。
    } // 失效恢复测试结束。
} // tests结束。
