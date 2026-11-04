//! 规划缓存与完整布局的有界后台析构；一个任务和一个固定待处理集合，不建立任务链。
use super::resources::{Completion, LayeredCollisionRuntime, Layout}; // 仅移动已失去发布权的所有权。
use bevy::tasks::{AsyncComputeTaskPool, TaskPool, futures::check_ready}; // 主帧只非阻塞轮询。
use std::sync::Arc; // 旧布局共享引用在后台归还。

pub(super) fn busy(state: &LayeredCollisionRuntime) -> bool {
    // 退役期间不允许新布局或烘焙继续增长内存。
    state.retire_task.is_some()
        || state.retired.applied.is_some()
        || state.retired.candidate.is_some()
        || state.retired.completion.is_some() // 固定字段检查，与地图规模无关。
} // 已完成CPU结果可以留在原单飞任务槽，不另建完成队列。

pub(super) fn applied(state: &mut LayeredCollisionRuntime, layout: Option<Arc<Layout>>) {
    // 发布替换或换图退出。
    if let Some(layout) = layout {
        // 空旧状态不会添加任务。
        assert!(
            state.retired.applied.is_none(),
            "退役期间不得采用新的碰撞布局"
        ); // 生产背压保证唯一旧版。
        state.retired.applied = Some(layout); // O(1)转移，不遍历缓存组。
    } // 保留后台实际释放机会。
} // 不影响仍有效的当前布局。

pub(super) fn candidate(state: &mut LayeredCollisionRuntime, layout: Option<Layout>) {
    // 需求换代退出等待IO的候选。
    if let Some(layout) = layout {
        // 重复取消已取走候选时不新增负载。
        assert!(
            state.retired.candidate.is_none(),
            "退役期间不得生成新的碰撞候选"
        ); // 不以Vec隐藏积压。
        state.retired.candidate = Some(layout); // 只移动Map/Vec头及Arc。
    } // 真正析构由poll派发。
} // 原CPU任务仍由旧槽持有。

pub(super) fn completion(state: &mut LayeredCollisionRuntime, result: Completion) {
    // 迟到任务、承托拒收或发布失败。
    assert!(
        state.retired.completion.is_none(),
        "退役期间不得接收第二个碰撞完成值"
    ); // 单飞+接收前背压保证固定上限。
    state.retired.completion = Some(result); // 完整Cooked/Layout在后台销毁。
} // 不给拒收结果任何发布机会。

pub(super) fn poll(state: &mut LayeredCollisionRuntime) {
    // 游戏和离场回收系统都调用。
    if busy(state) {
        poll_on_pool(state, AsyncComputeTaskPool::get());
    } // 空状态不初始化或访问全局池，兼容无窗口生命周期夹具。
} // 运行时复用现有CPU池。

fn poll_on_pool(state: &mut LayeredCollisionRuntime, pool: &TaskPool) {
    // 测试使用独立池验证固定槽，不改全局池。
    if state.retire_task.as_mut().and_then(check_ready).is_some() {
        state.retire_task = None;
    } // 已完成任务仅归还轻量句柄。
    if state.retire_task.is_none() && busy(state) {
        // 同时至多一个后台析构任务。
        let payload = std::mem::take(&mut state.retired); // 固定三个所有权槽一次移动。
        state.retire_task = Some(pool.spawn(async move {
            drop(payload);
        })); // 大树/组/网格只在后台释放，不await上一任务形成无界链。
    } // 待处理集合最多保留一次换图退出值。
} // 不阻塞主帧、不提前宣称释放完成。

#[cfg(test)] // 固定槽背压与换图排空不依赖真实图形资源。
mod tests {
    // 实际缓存内容的发布/失败覆盖在runtime_planning_cache_tests。
    use super::*; // 直接审计生产队列。
    use bevy::{
        ecs::world::CommandQueue,
        prelude::*,
        tasks::{TaskPoolBuilder, block_on},
    }; // 阻塞等待仅在测试线程。

    fn layout() -> Arc<Layout> {
        // 生命周期夹具只验证所有权，不重复规划。
        Arc::new(Layout {
            planning_cache: None,
            chunks: Default::default(),
            tiles: Default::default(),
            supported_polygon: Vec::new(),
            supported_rows: None,
            elapsed_ms: 0.0,
            rebuilt_blocks: 0,
            reused_blocks: 0,
        }) // 字段均为空，不提供伪地面。
    } // Arc弱引用可观察最后所有者是否真正释放。

    #[test] // 任务未回收时，重复换图只能增加一个固定待处理布局。
    fn retirement_keeps_one_task_and_fixed_pending_slots_across_resets() {
        // 禁止任务链或历史Vec。
        let pool = TaskPoolBuilder::new().num_threads(1).build(); // 一个真实后台worker。
        let mut state = LayeredCollisionRuntime::default(); // 空运行时。
        let old = layout(); // 模拟被新版本替换的旧布局。
        let old_weak = Arc::downgrade(&old); // 不延长其存活。
        applied(&mut state, Some(old)); // 主线程仅移动所有权。
        assert!(old_weak.upgrade().is_some() && busy(&state)); // 派发前不提前析构。
        poll_on_pool(&mut state, &pool); // 启动唯一释放任务。
        assert!(state.retire_task.is_some() && state.retired.applied.is_none()); // 一次移动整个固定集合。
        let current = layout(); // 模拟仍有效的新采用布局。
        let current_weak = Arc::downgrade(&current); // 监控换图释放。
        state.applied_layout = Some(current); // 成功发布后已有当前版本，此时不再创建新候选。
        let world = World::new(); // 清理空形状集合不需要图形。
        let mut queue = CommandQueue::default(); // 仅构造Commands入口。
        for _ in 0..8 {
            super::super::lifecycle::reset(&mut Commands::new(&mut queue, &world), &mut state);
        } // 频繁切图不能累计旧layout。
        assert!(state.retire_task.is_some() && state.retired.applied.is_some()); // 仍只有一个任务和一个待释放当前版本。
        assert!(current_weak.upgrade().is_some()); // 待处理值没有在主帧被释放。
        block_on(state.retire_task.take().unwrap()); // 测试线程等待第一任务真正完成。
        assert!(old_weak.upgrade().is_none()); // 第一布局实际释放。
        poll_on_pool(&mut state, &pool); // 第二固定集合现在才允许派发。
        block_on(state.retire_task.take().unwrap()); // 测试线程等待第二任务。
        assert!(current_weak.upgrade().is_none() && !busy(&state)); // 离场遗留完整排空，未建立历史链。
    } // 实际析构运行在专用任务池。
} // 自动测试不代表实机峰值内存测量。
