//! 近场按需并行门槛和真实TaskPool串并行结果确定性回归。

use std::collections::BTreeMap; // 大输入使用空上一活动集合。

use bevy::prelude::Vec2; // 构造唯一近场观察者位置。
use bevy::tasks::TaskPoolBuilder; // 创建隔离四线程池执行真实分片。

use crate::VegetationLayerId; // 构造两个独立近场层身份。

use super::super::collection::{
    // 直接验证纯策略与完整候选收集。
    NearFieldExecutionMode,
    NearFieldLayerWork,
    build_layer_work,
    build_parallel_batches,
    collect_with_task_pool,
    execution_mode,
}; // 完成并行测试接口导入。
use super::super::spatial_index::ExplicitPlacementSpatialIndexes; // 为大显式层建立真实索引。
use super::fixture::{explicit_layer, full_layout, placement}; // 复用合法层与布局fixture。

#[test] // 至少两个重层且总量足够时才允许按层并行。
fn execution_policy_keeps_light_or_single_heavy_work_serial() {
    // 纯策略不受机器计时抖动影响。
    let layers = [
        // 两个空层只提供稳定配置借用。
        explicit_layer(VegetationLayerId(1), Vec::new(), 4.0, 6.0), // 第一层。
        explicit_layer(VegetationLayerId(2), Vec::new(), 4.0, 6.0), // 第二层。
    ]; // 完成策略层数组。
    let single_heavy = [
        // 一个重层和一个轻层不得拆内层。
        work(&layers[0], 4_096), // 单层达到总门槛。
        work(&layers[1], 32),    // 第二层明显过轻。
    ]; // 完成单重层工作。
    assert_eq!(
        execution_mode(&single_heavy, 8),
        NearFieldExecutionMode::Serial
    ); // 保持串行。
    let low_total = [
        // 两层达到单层门槛但总量不足。
        work(&layers[0], 1_024), // 第一层恰好单层门槛。
        work(&layers[1], 1_024), // 第二层恰好单层门槛。
    ]; // 完成低总量工作。
    assert_eq!(
        execution_mode(&low_total, 8),
        NearFieldExecutionMode::Serial
    ); // 总量不足串行。
    let heavy = [
        // 两层和总量均达到门槛。
        work(&layers[0], 2_048), // 第一层贡献一半。
        work(&layers[1], 2_048), // 第二层补足总量。
    ]; // 完成双重层工作。
    assert_eq!(
        execution_mode(&heavy, 8),
        NearFieldExecutionMode::Parallel { workers: 2 }
    ); // 两层使用两worker。
    assert_eq!(execution_mode(&heavy, 2), NearFieldExecutionMode::Serial); // 半池仅一worker时串行。
} // 完成执行门槛回归。

#[test] // 有界批次必须把最重的前N层分散到不同Task。
fn parallel_batches_are_bounded_and_distribute_heavy_layers() {
    // 防止相邻重层被连续切片放入同一worker。
    let layers = (1..=5) // 构造五个稳定合法层供批次规划借用。
        .map(|id| explicit_layer(VegetationLayerId(id), Vec::new(), 4.0, 6.0)) // 空清单足以测试纯调度。
        .collect::<Vec<_>>(); // 收集完整层配置所有权。
    let estimates = [5_000, 4_000, 30, 20, 10]; // 前两层模拟相邻重工作，其余层很轻。
    let work_items = layers // 按稳定层ID构造原始相邻工作列表。
        .iter() // 借用全部五层。
        .zip(estimates) // 为每层注入固定工作量。
        .map(|(layer, checks)| work(layer, checks)) // 构造轻量工作描述。
        .collect::<Vec<_>>(); // 收集完整工作列表。
    let batches = build_parallel_batches(&work_items, 4); // 使用正式最多四worker规划批次。
    assert_eq!(batches.len(), 4); // 外层批次数严格不超过worker上限。
    assert!(batches.iter().all(|batch| !batch.is_empty())); // worker不超过层数时没有空Task。
    assert_eq!(batches[0][0].estimated_checks, 5_000); // 最重层进入第一个Task。
    assert_eq!(batches[1][0].estimated_checks, 4_000); // 次重层进入不同的第二个Task。
} // 完成有界负载分配回归。

#[test] // 真实ComputeTaskPool分片必须与串行结果逐项完全相等。
fn parallel_layer_collection_matches_serial_results() {
    // 结果不依赖worker完成顺序。
    let layers = [
        // 两个重层各含2048个附近候选。
        explicit_layer(
            // 构造第一重层。
            VegetationLayerId(1), // 第一层ID。
            (0..2_048) // 生成层内唯一ID。
                .map(|id| placement(id, 2.0, 2.0)) // 全部位于同一附近桶。
                .collect(), // 收集完整清单。
            9.0,                  // 九米进入圈。
            10.0,                 // 十米退出圈和单元边长。
        ), // 完成第一层。
        explicit_layer(
            // 构造第二重层。
            VegetationLayerId(2), // 第二层ID。
            (0..2_048) // 跨层可复用ID。
                .map(|id| placement(id, 2.0, 2.0)) // 全部位于同一附近桶。
                .collect(), // 收集完整清单。
            9.0,                  // 九米进入圈。
            10.0,                 // 十米退出圈和单元边长。
        ), // 完成第二层。
    ]; // 完成双重层配置。
    let layer_ids = [layers[0].id, layers[1].id]; // 两层均需要活动密度。
    let layout = full_layout(&layer_ids); // 构造双层全一布局。
    let observers = [Vec2::new(2.0, 2.0)]; // 唯一观察者与候选重合。
    let active = BTreeMap::new(); // 所有候选按进入圈判断。
    let mut indexes = ExplicitPlacementSpatialIndexes::default(); // 从空索引开始。
    indexes.rebuild_if_needed(1, &layers); // 为两个重层建独立网格。
    let work = build_layer_work(&layers, &indexes, &observers); // 真实估算每层2048检查。
    assert_eq!(
        execution_mode(&work, 4),
        NearFieldExecutionMode::Parallel { workers: 2 }
    ); // 四线程启用两worker。
    let serial = collect_with_task_pool(&work, &observers, &layout, &active, None); // 无池串行基准。
    let task_pool = TaskPoolBuilder::new().num_threads(4).build(); // 创建隔离四线程池。
    let parallel = collect_with_task_pool(
        // 使用同一输入执行并行。
        &work,            // 同一稳定工作列表。
        &observers,       // 同一观察者快照。
        &layout,          // 同一布局快照。
        &active,          // 同一活动集合。
        Some(&task_pool), // 注入本地任务池。
    ); // 完成真实并行收集。
    assert_eq!(parallel, serial); // 键、原型、Transform和顺序完全相同。
    assert_eq!(parallel.len(), 4_096); // 两层候选均完整且不跨层误去重。
} // 完成串并行确定性回归。

fn work(
    // 构造只用于纯策略的固定估算工作项。
    layer: &crate::VegetationLayerConfig, // 借用合法测试层。
    estimated_checks: usize,              // 注入候选检查上界。
) -> NearFieldLayerWork<'_> {
    // 返回无需真实索引的策略输入。
    NearFieldLayerWork {
        // 策略只读取估算和层数量。
        layer,                // 保存当前层借用。
        explicit_index: None, // 不运行候选收集。
        estimated_checks,     // 保存固定工作量。
    } // 返回策略工作项。
} // 完成固定工作项构造。
