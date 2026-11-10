//! 有界并行构建与串行权威结果逐项对照；微基准仅测纯CPU，不代表游戏帧时。
use super::super::{
    build,
    resources::{Cooked, Layout},
}; // 两种执行方式共用相同构造规则。
use super::{blank_stamps, request}; // 复用正式规格与源身份夹具。
use bevy::{
    prelude::Vec2,
    tasks::{TaskPoolBuilder, block_on},
}; // 同步等待仅存在于独立测试线程。
use feathrise_scene_content::sparse_height::{SparseHeightTile, SparseHeightTileKey}; // 非平高度快照。
use feathrise_scene_content::{SceneCollisionLod, SparseHeightSpec}; // 与用户配置相同的权威格距合同。
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
}; // 大数据保持不可变共享。

fn assert_layout_equal(actual: &Layout, expected: &Layout) {
    // 对照全部网格而非仅比较块数。
    assert_eq!(actual.chunks.len(), expected.chunks.len()); // 完整目标没有遗漏。
    assert_eq!(actual.tiles, expected.tiles); // 总源页需求完全一致。
    assert_eq!(actual.supported_rows, expected.supported_rows); // 活动门读取同版真实覆盖。
    assert_eq!(actual.supported_polygon, expected.supported_polygon); // 兼容轮廓保持原值。
    for (key, chunk) in &actual.chunks {
        // 每块接缝与精度独立审计。
        let reference = &expected.chunks[key]; // 稳定世界键必须存在。
        assert_eq!(chunk.plan, reference.plan); // 接缝摘要及格距身份一致。
        assert_eq!(chunk.geometry.vertices, reference.geometry.vertices); // 逐顶点比对。
        assert_eq!(chunk.geometry.triangles, reference.geometry.triangles); // 逐三角比对。
        assert_eq!(
            chunk.geometry.base_grid_points,
            reference.geometry.base_grid_points
        ); // 采样格点一致。
        assert_eq!(chunk.tiles, reference.tiles); // 局部源依赖不能少算。
    } // 耗时不参与内容等价。
} // 不依赖游戏窗口或共享线程池。

fn assert_cooked_equal(actual: &Cooked, expected: &Cooked) {
    // 审计最终Avian形状而非仅中间网格。
    assert_eq!(actual.plans, expected.plans); // 保留块也必须完整。
    assert_eq!(actual.bounds, expected.bounds); // 活动门块边界完全一致。
    assert_eq!(actual.supported_rows, expected.supported_rows); // 覆盖同版。
    assert_eq!(actual.supported_polygon, expected.supported_polygon); // 兼容边界同版。
    assert_eq!(actual.shapes.len(), expected.shapes.len()); // 相同真实变脏数量。
    for ((plan, stamp, collider), (old_plan, old_stamp, old_collider)) in
        actual.shapes.iter().zip(&expected.shapes)
    {
        // 输出顺序不能随调度改变。
        assert_eq!(plan, old_plan); // 稳定键及接缝相同。
        assert!(stamp == old_stamp); // 内容身份相同。
        let mesh = collider.shape().as_trimesh().unwrap(); // 正式碰撞必须是三角网格。
        let old_mesh = old_collider.shape().as_trimesh().unwrap(); // 串行权威形状。
        assert_eq!(mesh.vertices(), old_mesh.vertices()); // 高度采样及最终物理顶点逐项一致。
        assert_eq!(mesh.indices(), old_mesh.indices()); // Avian构造后的索引也必须一致。
    } // 不把形状数量相等冒充几何等价。
} // 不包含任何物理世界安装。

