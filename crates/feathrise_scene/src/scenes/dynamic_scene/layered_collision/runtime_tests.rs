//! 布局、源页局部依赖与后台形状构造回归；仅纯CPU测试，不运行游戏窗口。
#[path = "parallel_build_tests.rs"] // 复用本文件夹具，独立验证多worker与串行结果等价。
mod parallel_build_tests; // 专用测试线程池不修改游戏全局池。
#[path = "runtime_planning_cache_tests.rs"] // 跨次规划只随完整采用布局更新，独立审计生命周期。
mod runtime_planning_cache_tests; // 不把单次纯算法等价冒充发布安全。
#[path = "terrain_cutout_tests.rs"]
mod terrain_cutout_tests;
use super::{
    build,
    resources::{Request, SourceStamp},
    types::ChunkKey,
};
use bevy::prelude::{Entity, Vec2}; // 小型身份与玩家中心。
use feathrise_scene_content::sparse_height::{SparseHeightTile, SparseHeightTileKey}; // 唯一作者格点所有者。
use feathrise_scene_content::{SceneCollisionLod, SceneId, SparseHeightSpec}; // 真实场景规格。
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
}; // 使用与运行时相同的不可变源快照。 // 正式后台入口。

fn request() -> Request {
    // 非平凡尺寸不需要整图高度数组。
    Request {
        stitch_profiles:Default::default(),
        stitches:Default::default(),
        holes: Default::default(),
        // 默认两层与真实128米玩家中心。
        scene_id: SceneId::parse("MAP-layered-runtime-tests").unwrap(), // 合法稳定ID。
        root: Entity::from_bits(5),                                 // 独立根身份。
        spec: SparseHeightSpec::new([0.0; 2], [512.0; 2], 0.125, -128.0, 256.0).unwrap(), // 最高四级父页。
        manifest_revision: 1,  // 正式版本。
        live_revision: None,   // 初始无编辑覆盖。
        crater_revision: None, // 初始无技能浅坑。
        lods: vec![
            SceneCollisionLod {
                max_distance_meters: 16.0,
                spacing_multiplier: 1,
            },
            SceneCollisionLod {
                max_distance_meters: 64.0,
                spacing_multiplier: 4,
            },
        ], // 用户确认的起点。
        anchor: Vec2::splat(128.0), // 碰撞中心与相机无关。
    } // 不含大型高度数组。
} // 所有测试使用生产规划和采样。

fn blank_stamps(layout: &super::resources::Layout) -> BTreeMap<ChunkKey, SourceStamp> {
    // 构造显式平地的局部内容身份。
    layout
        .chunks
        .iter()
        .map(|(key, chunk)| {
            (
                *key,
                SourceStamp {
                    sources: chunk
                        .tiles
                        .iter()
                        .map(|tile| (*tile, String::new(), 0, 0))
                        .collect(),
                },
            )
        })
        .collect() // 每块仅持有自己的实际页集合。
} // 与平地缺席语义一致。

#[test] // 移动只重建变化块，几何与无缓存构建逐项相等。
fn cached_layout_shares_unchanged_topology_and_matches_cold_layout() {
    let old = request(); // 完整采用基线。
    let previous = build::prepare_layout(&old).unwrap(); // 零高度拓扑缓存。
    let mut moved = old.clone(); // 相同场景与作者规格。
    moved.anchor.x += 0.5; // 小幅移动改变圆周与必要接缝。
    let cached = build::prepare_layout_cached(&moved, Some((&old, &previous))).unwrap(); // 正式提前复用入口。
    let cold = build::prepare_layout(&moved).unwrap(); // 无缓存权威对照。
    assert!(cached.reused_blocks > 0 && cached.rebuilt_blocks > 0); // 不是整批重建，也不是错误冻结旧窗口。
    assert_eq!(
        cached.rebuilt_blocks + cached.reused_blocks,
        cold.chunks.len()
    ); // 每个目标块恰好分类一次。
    assert_eq!(cached.tiles, cold.tiles); // 源页依赖没有少算。
    assert_eq!(cached.supported_rows, cold.supported_rows); // 实际外缘覆盖完全一致。
    for (key, chunk) in &cached.chunks {
        // 对照全部最终网格，不只对照块数。
        let reference = &cold.chunks[key]; // 相同稳定键。
        assert_eq!(chunk.plan, reference.plan); // 精度与接缝身份一致。
        assert_eq!(chunk.geometry.vertices, reference.geometry.vertices); // 顶点坐标完全一致。
        assert_eq!(chunk.geometry.triangles, reference.geometry.triangles); // 三角拓扑完全一致。
        assert_eq!(
            chunk.geometry.base_grid_points,
            reference.geometry.base_grid_points
        ); // 页采样合同完全一致。
        assert_eq!(chunk.tiles, reference.tiles); // 每块局部依赖也完全一致。
        if let Some(old_chunk) = previous
            .chunks
            .get(key)
            .filter(|old_chunk| old_chunk.plan == chunk.plan)
        {
            // 相同身份必须是真正共享。
            assert!(Arc::ptr_eq(old_chunk, chunk)); // 证明没有深拷贝后冒充复用。
        } // 变化块不要求共享。
    } // 测试不改用户资源。
}

