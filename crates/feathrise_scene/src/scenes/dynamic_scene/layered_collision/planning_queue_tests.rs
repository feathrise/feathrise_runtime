//! 单次规划去重扫描的旧算法对照，涵盖径向临界点、尾格、同倍率带及确定性随机输入。
use super::*; // 使用父测试模块的规格、配置构造器与正式planner入口。

fn compare(spec: &SparseHeightSpec, lods: &[SceneCollisionLod], center: Vec2) {
    // 不以摘要替代实际单元审计。
    let actual = plan(spec, lods, center); // 正式队列规划。
    let expected = plan_reference(spec, lods, center); // 冻结的上一提交完整扫描规划。
    assert_eq!(
        actual, expected,
        "配置{lods:?}、中心{center:?}的结果或错误不一致"
    ); // 包括错误文本、块键、签名和真实覆盖。
    if let (Ok(actual), Ok(expected)) = (actual, expected) {
        // 成功时深比较全部单元，而非仅自定义PartialEq。
        for (key, chunk) in &actual.chunks {
            // 稳定顺序逐块核对。
            assert_eq!(
                chunk.cells, expected.chunks[key].cells,
                "块{key:?}单元或接缝改变"
            ); // 相同坐标和finer_edges意味着使用同一几何模板。
        } // 不能通过删减覆盖换速度。
    } // 预算拒绝必须与原合同一致。
} // 全部计算只在无窗口测试运行。

#[test] // 精确临界点与两侧相位保护浮点比较和管理带归属。
fn queue_matches_full_scan_at_radial_thresholds_and_scene_tails() {
    // 尾格不是整格时也必须同版。
    let spec =
        SparseHeightSpec::new([-50.0, -40.0], [78.125, 60.375], 0.125, -80.0, 160.0).unwrap(); // 两轴不同且尾格不整齐。
    let configurations = [
        // 包括跨度大、同倍率和很薄的管理带。
        vec![lod(1.0, 1), lod(4.0, 8)],  // 原用户小近层案例。
        vec![lod(1.0, 1), lod(2.0, 64)], // 邻接需要跨多次二倍过渡。
        vec![lod(1.0, 4), lod(1.001, 4), lod(8.0, 32)], // 相同倍率不能继承错误的父管理band。
        vec![lod(4.0, 2)],               // 单层无LOD交界。
        (0..7).map(|i| lod(1.0 + i as f32, 1 << i)).collect(), // 多于五层。
    ]; // 不改变任何预算。
    for lods in configurations {
        // 每组配置对照相同输入。
        for center in [
            Vec2::ZERO,
            Vec2::new(0.9999, -1.0001),
            Vec2::new(1.0, -1.0),
            Vec2::new(1.0001, -0.9999),
            Vec2::new(-50.0, -40.0),
            Vec2::new(78.125, 60.375),
            Vec2::new(78.125, -40.0),
            Vec2::new(-50.0, 60.375),
        ] {
            // 临界点及四角。
            compare(&spec, &lods, center); // 逐块逐格一致。
        } // 队列顺序不影响最终覆盖。
    } // 真实几何的接缝回归仍使用现有测试。
} // 不需要游戏或外部资产。

#[test] // 确定性随机组合覆盖平衡传播，不依赖随机库或系统时间。
fn queue_matches_full_scan_for_varied_centers_and_layer_gaps() {
    // 便于失败后精确复现。
    let spec = SparseHeightSpec::new([-64.0; 2], [64.0; 2], 0.125, -80.0, 160.0).unwrap(); // 小场景也覆盖图边裁剪。
    let mut seed = 0x748f_9201_u64; // 固定种子不消耗用户随机状态。
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        seed >> 32
    }; // 仅测试生成器。
    for _ in 0..40 {
        // 有界测试工作量。
        let center = Vec2::new(
            next() as u32 as f32 / u32::MAX as f32 * 140.0 - 70.0,
            next() as u32 as f32 / u32::MAX as f32 * 140.0 - 70.0,
        ); // 含图外少量位置。
        let count = (next() % 6 + 1) as usize; // 一至六层。
        let mut distance = 0.25_f32; // 累计距离严格递增。
        let mut exponent = (next() % 3) as u32; // 最近层不强制一倍。
        let mut lods = Vec::new(); // 仅本次配置列表。
        for _ in 0..count {
            // 随机同级或跨级。
            distance += (next() % 13 + 1) as f32 * 0.25; // 有界正增量。
            exponent = (exponent + (next() % 3) as u32).min(7); // 包含同倍率和四倍跳跃。
            lods.push(lod(distance, 1 << exponent)); // 合法二次幂倍率。
        } // 全窗口仍受同一预算约束。
        compare(&spec, &lods, center); // 成功和失败都核对。
    } // 覆盖传播次序和场景尾边。
} // 不作墙钟时间断言。