#[test] // 三种并发度均覆盖冷构造、移动与高度修订。
fn parallel_layout_matches_serial_and_reuses_real_arcs() {
    // 调度不能改变几何。
    let pool = TaskPoolBuilder::new().num_threads(5).build(); // 给四个worker及协调保留空间。
    let original = request(); // 默认16/64米窗口包含稳定内部块；极小窗口移动时可能所有管理块都触及变化边界。
    let original = Arc::new(original); // 只读冻结需求。
    let previous = Arc::new(build::prepare_layout(&original).unwrap()); // 已采用缓存基线。
    for workers in [1, 2, 4] {
        // 单worker也是正式异步入口。
        for scenario in 0..3 {
            // 冷构造、移动和同位置高度编辑分别审计。
            let mut current = (*original).clone(); // 不深拷贝高度页。
            if scenario == 1 {
                current.anchor.x += 0.5;
            } // 移动产生变化块与保留块。
            if scenario == 2 {
                current.live_revision = Some(7);
            } // 高度变化不应污染拓扑缓存。
            let current = Arc::new(current); // 后台冻结本次需求。
            let cache = (scenario != 0).then(|| (Arc::clone(&original), Arc::clone(&previous))); // 冷构造不提供缓存。
            let expected = build::prepare_layout_cached(
                &current,
                cache
                    .as_ref()
                    .map(|(old, layout)| (old.as_ref(), layout.as_ref())),
            )
            .unwrap(); // 逐块串行基线。
            let actual = block_on(build::prepare_layout_parallel(
                Arc::clone(&current),
                cache,
                &pool,
                workers,
            ))
            .unwrap(); // 仅测试阻塞等待。
            assert_layout_equal(&actual, &expected); // 比较每个顶点、索引与依赖。
            assert_eq!(actual.reused_blocks, expected.reused_blocks); // 诊断真实命中一致。
            assert_eq!(actual.rebuilt_blocks, expected.rebuilt_blocks); // 诊断真实重建一致。
            if scenario != 0 {
                // 相同计划必须真正共享，不准复制后冒充命中。
                assert!(actual.reused_blocks > 0); // 确实覆盖了复用分支。
                for (key, chunk) in &actual.chunks {
                    // 全部保留块逐项核对。
                    if let Some(old) = previous
                        .chunks
                        .get(key)
                        .filter(|old| old.plan == chunk.plan)
                    {
                        // 接缝身份相同才可复用。
                        assert!(Arc::ptr_eq(old, chunk)); // 证明大网格没有复制。
                    } // 变化块由同一规则重新生成。
                } // 已采用缓存始终不可变。
            } // 冷构造不要求共享。
            if scenario == 2 {
                assert_eq!(actual.rebuilt_blocks, 0);
            } // 只改高度不重建零高拓扑。
        } // 三种场景全部完成后再切并发度。
    } // 不测时间阈值，避免硬件敏感测试。
} // 独立池随测试结束回收。

