//! 邻接索引优化与原树查询逐项对照，保护格子、接缝、覆盖和预算语义。
#[path = "planning_cache_tests.rs"] // 跨次快照必须在真实连续移动中与冷规划一致。
mod cache_tests;
#[path = "planning_queue_tests.rs"] // 单次规划队列优化使用冻结旧算法单独对照。
mod queue_tests; // 不修改原有等价测试的覆盖范围。 // 独立测试不改变生产缓存策略。
use super::*; // 私有参考入口只在测试构建可用。

fn lod(distance: f32, multiplier: u32) -> MapCollisionLod {
    // 只构造测试配置，不改用户地图。
    MapCollisionLod {
        // 原始半径/倍率语义。
        max_distance_meters: distance,  // 累计半径。
        spacing_multiplier: multiplier, // 相对作者格的倍率。
    } // 保持正式校验。
} // 无测试旁路。

fn assert_reference(spec: &SparseHeightSpec, lods: &[MapCollisionLod], center: Vec2) {
    // 每个真实输入分别跑两种查找。
    let indexed = plan_indexed(spec, lods, center, true); // 正式索引路径。
    let reference = plan_indexed(spec, lods, center, false); // 原BTreeSet邻接查询。
    assert_eq!(
        indexed, reference,
        "中心 {center:?} 的块身份、签名及实际覆盖不同"
    ); // 包括错误与预算结果。
    if let (Ok(indexed), Ok(reference)) = (indexed, reference) {
        // 不能仅以摘要相同代替数据审计。
        for (key, actual) in &indexed.chunks {
            // 每个确定顺序的块。
            let expected = &reference.chunks[key]; // 已核对键集合。
            assert_eq!(actual.cells, expected.cells, "块 {key:?} 的单元/接缝不同"); // 深比较完整格及finer_edges。
        } // 不改变几何模板。
    } // 超预算仍同样拒绝。
} // 不测游戏帧时。

#[test] // 多相位连续移动、掉头和倍率跳级。
fn indexed_planning_matches_tree_lookup_during_movement() {
    // 所有量均限制在小型回归窗口。
    let spec = SparseHeightSpec::new([-128.0; 2], [128.0; 2], 0.125, -128.0, 256.0).unwrap(); // 固定作者坐标。
    let configurations = [
        // 含同倍率薄带和六层。
        vec![lod(2.0, 1)],                                     // 单层无过渡。
        vec![lod(1.0, 1), lod(4.0, 2)],                        // 小近层。
        vec![lod(1.0, 1), lod(8.0, 32)],                       // 必须内部二倍过渡。
        vec![lod(1.01, 4), lod(1.02, 4), lod(8.0, 16)],        // 相同倍率管理带。
        (0..6).map(|i| lod(1.0 + i as f32, 1 << i)).collect(), // 五层不是上限。
    ]; // 不增加新的功能配置。
    for lods in configurations {
        // 不同精度与管理块粒度。
        for center in [
            Vec2::ZERO,
            Vec2::new(0.13, -0.27),
            Vec2::new(0.49, 0.51),
            Vec2::new(2.03, -1.07),
            Vec2::ZERO,
        ] {
            // 前进、跨格、返回。
            assert_reference(&spec, &lods, center); // 完整计划逐项一致。
        } // 每次请求独立，不保留旧细化叶。
    } // 包括接缝的确定顺序。
} // 正确性测试不是性能承诺。

#[test] // 图边尾格、图外与非法输入都使用同一合同。
fn indexed_planning_matches_clipped_edges_and_budget_failures() {
    // 窄地图不能因索引生成图外地面。
    let spec = SparseHeightSpec::new([-13.0, -7.0], [58.3, 69.7], 0.125, -128.0, 256.0).unwrap(); // 合法尺寸至少64米，仍保留非整倍数尾边。
    for center in [
        Vec2::new(-13.0, -7.0),
        Vec2::new(58.3, 69.7),
        Vec2::new(58.29, -6.99),
        Vec2::new(-12.99, 69.69),
        Vec2::splat(9000.0),
    ] {
        // 四角及无覆盖。
        assert_reference(&spec, &[lod(1.0, 1), lod(9.0, 16)], center); // 按真实尾边裁切。
    } // 索引不扩大边界。
    assert_reference(&spec, &[], Vec2::ZERO); // 无层失败一致。
    assert_reference(&spec, &[lod(1.0, 3)], Vec2::ZERO); // 非二次幂失败一致。
    let large = SparseHeightSpec::new([-4096.0; 2], [4096.0; 2], 0.125, -128.0, 256.0).unwrap(); // 只分配规格。
    assert_reference(&large, &[lod(1000.0, 1)], Vec2::ZERO); // 枚举前硬预算一致。
} // 不削减预算规避错误。

