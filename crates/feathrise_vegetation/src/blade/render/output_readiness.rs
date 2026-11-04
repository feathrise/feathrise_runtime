//! Queue 只检查输出是否已分配，Draw 在 Compute 后严格检查本帧有效性，避免恢复帧额外闪烁。
pub(super) fn queue_output_is_available(instance_count: u32, allocated: bool) -> bool {
    // Queue 早于 Prepare，不能把上一帧有效性当本帧排队门。
    instance_count > 0 && allocated // 最终命令仍拒绝任何未完成或过期结果。
} // 不修改地图、高度或视图身份校验。
pub(super) fn draw_output_is_current(instance_count: u32, output_valid: bool) -> bool {
    // 相机执行前 Compute 已通过本帧正式采用检查。
    instance_count > 0 && output_valid // 即使已经排队，过期结果与资源等待仍必须隐藏。
} // 颜色、阴影与预通道共用严格门。

#[cfg(test)] // 真实引擎阶段回归不创建图形设备。
mod tests {
    // 检查 Queue→Prepare→Render 的跨帧恢复。
    use super::{draw_output_is_current, queue_output_is_available}; // 使用生产排队与绘制条件。
    use bevy::prelude::*; // 最小 ECS 世界和状态。
    use bevy::render::{Render, RenderSystems}; // 直接采用官方渲染阶段顺序。
    #[derive(Resource)] // 只模拟 GPU 准备结果，不模拟渲染算法。
    struct Frame {
        // 区分历史有效性与本帧完成状态。
        allocated: bool, // 持久输出缓冲是否存在。
        valid: bool,     // Queue 观察到的上一帧结果。
        completes: bool, // 当前 Prepare 的最终合法结果。
        queued: bool,    // 本帧候选阶段项。
        drawn: bool,     // 本帧实际绘制许可。
    } // 完成固定测试状态。
    fn queue(mut frame: ResMut<Frame>) {
        // 对应三条正式排队路径。
        frame.queued = queue_output_is_available(256, frame.allocated); // 不提前读取上一帧 valid。
    } // 完成测试排队。
    fn prepare(mut frame: ResMut<Frame>) {
        // 模拟身份和资源核对后的作业结果。
        frame.valid = frame.completes; // 未完成仍严格无效。
    } // 完成测试准备。
    fn draw(mut frame: ResMut<Frame>) {
        // 对应 Compute 后最终 RenderCommand。
        frame.drawn = frame.queued && draw_output_is_current(256, frame.valid); // 已排队不代表允许绘制。
    } // 完成最终门测试。
    #[test] // 恢复必须当帧生效，失败仍禁止旧输出。
    fn recovered_output_draws_in_the_same_render_frame() {
        // 真实调度关系重现移动导致的就绪边沿。
        let mut world = World::new(); // 无窗口世界。
        world.insert_resource(Frame {
            allocated: true,
            valid: false,
            completes: false,
            queued: false,
            drawn: false,
        }); // 上一帧分页等待后的持久窗口。
        let mut schedule = Render::base_schedule(); // 使用官方 Queue→Prepare→Render 阶段。
        schedule.add_systems(queue.in_set(RenderSystems::QueueMeshes)); // Queue 先读取历史状态。
        schedule.add_systems(prepare.in_set(RenderSystems::PrepareBindGroups)); // 本帧晚些时候准备完成。
        schedule.add_systems(draw.in_set(RenderSystems::Render)); // 渲染时核对最终当前状态。
        for completes in [true, false, true, true, false, true] {
            // 覆盖反复移动、缺页与恢复。
            world.resource_mut::<Frame>().completes = completes; // 设置本帧准备结果。
            schedule.run(&mut world); // 实际执行引擎阶段依赖。
            let frame = world.resource::<Frame>(); // 读取本帧结果。
            assert!(frame.queued); // 已分配窗口始终可准备候选阶段项。
            assert_eq!(frame.drawn, completes); // 成功当帧绘制，失败严格隐藏，不多等一帧。
        } // 完成连续跨帧回归。
    } // 完成恢复断帧回归。
    #[test] // 保留首次初始化、抑制及失败保护。
    fn missing_or_suppressed_output_stays_hidden() {
        // 已分配不等于允许最终绘制。
        assert!(!queue_output_is_available(256, false)); // 缓冲未创建时等待。
        assert!(!queue_output_is_available(0, true)); // 空候选不排队。
        assert!(!draw_output_is_current(256, false)); // 过期地图/高度/视图结果不绘制。
        assert!(!draw_output_is_current(0, true)); // 本帧主动抑制不得显示历史草。
    } // 完成安全边界回归。
} // 不引入任何逐株 CPU 路径。
