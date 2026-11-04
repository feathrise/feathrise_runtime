//! 显式空间索引的负坐标、边界、多观察者去重和全量扫描等价性回归。

use std::collections::BTreeMap; // 参考全量扫描使用稳定上一活动集合。

use bevy::prelude::{IVec2, Transform, Vec2, Vec3Swizzles}; // 构造世界位置并执行原距离判断。

use crate::{
    // 参考实现沿用公共候选与层配置语义。
    ActiveVegetationLayout,
    VegetationCandidateSource,
    VegetationLayerConfig,
    VegetationLayerId,
    VegetationRealization,
    deterministic_candidate,
}; // 完成空间回归依赖导入。

use super::super::ActiveNearFieldProxy; // 构造上一活动集合和参考结果。
use super::super::collection::{NearFieldEntry, build_layer_work, collect_with_task_pool}; // 运行新索引候选路径。
use super::super::spatial_index::ExplicitPlacementSpatialIndexes; // 直接查询均匀网格桶。
use super::fixture::{TEST_LAYER, explicit_layer, full_layout, placement}; // 复用合法显式层fixture。

#[test] // 粗筛必须正确覆盖负坐标、单元边界和重叠观察者。
fn explicit_spatial_index_handles_negative_boundaries_and_deduplicates_observers() {
    // floor错误会直接漏下标。
    let layer = explicit_layer(
        // 构造跨越世界原点的七个落点。
        TEST_LAYER, // 使用稳定测试层。
        vec![
            // 按X升序便于核对原清单下标。
            placement(0, -20.0, 0.0), // 远离全部窗口。
            placement(1, -10.0, 0.0), // 位于负窗口精确边界。
            placement(2, -5.0, 0.0),  // 与负观察者重合。
            placement(3, 0.0, 0.0),   // 被三个窗口重复覆盖。
            placement(4, 5.0, 0.0),   // 与正观察者重合。
            placement(5, 10.0, 0.0),  // 位于正窗口精确边界。
            placement(6, 20.0, 0.0),  // 远离全部窗口。
        ], // 完成跨原点清单。
        4.0,        // 进入半径不参与纯粗筛。
        5.0,        // 退出半径同时作为单元边长。
    ); // 完成测试层。
    let mut indexes = ExplicitPlacementSpatialIndexes::default(); // 从空索引开始。
    assert!(indexes.rebuild_if_needed(1, std::slice::from_ref(&layer))); // 第一代必须真实构建。
    let index = indexes.get(TEST_LAYER).unwrap(); // 显式近场层必须有索引。
    let observers = [
        // 三个窗口有意重叠。
        Vec2::new(-5.0, 0.0), // 覆盖-10到0。
        Vec2::ZERO,           // 重复覆盖-5到5。
        Vec2::new(5.0, 0.0),  // 覆盖0到10。
    ]; // 完成观察者集合。
    let nearby = index.query_indices(&observers, 5.0); // 查询退出半径桶并集。
    assert_eq!(nearby, vec![1, 2, 3, 4, 5]); // 边界完整且重复下标已去重。
    assert_eq!(index.estimated_candidate_checks(&observers, 5.0), 9); // 三个窗口各命中三项，重叠桶按调度上界重复计数。
} // 完成负坐标与边界回归。