#[test] // 编辑、撤销与失败不应污染缓存零高度，而切换场景必须冷构造。
fn cached_layout_survives_height_edits_but_rejects_other_scene_or_root() {
    let old = request(); // 已采用身份。
    let previous = build::prepare_layout(&old).unwrap(); // 只缓存零高度拓扑。
    let mut edited = old.clone(); // 局部编辑只改变高度源身份。
    edited.live_revision = Some(1); // 不能用全局修订强制重建所有拓扑。
    let cached = build::prepare_layout_cached(&edited, Some((&old, &previous))).unwrap(); // 全部拓扑仍有效。
    assert_eq!(cached.rebuilt_blocks, 0); // 源页变化留给SourceStamp与cook处理。
    assert_eq!(cached.reused_blocks, previous.chunks.len()); // 全部零高网格共享。
    let count = cached.chunks.len(); // 保留完整目标数。
    assert!(
        build::cook(
            &edited,
            cached,
            BTreeMap::new(),
            BTreeSet::new(),
            BTreeMap::new(),
            BTreeMap::new()
        )
        .is_err()
    ); // 故意缺签名失败，不能覆盖基线。
    let restored = build::prepare_layout_cached(&old, Some((&old, &previous))).unwrap(); // 撤销或失败恢复仍用同一已采用缓存。
    assert_eq!(restored.reused_blocks, count); // 没有被失败候选污染。
    assert!(
        previous.chunks.values().all(|chunk| chunk
            .geometry
            .vertices
            .iter()
            .all(|vertex| vertex.y == 0.0))
    ); // cooking不能原位写共享拓扑高度。
    for change_root in [false, true] {
        // 两种会话隔离独立覆盖。
        let mut other = old.clone(); // 规格和位置故意完全相同。
        if change_root {
            other.root = Entity::from_bits(11);
        } else {
            other.scene_id = SceneId::parse("MAP-layout-other").unwrap();
        } // 换场景或根代次。
        let next = build::prepare_layout_cached(&other, Some((&old, &previous))).unwrap(); // 即使键相同也不能跨会话复用。
        assert_eq!(next.reused_blocks, 0); // 冷构造保证身份隔离。
        assert_eq!(next.rebuilt_blocks, count); // 几何本身不受缓存策略改变。
    } // 不依赖真实AssetServer。
}

#[test] // 按需观察后台布局缓存收益；CPU微基准不代表游戏帧时。
#[ignore = "手动测量冷/热布局耗时，不作为硬件无关性能门槛"]
fn layout_cache_microbenchmark() {
    let mut previous_request = request(); // 采用用户实际场景尺度。
    previous_request.spec =
        SparseHeightSpec::new([-512.0; 2], [512.0; 2], 0.125, -80.0, 160.0).unwrap(); // 原始0.125米格距。
    previous_request.lods = [(10.0, 1), (100.0, 4), (800.0, 32)]
        .into_iter()
        .map(|(distance, multiplier)| SceneCollisionLod {
            max_distance_meters: distance,
            spacing_multiplier: multiplier,
        })
        .collect(); // 不缩小范围或降低精度。
    previous_request.anchor = Vec2::ZERO; // 起始玩家位置。
    let mut previous = build::prepare_layout(&previous_request).unwrap(); // 模拟初次完整采用。
    for x in [0.0, 1.0, 2.0, 3.0] {
        // 后续结果递进成为下次采用基线。
        let mut current = previous_request.clone(); // 内容身份保持相同。
        current.anchor = Vec2::new(x, 0.0); // 玩家沿X方向移动，不引入预取。
        let cold = build::prepare_layout(&current).unwrap(); // 本位置完整冷构造。
        let cached =
            build::prepare_layout_cached(&current, Some((&previous_request, &previous))).unwrap(); // 同位置真实提前复用。
        eprintln!(
            "layout_cache x={x:.1} cold_ms={:.3} cached_ms={:.3} reused={} rebuilt={} total={}",
            cold.elapsed_ms,
            cached.elapsed_ms,
            cached.reused_blocks,
            cached.rebuilt_blocks,
            cached.chunks.len()
        ); // 输出实测不对耗时做断言。
        assert_eq!(cached.chunks.len(), cold.chunks.len()); // 完整目标数量一致。
        assert_eq!(cached.tiles, cold.tiles); // 总页依赖一致。
        assert_eq!(cached.supported_rows, cold.supported_rows); // 实际承托覆盖一致。
        assert_eq!(cached.supported_polygon, cold.supported_polygon); // 兼容轮廓字段一致。
        for (key, chunk) in &cached.chunks {
            // 每个块逐项审计，避免以缓存错误换性能。
            let reference = &cold.chunks[key]; // 键不存在立即失败。
            assert_eq!(chunk.plan, reference.plan); // 单元与接缝身份相同。
            assert_eq!(chunk.geometry.vertices, reference.geometry.vertices); // 世界顶点相同。
            assert_eq!(
                chunk.geometry.base_grid_points,
                reference.geometry.base_grid_points
            ); // 作者采样点相同。
            assert_eq!(chunk.geometry.triangles, reference.geometry.triangles); // 三角面拓扑相同。
            assert_eq!(chunk.tiles, reference.tiles); // 本块依赖相同。
        } // 比较本身不计入布局内测量时钟。
        previous_request = current; // 模拟该完整版本成功采用。
        previous = cached; // 下轮只能从上一采用结果复用。
    } // 不创建窗口、不修改场景或配置。
}