#[test] // 非平高度与零变化、失败分支都不能随并行度改变。
fn parallel_cook_matches_nonflat_shapes_and_keeps_atomic_errors() {
    // 不将部分成功结果暴露给发布系统。
    let pool = TaskPoolBuilder::new().num_threads(5).build(); // 专用池避免修改游戏全局配置。
    let mut input = request(); // 作者0.125米格距。
    input.lods = vec![
        SceneCollisionLod {
            max_distance_meters: 2.0,
            spacing_multiplier: 1,
        },
        SceneCollisionLod {
            max_distance_meters: 8.0,
            spacing_multiplier: 4,
        },
    ]; // 保留真实接缝。
    let input = Arc::new(input); // 全worker共享输入。
    let key = SparseHeightTileKey { x: 4, z: 4 }; // 玩家中心所在作者页。
    let mut tile = SparseHeightTile::blank(&input.spec, key).unwrap(); // 仅分配一页。
    tile.set([0, 0], input.spec.encode_height(3.0).unwrap())
        .unwrap(); // 玩家脚下构造确定非平顶点。
    let source = Arc::new(tile); // 采样期间不可变。
    let sources = BTreeMap::from([(key, Arc::clone(&source))]); // 单页快照不复制内容。
    let baseline_layout = build::prepare_layout(&input).unwrap(); // 串行权威输入。
    let baseline_stamps = blank_stamps(&baseline_layout); // 同一个布局产生源身份，不重复规划。
    let expected = build::cook(
        &input,
        baseline_layout,
        baseline_stamps,
        BTreeSet::new(),
        sources.clone(),
        BTreeMap::new(),
    )
    .unwrap(); // 真实Avian三角形构造。
    assert!(expected.shapes.iter().any(|(_, _, collider)| {
        collider
            .shape()
            .as_trimesh()
            .unwrap()
            .vertices()
            .iter()
            .any(|point| point.y > 2.0)
    })); // 确认不是意外退化为全平测试。
    for workers in [1, 2, 4] {
        // 三种上限都必须完全等价。
        let layout = build::prepare_layout(&input).unwrap(); // 同一个目标窗口。
        let stamps = blank_stamps(&layout); // 所有块完整身份。
        let actual = block_on(build::cook_parallel(
            Arc::clone(&input),
            layout,
            stamps,
            BTreeSet::new(),
            sources.clone(),
            BTreeMap::new(),
            &pool,
            workers,
        ))
        .unwrap(); // 正式并行构造。
        assert_cooked_equal(&actual, &expected); // 逐物理顶点与三角面验证。
        assert!(Arc::ptr_eq(&actual.sources[&key], &source)); // 返回仍保有原源快照身份。
        assert!(
            actual
                .layout
                .as_ref()
                .unwrap()
                .chunks
                .values()
                .all(|chunk| chunk.geometry.vertices.iter().all(|vertex| vertex.y == 0.0))
        ); // 共享缓存不能被采样写高。
        let layout = build::prepare_layout(&input).unwrap(); // 相同窗口测试零变化。
        let reusable = layout.chunks.keys().copied().collect(); // 全部实际形状复用。
        let count = layout.chunks.len(); // 完整集合规模。
        let unchanged = block_on(build::cook_parallel(
            Arc::clone(&input),
            layout,
            BTreeMap::new(),
            reusable,
            BTreeMap::new(),
            BTreeMap::new(),
            &pool,
            workers,
        ))
        .unwrap(); // 故意不给新形状签名。
        assert!(unchanged.shapes.is_empty()); // 真正零次cooking。
        assert_eq!(unchanged.plans.len(), count); // 不能因复用丢失目标块。
        let bad = build::prepare_layout(&input).unwrap(); // 同样的有效几何。
        let failed = block_on(build::cook_parallel(
            Arc::clone(&input),
            bad,
            BTreeMap::new(),
            BTreeSet::new(),
            sources.clone(),
            BTreeMap::new(),
            &pool,
            workers,
        )); // 缺源身份使整批失败。
        assert!(failed.is_err()); // 不返回部分有效形状。
    } // 不修改全局物理状态。
} // 票据迟到和发布由现有运行时测试继续覆盖。