#[test] // 新空间索引结果必须与改造前显式全量扫描完全相同。
fn indexed_collection_matches_exhaustive_reference() {
    // 同时覆盖多观察者与退出半径。
    let layer = explicit_layer(
        // 在单块正坐标内布置边界候选。
        TEST_LAYER, // 使用稳定测试层。
        vec![
            // 横跨两个重叠观察者窗口。
            placement(0, 0.0, 0.0),  // 距首观察者五米，仅活动状态可保留。
            placement(1, 4.0, 0.0),  // 距首观察者一米。
            placement(2, 6.0, 0.0),  // 被两个观察者重复覆盖。
            placement(3, 10.0, 0.0), // 距第二观察者一米。
            placement(4, 14.0, 0.0), // 距第二观察者三米。
            placement(5, 16.0, 0.0), // 五米且未活动，应被内圈剔除。
            placement(6, 30.0, 0.0), // 远离两个窗口。
        ], // 完成显式清单。
        4.0,        // 新候选四米内进入。
        6.0,        // 活动候选六米内保留。
    ); // 完成等价性层。
    let layout = full_layout(&[TEST_LAYER]); // 全一布局让距离成为唯一变量。
    let observers = [Vec2::new(5.0, 0.0), Vec2::new(11.0, 0.0)]; // 两窗口重叠。
    let mut active = BTreeMap::new(); // 构造上一完整活动集合。
    active.insert(
        // 零号候选预先活动。
        (TEST_LAYER, 0), // 保存稳定联合主键。
        ActiveNearFieldProxy {
            // 描述与零号候选一致。
            prototype: layer.prototype, // 使用当前层原型。
            transform: Transform::from_xyz(0.0, 0.0, 0.0), // 使用零号变换。
        }, // 完成代理描述。
    ); // 完成活动集合。
    let mut indexes = ExplicitPlacementSpatialIndexes::default(); // 从空索引开始。
    indexes.rebuild_if_needed(1, std::slice::from_ref(&layer)); // 构建单层索引。
    let work = build_layer_work(std::slice::from_ref(&layer), &indexes, &observers); // 组装真实工作项。
    let indexed = collect_with_task_pool(&work, &observers, &layout, &active, None); // 强制串行隔离索引效果。
    let exhaustive = exhaustive_reference(&layer, &observers, &layout, &active); // 运行旧全量扫描参考。
    assert_eq!(indexed, exhaustive); // 键、原型、Transform和顺序必须完全一致。
} // 完成索引等价性回归。

fn exhaustive_reference(
    // 复刻改造前显式全量扫描。
    layer: &VegetationLayerConfig,   // 当前显式近场层。
    observers: &[Vec2],              // 当前观察者位置。
    layout: &ActiveVegetationLayout, // 当前活动布局。
    active: &BTreeMap<(VegetationLayerId, u64), ActiveNearFieldProxy>, // 上一活动集合。
) -> Vec<NearFieldEntry> {
    // 返回与新收集器相同类型。
    let VegetationCandidateSource::ExplicitPlacements(placements) = &layer.source else {
        // 仅处理显式源。
        return Vec::new(); // 非显式源不属于本参考。
    }; // 完成显式源提取。
    let VegetationRealization::HybridNearField {
        // 提取滞回半径。
        enter_radius, // 新候选内圈。
        exit_radius,  // 活动候选外圈。
    } = layer.realization
    else {
        // 仅处理混合近场层。
        return Vec::new(); // 非近场层不产生代理。
    }; // 完成半径提取。
    let mut result = Vec::new(); // 按原清单暂存结果。
    for placement_index in 0..placements.len() {
        // 旧路径每次扫描全部显式落点。
        let candidate = deterministic_candidate(
            // 恢复公共稳定候选。
            &layer.source,                         // 当前显式源。
            IVec2::new(placement_index as i32, 0), // 原清单下标映射。
        ) // 完成候选生成。
        .unwrap(); // 有效下标必定成功。
        let key = (layer.id, candidate.instance_id); // 构造联合主键。
        let radius = if active.contains_key(&key) {
            // 上一状态选择滞回半径。
            exit_radius // 活动候选使用外圈。
        } else {
            // 新候选使用内圈。
            enter_radius // 必须真正进入。
        }; // 完成半径选择。
        let candidate_xz = candidate.transform.translation.xz(); // 提取世界XZ。
        if !observers // 任一观察者命中即可保留。
            .iter() // 遍历全部观察者。
            .any(|observer| observer.distance_squared(candidate_xz) <= radius * radius)
        // 保持原闭区间圆形判断。
        {
            // 不在任何实际滞回圆内。
            continue; // 跳过当前候选。
        } // 完成距离门。
        if !crate::resolved_candidate_enabled(layout, layer.id, &candidate) {
            // 保持原布局选择。
            continue; // 未启用候选不产生代理。
        } // 完成布局门。
        result.push((
            // 保存完整代理结果。
            key, // 保存联合主键。
            ActiveNearFieldProxy {
                // 构造代理描述。
                prototype: layer.prototype,     // 保存层原型。
                transform: candidate.transform, // 保存候选变换。
            }, // 完成代理描述。
        )); // 完成参考结果登记。
    } // 完成全量扫描。
    result.sort_by_key(|entry| entry.0); // 与新实现相同稳定排序。
    result // 公共验证已拒绝重复层内ID。
} // 完成旧全量扫描参考。