#[test] // 用户800米远层覆盖1024米场景，依赖位置超过512不再阻止首次加载。
fn wide_collision_coverage_exceeding_512_source_positions_cooks() {
    let mut input = request(); // 同一生产构建入口。
    input.spec = SparseHeightSpec::new([-512.0; 2], [512.0; 2], 0.125, -80.0, 160.0).unwrap(); // 用户场景尺寸与作者格距。
    input.anchor = Vec2::ZERO; // 默认出生点。
    input.lods = [(10.0, 1), (100.0, 4), (800.0, 32)]
        .into_iter()
        .map(|(distance, multiplier)| SceneCollisionLod {
            max_distance_meters: distance,
            spacing_multiplier: multiplier,
        })
        .collect(); // 不缩小半径、不降低配置精度。
    super::super::layered_collision::validate_layered_collision_settings(&input.spec, &input.lods)
        .unwrap(); // 保存侧相同配置能通过。
    let layout = build::prepare_layout(&input).unwrap(); // 重启侧布局也必须通过。
    assert!(layout.tiles.len() > 512); // 明确命中旧版拒绝条件。
    let stamps = blank_stamps(&layout); // 隐式平地没有实际高度页分配。
    let cooked = build::cook(
        &input,
        layout,
        stamps,
        BTreeSet::new(),
        BTreeMap::new(),
        BTreeMap::new(),
    )
    .unwrap(); // 正式后台形状构建。
    assert!(!cooked.shapes.is_empty()); // 不是用空候选绕过报错。
    assert!(cooked.sources.is_empty()); // 空白依赖位置不冒充驻留内存页。
} // 无磁盘写入、不改用户场景。

#[test] // 全平场景也必须遵守有限碰撞覆盖。
fn flat_default_layout_cooks_only_bounded_window() {
    // 默认计划含移动余量。
    let request = request(); // 512米测试图。
    let layout = build::prepare_layout(&request).unwrap(); // 正式布局检查含过渡块预算。
    let count = layout.chunks.len(); // 冻结理论构造数。
    assert!(count > 1 && count <= 512); // 不允许回退为四顶点整图平面。
    assert!(!layout.tiles.is_empty()); // 有限几何确定实际依赖，不能用固定512页限制覆盖。
    let stamps = blank_stamps(&layout); // 在转移布局所有权前记录同版身份。
    let cooked = build::cook(
        &request,
        layout,
        stamps,
        BTreeSet::new(),
        BTreeMap::new(),
        BTreeMap::new(),
    )
    .unwrap(); // 隐式平地不分配源页。
    assert_eq!(cooked.shapes.len(), count); // 首次每块都必须得到完整形状。
    assert_eq!(cooked.plans.len(), count); // 发布集合没有丢块。
    assert_eq!(cooked.bounds.len(), count); // 活动门与同版真实几何保持一致。
    assert!(cooked.sources.is_empty()); // 隐式平地没有伪造常驻页。
    assert!(
        cooked
            .bounds
            .iter()
            .all(|(lo, hi)| lo.x > 0.0 && lo.y > 0.0 && hi.x < 512.0 && hi.y < 512.0)
    ); // 没有整图承托捷径。
} // 物理预算不随空白场景面积膨胀。

