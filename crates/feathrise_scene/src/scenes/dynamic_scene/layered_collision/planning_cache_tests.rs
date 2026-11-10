//! 跨次规划缓存的连续移动、掉头、边界及配置换代对照；性能只测后台CPU。
use super::*; // 复用配置构造器及独立冷规划入口。
use std::sync::Arc; // 仅保留上一个已确认有效的候选快照。

fn assert_same(actual: &Plan, expected: &Plan) {
    // 不让ChunkPlan摘要相等掩盖真实cells差异。
    assert_eq!(actual, expected); // 审计块键、摘要和实际覆盖行。
    for (key, chunk) in &actual.chunks {
        // 所有单元和接缝必须精确一致。
        assert_eq!(
            chunk.cells, expected.chunks[key].cells,
            "块{key:?}存在过期选级或接缝"
        ); // 同版真实几何输入。
    } // 不比较耗时与缓存所有权。
} // 不读取或改写用户资产。

#[test] // 连续复用不能把临界点误差或过期精细叶逐次积累。
fn cached_planning_matches_cold_during_turns_edges_and_teleports() {
    // 覆盖正反移动与局部精度撤销。
    let spec = SparseHeightSpec::new(
        [-1024.0, -1040.0],
        [1040.125, 1040.375],
        0.125,
        -80.0,
        160.0,
    )
    .unwrap(); // 两轴不同且存在尾格，并实际支持64倍及七层配置。
    for lods in [
        vec![lod(1.0, 1), lod(4.0, 8)],
        vec![lod(1.0, 1), lod(2.0, 64)],
        vec![lod(1.0, 4), lod(1.001, 4), lod(8.0, 32)],
        vec![lod(4.0, 2)],
        (0..7).map(|i| lod(i as f32 + 1.0, 1 << i)).collect(),
    ] {
        // 同倍率薄层和大倍率跳级也保留管理带。
        let mut previous: Option<Arc<PlanCache>> = None; // 新配置从空缓存起步。
        let mut successful = 0; // 防止因场景不支持倍率而只测试到错误分支。
        let centers = (0..24)
            .chain((0..24).rev())
            .map(|i| Vec2::new(i as f32 * 0.125, (i % 5) as f32 * 0.03125))
            .chain([
                Vec2::from_array(spec.world_min),
                Vec2::from_array(spec.world_max),
                Vec2::new(spec.world_max[0], spec.world_min[1]),
                Vec2::new(spec.world_min[0], spec.world_max[1]),
                Vec2::splat(10000.0),
                Vec2::ZERO,
            ]); // 小步、掉头、图角、图外及传送返回。
        for center in centers {
            // 每次使用刚完成的快照，检查真实跨代链。
            let actual = plan_cached(&spec, &lods, center, previous.as_deref()); // 缓存优化路径。
            let expected = plan(&spec, &lods, center); // 第一阶段冷算法权威结果。
            match (actual, expected) {
                // 合法预算错误也要求一致。
                (Ok((actual, cache)), Ok(expected)) => {
                    assert_same(&actual, &expected);
                    previous = Some(cache);
                    successful += 1;
                } // 只有成功结果可成为下一轮基线。
                (Err(actual), Err(expected)) => assert_eq!(actual, expected), // 坏配置不污染旧缓存。
                (actual, expected) => panic!(
                    "冷/热结果不一致，位置{center:?}: hot_ok={}, cold={expected:?}",
                    actual.is_ok()
                ), // 明确失败场景。
            } // 不扩大覆盖或偷偷降精度。
        } // 离开精细区后必须能够回收旧细格。
        assert!(successful >= 48, "配置{lods:?}未真正覆盖连续成功规划"); // 连续移动/掉头必须实际成功，不能把拒绝当等价。
    } // 每种配置独立生命周期。
} // 不依赖线程池或物理世界。