#[test] // 固定工作量比较1、2、4worker；不作为硬件无关门槛。
#[ignore = "手动CPU基准：固定旧中心0到新中心1，非游戏实机帧时"]
fn parallel_layout_and_cook_microbenchmark() {
    // 按用户场景尺度测量同一移动批次。
    let pool = TaskPoolBuilder::new().num_threads(5).build(); // 专用池隔离其他测试任务。
    let mut old = request(); // 起点继承合法场景与根身份。
    old.spec = SparseHeightSpec::new([-512.0; 2], [512.0; 2], 0.125, -80.0, 160.0).unwrap(); // 用户真实场景规格。
    old.lods = [(10.0, 1), (100.0, 4), (800.0, 32)]
        .into_iter()
        .map(|(distance, multiplier)| SceneCollisionLod {
            max_distance_meters: distance,
            spacing_multiplier: multiplier,
        })
        .collect(); // 不缩小覆盖换取性能。
    old.anchor = Vec2::ZERO; // 所有样本的统一旧中心。
    let old = Arc::new(old); // 共享旧内容身份。
    let previous = Arc::new(build::prepare_layout(&old).unwrap()); // 只构造一次已采用布局。
    let mut current = (*old).clone(); // 新内容没有高度改动。
    current.anchor = Vec2::new(1.0, 0.0); // 所有样本严格相同移动，不递进改变工作量。
    let current = Arc::new(current); // 完整冻结新需求。
    let expected = build::prepare_layout_cached(&current, Some((&old, &previous))).unwrap(); // 原串行布局权威基线。
    for (round, order) in [[1, 2, 4], [1, 4, 2], [4, 2, 1]].into_iter().enumerate() {
        // 第一轮预热，后两轮交错顺序减轻冷缓存偏差。
        for workers in order {
            // 每个样本都从同一旧缓存开始。
            let layout = block_on(build::prepare_layout_parallel(
                Arc::clone(&current),
                Some((Arc::clone(&old), Arc::clone(&previous))),
                &pool,
                workers,
            ))
            .unwrap(); // 真实并行布局。
            assert_layout_equal(&layout, &expected); // 测速不能牺牲等价性。
            let reusable = layout
                .chunks
                .iter()
                .filter(|(key, chunk)| {
                    previous
                        .chunks
                        .get(key)
                        .is_some_and(|old| old.plan == chunk.plan)
                })
                .map(|(key, _)| *key)
                .collect(); // 平地移动只重建计划变化块。
            let stamps = blank_stamps(&layout); // 显式平地身份。
            let layout_ms = layout.elapsed_ms; // 仅布局阶段墙钟。
            let rebuilt = layout.rebuilt_blocks; // 布局真实重建数量。
            let reused = layout.reused_blocks; // 布局真实复用数量。
            let cooked = block_on(build::cook_parallel(
                Arc::clone(&current),
                layout,
                stamps,
                reusable,
                BTreeMap::new(),
                BTreeMap::new(),
                &pool,
                workers,
            ))
            .unwrap(); // 固定隐式平地CPU输入，不含IO。
            eprintln!(
                "parallel_collision round={round} workers={workers} warmup={} layout_ms={layout_ms:.3} cook_ms={:.3} total_ms={:.3} rebuilt={rebuilt} reused={reused} shapes={} (CPU_only_flat_sources)",
                round == 0,
                cooked.elapsed_ms,
                layout_ms + cooked.elapsed_ms,
                cooked.shapes.len()
            ); // 报原始数据，不承诺游戏帧时。
        } // 每次候选释放，避免基准自身无限保留结果。
    } // 不断言特定倍数加速。
} // 实机帧时和CPU争用需另行验收。