#[test] // 按精度range必须与原全树filter同一序列。
fn level_range_preserves_original_selection_order() {
    // 独立验证省掉全叶扫描不会遗漏。
    let leaves: BTreeSet<CellKey> = [(0, 0, 0), (1, 0, 2), (1, 2, 0), (2, 0, 4), (5, 32, 64)]
        .into_iter()
        .collect(); // 多层稀疏叶。
    for level in 0..=7 {
        // 含空层。
        let original: Vec<_> = leaves
            .iter()
            .copied()
            .filter(|key| key.0 == level)
            .collect(); // 原过滤语义。
        let ranged: Vec<_> = leaves
            .range((level, 0, 0)..=(level, u64::MAX, u64::MAX))
            .copied()
            .collect(); // 新精确范围。
        assert_eq!(original, ranged); // 顺序与成员均相同。
    } // 不依赖hash迭代顺序。
} // 有序签名稳定。

#[test] // 只显式运行，不增加普通回归的耗时。
#[ignore = "手动规划微基准；--ignored --nocapture，不代表游戏实机帧时"] // 不污染地图或设置。
fn planning_index_microbenchmark() {
    // 同进程比较完整规划的查找路径。
    let spec = SparseHeightSpec::new([-512.0; 2], [512.0; 2], 0.125, -128.0, 256.0).unwrap(); // 用户实际地图尺度与作者格距。
    let mut layers = vec![lod(10.0, 1), lod(100.0, 4), lod(800.0, 32)]; // 用户实际三层配置。
    let centers = [Vec2::ZERO, Vec2::new(0.25, 0.13), Vec2::new(0.5, 0.26)]; // 三个连续移动中心。
    if let Err(error) = plan_indexed(&spec, &layers, centers[0], true) {
        // 预热兼预算检查，不纳入计时。
        assert!(error.contains("预算"), "非预算错误不能换配置绕过：{error}"); // 只有预算拒绝允许备用案例。
        eprintln!("三层案例被正式预算拒绝：{error}；改用10米×1、100米×8两层代表配置"); // 明确披露不是原配置通过。
        layers = vec![lod(10.0, 1), lod(100.0, 8)]; // 固定公开备用，不修改生产预算。
        plan_indexed(&spec, &layers, centers[0], true).expect("备用两层配置必须合法"); // 不吞掉第二次失败。
    } // 正式地图文件完全不读写。
    let mut totals = [std::time::Duration::ZERO; 2]; // 0为哈希索引，1为树查询。
    for (sample, center) in centers.into_iter().enumerate() {
        // 交替顺序减轻缓存先后偏差。
        let mut results = [None, None]; // 保留完整结果供等价审计，析构不计入规划耗时。
        let order = if sample % 2 == 0 { [0, 1] } else { [1, 0] }; // 不总让同一路径先运行。
        for mode in order {
            // 同一输入的两个算法。
            let started = std::time::Instant::now(); // 仅包围纯规划。
            let result =
                plan_indexed(&spec, &layers, center, mode == 0).expect("微基准规划必须成功"); // 不隐藏移动后的预算错误。
            let elapsed = started.elapsed(); // 不包含后面的深比较。
            totals[mode] += elapsed; // 三次总量。
            eprintln!(
                "planning sample={sample} center={center:?} mode={} elapsed_ms={:.3} chunks={}",
                if mode == 0 { "indexed" } else { "tree" },
                elapsed.as_secs_f64() * 1000.0,
                result.chunks.len()
            ); // 只输出诊断。
            results[mode] = Some(result); // 同版本结果保持到比对结束。
        } // 两种实现均已完成。
        let indexed = results[0].as_ref().unwrap(); // 索引结果。
        let reference = results[1].as_ref().unwrap(); // 树结果。
        assert_eq!(indexed, reference); // 块键、签名、覆盖必须严格一致。
        for (key, chunk) in &indexed.chunks {
            // 不能只以摘要代替单元检查。
            assert_eq!(chunk.cells, reference.chunks[key].cells); // 完整坐标与接缝标记逐项一致。
        } // 只有正确性可作硬断言。
    } // 三次连续移动完成。
    eprintln!(
        "planning total indexed_ms={:.3} tree_ms={:.3}; 仅无窗口CPU微基准，不是实机跟随延迟",
        totals[0].as_secs_f64() * 1000.0,
        totals[1].as_secs_f64() * 1000.0
    ); // 不断言性能以免机器噪声制造假失败。
} // 无资产与配置副作用。