#[test] // 配置和规格改变不能使用同坐标的旧管理带。
fn cached_planning_invalidates_identity_and_preserves_errors() {
    // 传入旧cache也必须由规划器验证。
    let spec = SparseHeightSpec::new([-512.0; 2], [512.0; 2], 0.125, -80.0, 160.0).unwrap(); // 实际支持8倍的合法基线，不把配置拒绝误当缓存回归。
    let lods = [lod(2.0, 1), lod(8.0, 8)]; // 默认细近粗远。
    let (_, old) = plan_cached(&spec, &lods, Vec2::ZERO, None).unwrap(); // 完整成功快照。
    for changed in [
        vec![lod(3.0, 1), lod(9.0, 8)],
        vec![lod(2.0, 2), lod(8.0, 8)],
        vec![lod(2.0, 1), lod(4.0, 4), lod(8.0, 8)],
        vec![],
        vec![lod(f32::NAN, 1)],
        vec![lod(8.0, 1), lod(2.0, 4)],
        vec![lod(1000.0, 1)],
    ] {
        // 增层、改距离倍率及错误预算。
        let actual = plan_cached(&spec, &changed, Vec2::ZERO, Some(&old)).map(|(plan, _)| plan); // 不替换有效基线。
        let expected = plan(&spec, &changed, Vec2::ZERO); // 冷配置验证。
        assert_eq!(actual, expected); // 包括错误文本。
        if let (Ok(actual), Ok(expected)) = (actual, expected) {
            assert_same(&actual, &expected);
        } // 深比较。
    } // 失败后原缓存仍应可复用。
    for changed in [
        SparseHeightSpec::new([-512.0; 2], [512.0; 2], 0.25, -80.0, 160.0).unwrap(),
        SparseHeightSpec::new([-488.0; 2], [536.0; 2], 0.125, -80.0, 160.0).unwrap(),
    ] {
        // 格距或世界原点换代，仍支持所有测试倍率。
        let (actual, _) = plan_cached(&changed, &lods, Vec2::ZERO, Some(&old)).unwrap(); // 旧快照不适配时冷重算。
        assert_same(&actual, &plan(&changed, &lods, Vec2::ZERO).unwrap()); // 保证所有固定格坐标更新。
    } // 不跨规格缓存。
    let (actual, _) = plan_cached(&spec, &lods, Vec2::ZERO, Some(&old)).unwrap(); // 错误候选没有修改旧快照。
    assert_same(&actual, &plan(&spec, &lods, Vec2::ZERO).unwrap()); // 完整回到基线。
    assert_eq!(
        plan_cached(&spec, &lods, Vec2::splat(f32::NAN), Some(&old)).err(),
        plan(&spec, &lods, Vec2::splat(f32::NAN)).err()
    ); // 非有限中心在命中缓存前拒绝。
} // 运行时场景/根身份另在布局入口测试。

#[test] // 以计数和Arc证明实际复用，不用墙钟断言代替缓存语义。
fn cached_planning_reuses_groups_and_same_position_plan() {
    // 同位置高度修订应省掉整份规划。
    let spec = SparseHeightSpec::new([-512.0; 2], [512.0; 2], 0.125, -80.0, 160.0).unwrap(); // 足够大规格支持全部倍率。
    let lods = [lod(4.0, 1), lod(32.0, 4)]; // 稳定内外区域和真实过渡。
    let (initial, cache) = plan_cached(&spec, &lods, Vec2::ZERO, None).unwrap(); // 完整采用基线。
    take_cache_counts(); // 计数不包含首载。
    let (same, _) = plan_cached(&spec, &lods, Vec2::ZERO, Some(&cache)).unwrap(); // 高度修订不改变这些纯规划输入。
    let counts = take_cache_counts(); // 重建、复用、整计划次数。
    assert_eq!(counts[0], 0); // 不重建任何径向组。
    assert!(counts[1] > 0 && counts[2] == 1); // 全Plan真实短路，不只是恰好相等。
    for (key, chunk) in &same.chunks {
        assert!(Arc::ptr_eq(
            chunk.cells.as_ref().unwrap(),
            initial.chunks[key].cells.as_ref().unwrap()
        ));
    } // 单元数组没有深复制。
    let center = Vec2::new(0.25, 0.125); // 改变径向边界但仍有稳定区域。
    let (moved, _) = plan_cached(&spec, &lods, center, Some(&cache)).unwrap(); // 移动时只重建失效组。
    let counts = take_cache_counts(); // 本次移动的实际工作量。
    assert!(counts[0] > 0 && counts[1] > 0 && counts[2] == 0); // 没有错误冻结整份旧计划。
    assert_same(&moved, &plan(&spec, &lods, center).unwrap()); // 接缝仍完整核验。
} // 无机器相关时间阈值。