#[test] // 所有块均复用时不能偷偷重新采样或重新cooking。
fn unchanged_layout_reuses_every_shape_without_sources() {
    // 独立于诊断全局修订。
    let request = request(); // 稳定窗口。
    let layout = build::prepare_layout(&request).unwrap(); // 正式几何集合。
    let reusable: BTreeSet<_> = layout.chunks.keys().copied().collect(); // 模拟已通过实际源签名比较。
    let count = reusable.len(); // 发布集合依然完整。
    let cooked = build::cook(
        &request,
        layout,
        BTreeMap::new(),
        reusable,
        BTreeMap::new(),
        BTreeMap::new(),
    )
    .unwrap(); // 故意不给新形状签名，验证复用短路。
    assert!(cooked.shapes.is_empty()); // 必须零次新形状构造。
    assert_eq!(cooked.plans.len(), count); // 保留块不能从最终目标消失。
    assert_eq!(cooked.bounds.len(), count); // 活动门仍获得全套当前覆盖。
} // 复用不是省略发布身份。

#[test] // 编辑只触及本页采样的块，不以全局修订把整个窗口都判脏。
fn local_source_page_changes_only_dependent_chunks() {
    // 使用正式布局依赖而非模拟空间距离。
    let request = request(); // 多作者页的有界窗口。
    let layout = build::prepare_layout(&request).unwrap(); // 所有块真实所需的格点页。
    let changed = SparseHeightTileKey { x: 4, z: 4 }; // 玩家所在32米作者页。
    let before = blank_stamps(&layout); // 原全平页签名。
    let after: BTreeMap<_, _> = before
        .iter()
        .map(|(key, stamp)| {
            // 只修改一页的实时内容身份。
            let mut stamp = stamp.clone(); // 小型局部身份副本。
            for source in &mut stamp.sources {
                if source.0 == changed {
                    source.2 = 1;
                }
            } // 内容修订不触碰其他页。
            (*key, stamp) // 保留稳定块键。
        })
        .collect(); // 新签名完整。
    let dirty: BTreeSet<_> = layout
        .chunks
        .iter()
        .filter(|(key, _)| before.get(*key) != after.get(*key))
        .map(|(key, _)| *key)
        .collect(); // 同生产比较语义。
    assert!(!dirty.is_empty() && dirty.len() < layout.chunks.len()); // 本次只局部失效。
    for (key, chunk) in &layout.chunks {
        assert_eq!(dirty.contains(key), chunk.tiles.contains(&changed));
    } // 共享边邻块也由实际采样依赖覆盖。
    let reusable = layout
        .chunks
        .keys()
        .filter(|key| !dirty.contains(*key))
        .copied()
        .collect(); // 其余叶原样保留。
    let cooked = build::cook(
        &request,
        layout,
        after,
        reusable,
        BTreeMap::new(),
        BTreeMap::new(),
    )
    .unwrap(); // 仅构造局部集合。
    assert_eq!(cooked.shapes.len(), dirty.len()); // 后台真实构造数量等于局部脏块数。
} // 源身份提取本身仍由系统集成测试覆盖。

#[test] // 角落格和非整数扇形中心不能发生数组越界。
fn sampling_corners_clip_last_grid_point_without_round_trip() {
    // 浮点世界反投影不参与。
    let request = request(); // 格点总数4096。
    assert_eq!(
        build::sample_corners(&request.spec, [4095.5, 4095.5]),
        [[4095, 4095], [4096, 4095], [4095, 4096], [4096, 4096]]
    ); // 半整数尾格使用四个合法源点。
    assert_eq!(
        build::sample_corners(&request.spec, [4096.0, 4096.0]),
        [[4096, 4096]; 4]
    ); // 全图终点只属于最后作者页。
} // 不采样越界的下一格。

#[test] // 原始格点非平高度通过源Arc传递，候选构造不会覆盖作者数据。
fn source_snapshot_is_retained_and_missing_shape_identity_fails() {
    // 背景构造必须完整返回或完整失败。
    let mut request = request(); // 缩小测试窗口以减少无关cooking。
    request.lods = vec![SceneCollisionLod {
        max_distance_meters: 1.0,
        spacing_multiplier: 1,
    }]; // 单层也受固定移动余量保护。
    let layout = build::prepare_layout(&request).unwrap(); // 真实有限布局。
    let stamps = blank_stamps(&layout); // 完整形状签名。
    let key = SparseHeightTileKey { x: 4, z: 4 }; // 样例非平源。
    let mut tile = SparseHeightTile::blank(&request.spec, key).unwrap(); // 单页有界分配。
    tile.set([0, 0], request.spec.encode_height(3.0).unwrap())
        .unwrap(); // 改变一个作者格点。
    let source = Arc::new(tile); // 构造不可变快照。
    let cooked = build::cook(
        &request,
        layout,
        stamps,
        BTreeSet::new(),
        BTreeMap::from([(key, Arc::clone(&source))]),
        BTreeMap::new(),
    )
    .unwrap(); // 后台只浅克隆源。
    assert!(Arc::ptr_eq(cooked.sources.get(&key).unwrap(), &source)); // 旧版源身份在采用后继续存活。
    assert_eq!(
        source.get([0, 0]),
        Some(request.spec.encode_height(3.0).unwrap())
    ); // 构造没有改写源。
    assert!(
        cooked
            .layout
            .as_ref()
            .unwrap()
            .chunks
            .values()
            .all(|chunk| chunk.geometry.vertices.iter().all(|vertex| vertex.y == 0.0))
    ); // 非平采样只写新形状副本，下一候选复用的拓扑仍是零高度。
    let bad_layout = build::prepare_layout(&request).unwrap(); // 相同几何但缺少身份。
    assert!(
        build::cook(
            &request,
            bad_layout,
            BTreeMap::new(),
            BTreeSet::new(),
            BTreeMap::new(),
            BTreeMap::new()
        )
        .is_err()
    ); // 不产生可误采用的半批次。
} // 失败采用策略交给主世界票据门。

