//! 有界worker、稳定结果、失败排空与低核回退的无窗口测试。
use super::*; // 正式有界调度入口。
use bevy::tasks::{TaskPoolBuilder, block_on}; // block_on仅存在于测试驱动，不进入主帧。
use std::sync::atomic::{AtomicUsize, Ordering}; // 观测并行活动和真实完成。

#[test] // 配额不随块数无限增长。
fn worker_budget_reserves_capacity_and_handles_tiny_batches() {
    // 测试纯配置，无线程启动。
    assert_eq!(worker_count(1, 512, 4), 1); // 单核不能等待另一个worker。
    assert_eq!(worker_count(2, 512, 4), 1); // 两线程池保留其他任务余量。
    assert_eq!(worker_count(3, 512, 4), 2); // 三线程允许两个worker。
    assert_eq!(worker_count(64, 512, 100), 4); // 请求不能绕过上限。
    assert_eq!(worker_count(8, 1, 4), 1); // 小工作量不并行。
    assert_eq!(worker_count(8, 0, 0), 1); // 空输入仍返回完整空集合。
} // 不改变场景预算。

#[test] // 单线程池也能执行后台协调器，且结果顺序不受调度影响。
fn worker_results_match_serial_on_one_two_and_four_workers() {
    // 使用独立测试池，不改全局池。
    for workers in [1, 2, 4] {
        // 三种并发配置共用同一函数。
        let pool = Arc::new(
            TaskPoolBuilder::new()
                .num_threads(if workers == 1 { 1 } else { workers + 1 })
                .build(),
        ); // 单核回退也实际执行。
        let worker_pool = Arc::clone(&pool); // 协调器持有池直到所有任务结束。
        let task = pool.spawn(async move {
            map(&worker_pool, (0..32).collect(), workers, |i| Ok(i * i)).await
        }); // 与生产相同后台等待结构。
        assert_eq!(
            block_on(task).unwrap(),
            (0..32).map(|i| i * i).collect::<Vec<_>>()
        ); // 顺序与串行完全一致。
    } // Task先结束，池后释放。
} // 不靠主线程运行工作函数。

#[test] // 真并行必须有同时工作的多个worker，错误也必须等其他worker结束。
fn workers_overlap_and_failure_drains_all_started_groups() {
    // 有界短睡眠仅模拟后台计算耗时。
    let pool = TaskPoolBuilder::new().num_threads(5).build(); // 四worker加一份余量。
    let active = Arc::new(AtomicUsize::new(0)); // 当前工作函数活动数。
    let peak = Arc::new(AtomicUsize::new(0)); // 最大同时活动数。
    let count = Arc::clone(&active); // 闭包共享观测器。
    let high = Arc::clone(&peak); // 不改变生产调度。
    let output = block_on(map(&pool, (0..32).collect(), 4, move |i| {
        // 大于小批次阈值。
        let now = count.fetch_add(1, Ordering::SeqCst) + 1; // 当前worker进入。
        high.fetch_max(now, Ordering::SeqCst); // 记录真实重叠。
        std::thread::sleep(std::time::Duration::from_millis(2)); // 给其余worker执行机会，无主帧参与。
        count.fetch_sub(1, Ordering::SeqCst); // 当前worker离开。
        Ok(i) // 无副作用纯数据结果。
    }))
    .unwrap(); // 完整成功。
    assert_eq!(output, (0..32).collect::<Vec<_>>()); // 保持顺序。
    assert!((2..=4).contains(&peak.load(Ordering::SeqCst))); // 不是顺序await导致的伪并行。
    assert_eq!(active.load(Ordering::SeqCst), 0); // 返回不残留worker。
    for panic_worker in [false, true] {
        // 普通错误与可展开panic都必须排空其他分组。
        let completed = Arc::new(AtomicUsize::new(0)); // 非失败组完成计数。
        let observed = Arc::clone(&completed); // 跨worker只读共享所有权。
        let result = block_on(map(&pool, (0..16).collect(), 2, move |i| {
            // 偶数第一项失败，奇数worker继续。
            if i == 0 {
                // 第一分组提前失败。
                assert!(!panic_worker, "测试worker异常"); // 生产catch_unwind捕获，不让整个批次提前归还。
                return Err("测试构建失败".into()); // 正常失败同样验证。
            } // 第二分组仍需实际完成。
            std::thread::sleep(std::time::Duration::from_millis(1)); // 非失败组完成不被取消。
            observed.fetch_add(1, Ordering::SeqCst); // 统计所有奇数项。
            Ok(i) // 不返回半成品给发布层。
        })); // 等所有子任务后才返回错误。
        assert!(result.is_err()); // 错误不会被吞掉。
        assert_eq!(completed.load(Ordering::SeqCst), 8); // 证明没有提前drop其余CPU任务释放并发槽。
    } // 错误后允许下一批，不留后台漏跑工作。
} // 所有等待仅为测试专用。