#[test] // 开发机CPU基准，显式运行且不使用硬件敏感时间断言。
#[ignore = "手动跨次规划CPU对照：连续移动与旧冷队列相同输入，非游戏帧时"]
fn cached_planning_movement_microbenchmark() {
    // 每个位置都实际推进缓存，包含反向移动。
    let spec = SparseHeightSpec::new([-512.0; 2], [512.0; 2], 0.125, -80.0, 160.0).unwrap(); // 与此前基准相同规格。
    let lods = [lod(10.0, 1), lod(100.0, 4), lod(800.0, 32)]; // 不减范围或提高倍率换速度。
    let started = std::time::Instant::now(); // 首载缓存成本也记录。
    let (first, mut previous) = plan_cached(&spec, &lods, Vec2::ZERO, None).unwrap(); // 建立初始快照。
    eprintln!(
        "cache_plan cold_ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    ); // 不把首次成本藏到预热。
    assert_same(&first, &plan(&spec, &lods, Vec2::ZERO).unwrap()); // 先确认初始等价。
    let mut totals = [0.0_f64; 2]; // 0冷队列，1跨次复用。
    for (sample, center) in [
        Vec2::new(0.25, 0.13),
        Vec2::new(1.0, 0.0),
        Vec2::new(2.0, 0.0),
        Vec2::new(3.0, 1.0),
        Vec2::new(2.0, 0.0),
        Vec2::ZERO,
    ]
    .into_iter()
    .enumerate()
    {
        // 小步、大步、掉头和返回。
        let mut plans = [None, None]; // 不在测量区间深比较或销毁。
        for mode in if sample % 2 == 0 { [0, 1] } else { [1, 0] } {
            // 交替冷热执行顺序。
            take_cache_counts(); // 清除上一组计数，不混入预热。
            let started = std::time::Instant::now(); // 只计完整规划调用。
            let result = if mode == 0 {
                plan(&spec, &lods, center).unwrap()
            } else {
                let (plan, cache) = plan_cached(&spec, &lods, center, Some(&previous)).unwrap();
                previous = cache;
                plan
            }; // 热路径真实释放上一代快照。
            let ms = started.elapsed().as_secs_f64() * 1000.0; // 不包括输出与深比较。
            let counts = take_cache_counts(); // 重建组、复用组、完整Plan命中次数。
            totals[mode] += ms; // 全部移动样本合计。
            eprintln!(
                "cache_plan sample={sample} mode={} ms={ms:.3} chunks={} groups={counts:?}",
                if mode == 0 {
                    "cold_queue"
                } else {
                    "cross_refresh"
                },
                result.chunks.len()
            ); // 如实输出，允许优化未获益。
            plans[mode] = Some(result); // 两条路径保持完整结果。
        } // 本位置两条路径已就绪。
        assert_same(plans[0].as_ref().unwrap(), plans[1].as_ref().unwrap()); // 不能靠漏格/旧格省时。
    } // 只保留一个上一版本快照，不累积历史链。
    eprintln!(
        "cache_plan totals cold_queue_ms={:.3} cross_refresh_ms={:.3}; CPU_only",
        totals[0], totals[1]
    ); // 不代表端到端跟随或主线程性能。
} // 实机验收仍需运行游戏。