#[test] // 鞍面可区分作者三角插值和错误的双线性采样。
fn precise_fractional_samples_follow_author_triangle_diagonal() {
    // f32米制代码必须通过spec编码，不能把旧u16整数码当作浮点位模式。
    let spec = SparseHeightSpec::new([0.0; 2], [513.0; 2], 1.0, 0.0, 65535.0).unwrap(); // 尾格总数为奇数，四个整数米高程可精确表达。
    let key = SparseHeightTileKey { x: 2, z: 2 }; // 最后页拥有512、513两个端点。
    let mut tile = SparseHeightTile::blank(&spec, key).unwrap(); // 只分配最后2乘2源格点。
    tile.set([0, 0], spec.encode_height(0.0).unwrap()).unwrap(); // 西南角零高度。
    tile.set([1, 0], spec.encode_height(4.0).unwrap()).unwrap(); // 东南角四米。
    tile.set([0, 1], spec.encode_height(8.0).unwrap()).unwrap(); // 西北角八米。
    tile.set([1, 1], spec.encode_height(20.0).unwrap()).unwrap(); // 东北角二十米形成非共面四点。
    let tiles = BTreeMap::from([(key, Arc::new(tile))]); // 后台统一权威源。
    let craters = BTreeMap::new(); // 本例单独审计作者高度插值。
    assert_eq!(
        build::sample_height(&spec, &tiles, &craters, [512.25, 512.25]).unwrap(),
        3.0
    ); // 下三角：0+0.25*4+0.25*8。
    assert_eq!(
        build::sample_height(&spec, &tiles, &craters, [512.75, 512.75]).unwrap(),
        13.0
    ); // 上三角：20+0.25*(4-20)+0.25*(8-20)。
    assert_eq!(
        build::sample_height(&spec, &tiles, &craters, [512.5, 512.5]).unwrap(),
        6.0
    ); // 尾格扇形中心落在共享对角线，双线性错误结果会是8。
    assert_eq!(
        build::sample_height(&spec, &tiles, &craters, [513.0, 513.0]).unwrap(),
        20.0
    ); // 全图端点不越界且保留权威高度。
    assert_eq!(
        build::sample_height(&spec, &tiles, &craters, [513.0, 512.5]).unwrap(),
        12.0
    ); // 边界只有一条合法线段。
} // 半整数尾格与整数源点共享同一稳定采样合同。

#[test] // 刷新应发生在靠近细块中线时，不等到走完一个块或碰撞范围边缘。
fn movement_requests_start_early_and_keep_boundary_hysteresis() {
    let old = request(); // 锚点128，细块宽4米。
    let center = old.anchor + Vec2::new(2.51, 0.0); // 细块边缘之前已需要刷新。
    let next = build::select_request_anchor(&old, Some(&old), center, false, Some(old.anchor)); // 没有后台候选，应提前请求。
    assert_eq!(next, center); // 围绕真实位置，不再量化圆心到块中心。
    let mut pending = old.clone(); // 模拟已更新的稳定锚点。
    pending.anchor = next; // 接近边界时不被小抖动立即撤回。
    assert_eq!(
        build::select_request_anchor(
            &pending,
            Some(&pending),
            old.anchor + Vec2::new(1.99, 0.0),
            false,      // 当前没有在途任务。
            Some(next), // 上帧位置不是不断重置的候选圆心。
        ),
        next
    ); // 小幅往返只合并中心，不增加任何实际覆盖。
    assert!(center.distance(old.anchor) < old.lods[0].max_distance_meters); // 触发远早于最近层和整体窗口边缘。
} // 固定世界块身份仍然可复用。