#[test] // 不放宽或收紧非法输入与资源拒绝合同。
fn queue_preserves_invalid_input_and_budget_errors() {
    // 大图仅创建规格，不分配高度。
    let spec = SparseHeightSpec::new([-512.0; 2], [512.0; 2], 0.125, -80.0, 160.0).unwrap(); // 当前用户尺度。
    for lods in [
        vec![],
        vec![lod(0.0, 1)],
        vec![lod(f32::INFINITY, 1)],
        vec![lod(1.0, 3)],
        vec![lod(2.0, 4), lod(1.0, 4)],
        vec![lod(1.0, 8), lod(2.0, 2)],
        vec![lod(1000.0, 1)],
        vec![lod(64.0, 1)],
    ] {
        // 配置校验、枚举预算和细化/块预算。
        compare(&spec, &lods, Vec2::ZERO); // 原算法错误应一致。
    } // 不静默提高倍率。
    compare(&spec, &[lod(1.0, 1)], Vec2::splat(f32::NAN)); // 非有限中心。
    compare(&spec, &[lod(1.0, 1)], Vec2::splat(10000.0)); // 图外空窗口。
} // 没有新的配置字段。

#[test] // 显式手动对照，不把机器相关耗时当硬性正确性条件。
#[ignore = "手动队列规划CPU基准：旧完整扫描与新队列同输入；非实机帧时"]
fn queue_planning_microbenchmark() {
    // 使用用户既有大范围配置。
    let spec = SparseHeightSpec::new([-512.0; 2], [512.0; 2], 0.125, -80.0, 160.0).unwrap(); // 不改变作者格距。
    let lods = [lod(10.0, 1), lod(100.0, 4), lod(800.0, 32)]; // 相同三层配置。
    compare(&spec, &lods, Vec2::ZERO); // 预热且先确认输出一致，不计时。
    let mut total = [0.0_f64; 2]; // 0旧完整扫描，1新队列。
    for (sample, center) in [Vec2::ZERO, Vec2::new(0.25, 0.13), Vec2::new(1.0, 0.0)]
        .into_iter()
        .enumerate()
    {
        // 覆盖连续移动不同相位。
        let mut results = [None, None]; // 比较保留完整结果，析构不计入构建时间。
        for mode in if sample % 2 == 0 { [0, 1] } else { [1, 0] } {
            // 交替先后顺序。
            take_work_counts(); // 清除预热或上一组的测试线程计数。
            let started = std::time::Instant::now(); // 仅纯规划墙钟。
            let result = if mode == 0 {
                plan_reference(&spec, &lods, center)
            } else {
                plan(&spec, &lods, center)
            }
            .unwrap(); // 相同数据和预算。
            let ms = started.elapsed().as_secs_f64() * 1000.0; // 不包括下方深比较。
            let work = take_work_counts(); // 邻接查询、平衡全扫描、距离计算次数。
            total[mode] += ms; // 三个位置总量。
            eprintln!(
                "queue_plan sample={sample} mode={} ms={ms:.3} chunks={} work={work:?}",
                if mode == 0 { "full_scan" } else { "queue" },
                result.chunks.len()
            ); // 输出原始测量。
            results[mode] = Some(result); // 保持对照数据。
        } // 两个路径都完成。
        let reference = results[0].as_ref().unwrap(); // 原版本。
        let actual = results[1].as_ref().unwrap(); // 新版本。
        assert_eq!(actual, reference); // 身份和覆盖相同。
        for (key, chunk) in &actual.chunks {
            assert_eq!(chunk.cells, reference.chunks[key].cells);
        } // 深比接缝单元。
    } // 确保没有用少算格子换速度。
    eprintln!(
        "queue_plan total full_scan_ms={:.3} queue_ms={:.3}; CPU_only",
        total[0], total[1]
    ); // 不承诺目标硬件或游戏跟随效果。
} // 不修改用户场景、设置或物理世界。