#[test] // 切换场景不能因父请求失效提前归还仍有worker在工作的槽位。
fn reset_retains_parent_task_until_workers_finish() {
    // 用确定性闸门而非依赖机器快慢。
    use super::super::{
        lifecycle, parallel,
        resources::{Completion, LayeredCollisionRuntime, TaskResult},
    }; // 调用正式生命周期入口。
    use bevy::{
        prelude::{App, Update},
        tasks::futures::check_ready,
    }; // 无窗口测试及非阻塞检查。
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::{Duration, Instant},
    }; // 超时保证失败时不会永久挂住测试。
    let pool = Arc::new(TaskPoolBuilder::new().num_threads(5).build()); // 父任务及worker共享专用池。
    let worker_pool = Arc::clone(&pool); // 父任务持有池直到所有worker结束。
    let gate = Arc::new(AtomicBool::new(false)); // 控制CPU工作真实结束时刻。
    let worker_gate = Arc::clone(&gate); // 只读闸门句柄。
    let (started, received) = mpsc::channel(); // 确认至少一个worker实际开始。
    let input = Arc::new(request()); // 保留原票据完整内容身份。
    let task = pool.spawn(async move {
        // 父任务与生产一致持有全部子任务。
        let result = parallel::map(&worker_pool, (0..32).collect(), 4, move |item| {
            // 足够工作项启用四worker路径。
            started.send(()).unwrap(); // 通知测试进入在途阶段。
            let deadline = Instant::now() + Duration::from_secs(5); // 断言失败也能自动结束。
            while !worker_gate.load(Ordering::Acquire) {
                // 测试专用模拟未完成CPU工作。
                if Instant::now() >= deadline {
                    return Err("测试worker闸门超时".into());
                } // 不能让CI永久死锁。
                std::thread::sleep(Duration::from_millis(1)); // 测试阻塞不进入运行时实现。
            } // 放行后正常交付局部结果。
            Ok(item) // 任意纯数据足以验证批次生命周期。
        })
        .await; // 全worker结束才完成父任务。
        TaskResult {
            ticket: 41,
            request: input,
            completion: Completion::Layout(Err(result
                .err()
                .unwrap_or_else(|| "测试仅验证任务完成身份".into()))),
        } // 不发布假布局。
    }); // 此时任务已经在专用后台池运行。
    received.recv_timeout(Duration::from_secs(5)).unwrap(); // 确认不是仅测试未调度任务。
    let mut app = App::new(); // 无渲染和IO依赖。
    app.insert_resource(LayeredCollisionRuntime {
        task: Some(task),
        ticket: 41,
        ..Default::default()
    }); // 原图拥有在途父句柄。
    app.init_resource::<feathrise_scene_core::residency::TerrainCollisionRegion>(); // 正式清理所需承托状态。
    app.init_resource::<feathrise_scene_content::SceneCollisionLodStatus>(); // 正式清理所需诊断状态。
    app.add_systems(Update, lifecycle::clear_layered_collision); // 内部调用正式reset。
    app.update(); // 切换场景语义立即生效，不等待worker。
    let mut state = app.world_mut().resource_mut::<LayeredCollisionRuntime>(); // 查询换代后的单飞槽。
    assert_eq!(state.ticket, 42); // 原票据已经无发布权。
    assert!(state.task.is_some()); // 未完成任务仍占槽。
    assert!(state.task.as_mut().and_then(check_ready).is_none()); // 真实worker尚未完成。
    gate.store(true, Ordering::Release); // 允许所有worker实际退出。
    let completed = block_on(state.task.take().unwrap()); // 仅测试等待完整批次回收。
    assert_eq!(completed.ticket, 41); // 完成值保留旧票据，不能冒用当前票据。
    assert_ne!(completed.ticket, state.ticket); // 正式驱动应拒绝迟到结果。
} // 没有向世界安装任何部分形状。

#[test] // 保守预检超预算不应拒绝原来可接受的去重几何。
fn conservative_parallel_budget_falls_back_without_rejecting_valid_layout() {
    // 验证并行不收紧配置接受范围。
    let pool = TaskPoolBuilder::new().num_threads(5).build(); // 专用池没有外部争用。
    let mut input = request(); // 原始作者格距0.125米。
    input.spec = SparseHeightSpec::new([-512.0; 2], [512.0; 2], 0.125, -80.0, 160.0).unwrap(); // 足够大场景不裁掉测试圆。
    input.anchor = Vec2::ZERO; // 世界中央。
    input.lods = vec![SceneCollisionLod {
        max_distance_meters: 40.0,
        spacing_multiplier: 1,
    }]; // 约32万格，去重前四顶点估算超过一百万。
    let expected = build::prepare_layout(&input).unwrap(); // 原串行精确预算应接受。
    let worst_vertices: usize = expected
        .chunks
        .values()
        .map(|chunk| chunk.plan.cells.as_ref().unwrap().len() * 4)
        .sum(); // 无接缝单层普通格的保守上界。
    assert!(worst_vertices > 1_048_576); // 确认覆盖了保守回退分支。
    let actual = block_on(build::prepare_layout_parallel(
        Arc::new(input),
        None,
        &pool,
        4,
    ))
    .unwrap(); // 不应该因并行预检误拒配置。
    assert_layout_equal(&actual, &expected); // 回退仍保持逐项精确等价。
} // 此测试不对回退速度做承诺。