#[test] // 手动控制后台完成阶段，持续移动跨多块也应有可采用的完整候选。
fn moving_across_blocks_keeps_valid_candidate_then_immediately_catches_up() {
    let original = request(); // 当前已采用中心128米。
    let mut candidate = original.clone(); // 准备提前刷新请求。
    let first_center = original.anchor + Vec2::new(2.6, 0.0); // 第一触发点。
    candidate.anchor = build::select_request_anchor(
        &candidate,
        Some(&original),
        first_center,
        false,
        Some(original.anchor),
    ); // 围绕触发位置开始。
    let completed_request = Arc::new(candidate.clone()); // 模拟布局/IO/cook各阶段冻结的同一完整身份。
    for offset in [4.1, 6.1, 8.1, 10.1] {
        // 玩家持续运动，并未停到旧窗口边缘。
        let center = original.anchor + Vec2::new(offset, 0.0); // 穿过多个细块边界。
        let mut current = original.clone(); // 最新源与配置完全相同。
        current.anchor = build::select_request_anchor(
            &current,
            Some(&candidate),
            center,
            true,
            Some(first_center),
        ); // 模拟候选尚未完成。
        assert_eq!(&current, completed_request.as_ref()); // 整体Request相等，真实票据驱动不会清空layout或废弃完成结果。
        candidate = current; // 每个模拟帧继续沿用有效候选。
    } // 最后一帧仍保持完整可采用身份。
    let latest_center = original.anchor + Vec2::new(10.1, 0.0); // 模拟此帧后台完成并正式采用。
    let next_anchor = build::select_request_anchor(
        &candidate,
        Some(&candidate),
        latest_center,
        false,
        Some(latest_center),
    ); // 下一帧立即追赶最新玩家，不要求静止。
    assert_eq!(next_anchor, latest_center); // 后续候选立即追赶真实玩家圆心。
    assert_ne!(next_anchor, candidate.anchor); // 确实产生后续请求而不是一直锁住旧窗口。
} // 安全范围内只合并移动，不放宽完成值完整身份校验。

#[test] // 用户1米/100米配置：慢后台下持续移动必须完成，不能等外缘停止。
fn tiny_near_large_far_completes_during_continuous_movement() {
    let mut candidate = request(); // 正式0.125米作者格距。
    candidate.lods[0].max_distance_meters = 1.0; // 用户近层参数。
    candidate.lods[1] = SceneCollisionLod {
        max_distance_meters: 100.0,
        spacing_multiplier: 8,
    }; // 用户远层参数。
    let start = candidate.anchor; // 冻结旧位置，仅用于验证未到100米外缘。
    let mut center = start; // 最新玩家位置，模拟不断向前走。
    for _ in 0..2 {
        // 两轮完整构造和追赶，不能只保证第一批偶然成功。
        let frozen = candidate.clone(); // 布局/加载/烘焙均绑定此内容和圆心。
        let layout = build::prepare_layout(&frozen).unwrap(); // 生产入口，确保这组参数可以构造。
        let stamps = blank_stamps(&layout); // 布局阶段冻结合法平地源身份。
        for _ in 0..120 {
            // 人为延迟120帧；不以开发机实际构建快慢掩盖饥饿。
            let last = center; // 上帧真实位置。
            center.x += 0.05; // 连续移动6米，远超过旧0.5米作废阈值。
            candidate.anchor =
                build::select_request_anchor(&candidate, Some(&frozen), center, true, Some(last)); // 与运行时相同的冻结规则。
            assert_eq!(candidate, frozen); // 完整身份不变，driver不得递增票据或拒绝此候选。
        } // 此时玩家仍在100米窗口内部且一直在走。
        let cooked = build::cook(
            &frozen,
            layout,
            stamps,
            BTreeSet::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        )
        .unwrap(); // 真正完成物理形状。
        assert!(!cooked.shapes.is_empty()); // 不是只让空任务通过。
        assert!(center.distance(start) < 100.0); // 完成不依赖走到外缘或停步。
        candidate.anchor =
            build::select_request_anchor(&candidate, Some(&frozen), center, false, Some(center)); // 完成后立即追赶合并的最新位置。
        assert_eq!(candidate.anchor, center); // 不永久卡在上一批圆心。
        assert_ne!(candidate.anchor, frozen.anchor); // 下一批确实前进。
    } // 有界一批接一批，不增加并发或覆盖。
}

#[test] // 完成的候选落在回头玩家另一侧时，不能撤掉仍有效的旧地面。
fn turning_back_preserves_existing_ground() {
    let mut input = request(); // 正式后台入口生成候选，不用假轮廓。
    input.anchor = Vec2::new(130.0, 128.0); // 候选在旧中心右侧2米。
    input.lods = vec![SceneCollisionLod {
        max_distance_meters: 4.0,
        spacing_multiplier: 4,
    }]; // 小覆盖便于制造回头离开候选。
    let layout = build::prepare_layout(&input).unwrap(); // 世界固定格布局。
    let stamps = blank_stamps(&layout); // 完整平地源。
    let cooked = build::cook(
        &input,
        layout,
        stamps,
        BTreeSet::new(),
        BTreeMap::new(),
        BTreeMap::new(),
    )
    .unwrap(); // 真正可发布的候选。
    let mut region = feathrise_scene_core::residency::TerrainCollisionRegion {
        center: Vec2::new(125.0, 128.0),
        supported_bounds: vec![(Vec2::splat(124.0), Vec2::splat(132.0))],
        ..Default::default()
    }; // 旧中心128的有效地面仍承托回头后的玩家。
    assert!(!super::publication::preserves_player_ground(
        &region, &cooked
    )); // 不采用右方候选撤掉左方旧地面。
    region.center = input.anchor; // 玩家在新候选内时可正常替换。
    assert!(super::publication::preserves_player_ground(
        &region, &cooked
    )); // 正常前进不会被保护门阻塞。
    region.supported_bounds.clear(); // 首次进入尚无旧地面。
    assert!(super::publication::preserves_player_ground(
        &region, &cooked
    )); // 允许第一批完整结果发布。
}

#[test] // 累计长距离走动不是单帧传送，不能重新引入另一种半径超时。
fn cumulative_movement_does_not_cancel_pending_candidate() {
    let old = request(); // 默认64米外层。
    let mut last = old.anchor; // 每帧位置独立于冻结圆心。
    for _ in 0..200 {
        // 累计100米超过整个外层，仍不把普通帧位移误判为传送。
        let center = last + Vec2::new(0.5, 0.0); // 实际承托保护可暂停实体；请求算法本身必须有完成机会。
        assert_eq!(
            build::select_request_anchor(&old, Some(&old), center, true, Some(last)),
            old.anchor
        ); // 在途保持有效。
        last = center; // 只保存最新一个位置，无历史队列。
    } // 覆盖外跳转由独立测试验证。
}

#[test] // 单帧跨覆盖跳转或内容变化必须换代，不能借防饥饿复活旧版本。
fn teleport_and_source_configuration_changes_invalidate_movement_candidate() {
    let old = request(); // 完整旧内容身份。
    let far = old.anchor + Vec2::new(old.lods.last().unwrap().max_distance_meters + 8.0, 0.0); // 单帧跨整个覆盖跳转必须重定向。
    assert_ne!(
        build::select_request_anchor(&old, Some(&old), far, true, Some(old.anchor)), // 明确提供跳转前位置。
        old.anchor
    ); // 传送和极高速必须重定向，旧承托门仍负责安全。
    let center = old.anchor + Vec2::new(2.1, 0.0); // 原候选安全范围内但不应忽略新内容。
    for field in 0..6 {
        // 正式源、草稿、技能、配置、根和场景均单独验收。
        let mut changed = old.clone(); // 每次仅改变一个维度。
        match field {
            // 对照完整Request身份合同。
            0 => changed.manifest_revision += 1, // 正式地形修订。
            1 => changed.live_revision = Some(1), // F12作者覆盖。
            2 => changed.crater_revision = Some(1), // 会话技能坑。
            3 => changed.lods[0].max_distance_meters += 1.0, // 配置保存。
            4 => changed.root = Entity::from_bits(7), // 同图根换代。
            _ => changed.scene_id = SceneId::parse("MAP-movement-new-source").unwrap(), // 跨场景。
        } // 新内容不能继承普通移动的冻结豁免。
        changed.anchor =
            build::select_request_anchor(&changed, Some(&old), center, true, Some(old.anchor)); // 内容换代不受移动合并保护。
        assert_eq!(changed.anchor, center); // 没有保留旧中心或格点量化偏移。
        assert_ne!(changed, old); // 驱动比较完整Request会递增ticket并拒绝旧完成值。
    } // 每个身份维度都仍严格换代。
} // 不承诺任意速度都能赶上有限后台预算。

#[test] // 用户截图参数必须走不加缓冲的真实后台布局，而非只测纯规划。
fn circular_layout_keeps_small_configured_window() {
    let mut input = request(); // 真实0.125米作者格距。
    input.lods = vec![
        SceneCollisionLod {
            max_distance_meters: 1.0,
            spacing_multiplier: 4,
        },
        SceneCollisionLod {
            max_distance_meters: 4.0,
            spacing_multiplier: 8,
        },
    ]; // 用户截图配置。
    let expected = super::planning::plan(&input.spec, &input.lods, input.anchor).unwrap(); // 不带隐藏余量的唯一基线。
    let layout = build::prepare_layout(&input).unwrap(); // 运行时正式入口。
    assert_eq!(
        layout.chunks.keys().copied().collect::<Vec<_>>(),
        expected.chunks.keys().copied().collect::<Vec<_>>()
    ); // 发布候选严格等于原始距离规划。
    for chunk in layout.chunks.values() {
        // 审计实际几何，不仅比诊断层数。
        for point in &chunk.geometry.vertices {
            // 远层每格1米，取整最多增加一个格子对角线，不按管理块或预取外扩。
            assert!(
                Vec2::new(point.x, point.z).distance(input.anchor) <= 4.0 + 2.0_f32.sqrt() + 0.001
            ); // 仍是有限的小范围。
        } // 全部网格顶点受界。
    } // 不隐藏F1任何三角面。

    let moved = input.anchor + Vec2::new(0.3, 0.0); // 未走到1米近层边缘。
    assert_eq!(
        build::select_request_anchor(&input, Some(&input), moved, false, Some(input.anchor)), // 小范围同样提前触发。
        moved
    ); // 不需要额外覆盖也应提前提出更新请求。
} // 未就绪地面仍由原承托门保护。

#[test]
fn world_metric_session_crater_reaches_real_cooked_collision_without_changing_author_source() {
    let mut input = request();
    // 与生产最小64米作者规格一致，0.125米格距下世界原点落在第256格。
    input.spec = SparseHeightSpec::new([-32.0; 2], [32.0; 2], 0.125, -8.0, 16.0).unwrap();
    input.anchor = Vec2::ZERO;
    input.lods = vec![SceneCollisionLod { max_distance_meters: 8.0, spacing_multiplier: 1 }];
    let mut craters = feathrise_scene_content::SparseHeightSessionCrater::default();
    craters.clear(Some(input.scene_id.clone()));
    craters.apply_world_metric(&input.spec, Vec2::ZERO, 4.0, 0.5, bevy::prelude::Vec3::new(2.0, 3.0, 0.5),
        crate::scenes::dynamic_scene::session_crater::soil_surface_for_test(&input.spec)).unwrap();
    input.crater_revision = Some(craters.revision());
    let layout = build::prepare_layout(&input).unwrap();
    let chunk = layout.chunks.values().find(|chunk| chunk.geometry.base_grid_points.contains(&[256.0, 256.0])).unwrap();
    let (source_key, local) = input.spec.tile_for_grid_point(256, 256).unwrap();
    let author = Arc::new(SparseHeightTile::blank(&input.spec, source_key).unwrap());
    let before = author.get(local);
    assert!(before.is_some(), "必须核对真实存在的作者局部格点，不能用两次越界None证明数据未变");
    let sources = BTreeMap::from([(source_key, author.clone())]);
    let collider = build::cook_chunk(&input.spec, chunk, &sources, craters.tiles(), &input.stitches).unwrap();
    let ray = avian3d::parry::query::Ray::new(bevy::prelude::Vec3::new(0.0, 2.0, 0.0), bevy::prelude::Vec3::NEG_Y);
    let distance = collider.shape().cast_local_ray(&ray, 10.0, false).unwrap();
    let expected = 2.0 - input.spec.decode_height(input.spec.flat_height_code()) + 0.5 / 3.0;
    assert!((distance - expected).abs() < 0.001, "真实碰撞坑底必须消费同一源空间浅坑深度");
    assert_eq!(author.get(local), before, "烘焙只合成会话偏移，作者页不能被写成技能坑");
}

#[test]
fn unbounded_float_collision_cooks_real_hills_and_valleys_outside_legacy_range() {
    for height in [-384.5_f32, 512.25] {
        let mut input = request();
        input.spec = SparseHeightSpec::new([0.0; 2], [64.0; 2], 1.0, -8.0, 16.0).unwrap();
        input.anchor = Vec2::splat(32.0);
        input.lods = vec![SceneCollisionLod { max_distance_meters: 8.0, spacing_multiplier: 1 }];
        let layout = build::prepare_layout(&input).unwrap();
        let chunk = layout.chunks.values().find(|chunk| !chunk.geometry.triangles.is_empty()).unwrap();
        let key = SparseHeightTileKey { x: 0, z: 0 };
        let mut tile = SparseHeightTile::blank(&input.spec, key).unwrap();
        let code = input.spec.encode_height(height).unwrap();
        for z in 0..tile.shape[1] {
            for x in 0..tile.shape[0] { tile.set([x, z], code).unwrap(); }
        }
        let sources = BTreeMap::from([(key, Arc::new(tile))]);
        let collider = build::cook_chunk(&input.spec, chunk, &sources, &BTreeMap::new(), &input.stitches).unwrap();
        let triangle = chunk.geometry.triangles[0];
        let center = triangle.into_iter().map(|index| chunk.geometry.vertices[index as usize])
            .fold(bevy::prelude::Vec3::ZERO, |sum, vertex| sum + vertex) / 3.0;
        let ray = avian3d::parry::query::Ray::new(
            bevy::prelude::Vec3::new(center.x, height + 20.0, center.z), bevy::prelude::Vec3::NEG_Y);
        let distance = collider.shape().cast_local_ray(&ray, 40.0, false).unwrap();
        assert!((distance - 20.0).abs() < 0.001, "真实碰撞不能截回旧±8米：{height}");
    }
}
