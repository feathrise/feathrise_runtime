//! 跨刷新规划快照的真实布局、采样和发布生命周期回归；不启动图形或物理步。
use super::super::super::resources::{DynamicMapRuntimeState, DynamicMapRuntimeStatus}; // 成功与失败发布状态。
use super::super::{
    build, lifecycle, publication,
    resources::{LayeredCollisionRuntime, Layout, Request},
}; // 测试真实生产入口。
use super::{blank_stamps, request}; // 复用正式地图与完整源身份夹具。
use bevy::{
    ecs::world::CommandQueue,
    prelude::*,
    tasks::{TaskPool, TaskPoolBuilder, block_on},
}; // 阻塞只发生在专用测试线程。
use feathrise_scene_core::residency::TerrainCollisionRegion; // 承托必须和缓存同版。
use feathrise_map_content::sparse_height::{SparseHeightTile, SparseHeightTileKey}; // 非平高度快照。
use feathrise_map_content::{MapCollisionLod, MapCollisionLodStatus, MapId, SparseHeightSpec}; // 完整配置与规格身份。
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
}; // 保持源与缓存不可变。

fn small_request() -> Arc<Request> {
    // 多次生命周期测试使用小窗口减少无关工作。
    let mut input = request(); // 标准作者格距和地图身份。
    input.lods = vec![
        MapCollisionLod {
            max_distance_meters: 2.0,
            spacing_multiplier: 1,
        },
        MapCollisionLod {
            max_distance_meters: 8.0,
            spacing_multiplier: 4,
        },
    ]; // 包含真实粗细接缝。
    Arc::new(input) // 每轮后台共享输入。
} // 不构造整图高度。

fn warm_layout(
    pool: &TaskPool,
    input: &Arc<Request>,
    previous: Option<(Arc<Request>, Arc<Layout>)>,
) -> Layout {
    // 始终调用生产缓存路径。
    let layout = block_on(build::prepare_layout_parallel(
        Arc::clone(input),
        previous,
        pool,
        2,
    ))
    .unwrap(); // 测试线程等待，不在帧路径。
    assert!(layout.planning_cache.is_some()); // 确认覆盖真正的跨刷新快照，而非旧冷入口。
    let cold = build::prepare_layout(input).unwrap(); // 独立无缓存完整规划作为权威对照。
    assert_eq!(layout.tiles, cold.tiles); // 源页集合一致。
    assert_eq!(layout.supported_rows, cold.supported_rows); // 地图边缘及完整格覆盖一致。
    assert_eq!(layout.chunks.len(), cold.chunks.len()); // 没有遗漏或残留管理块。
    for (key, chunk) in &layout.chunks {
        // 不能仅凭摘要宣布等价。
        let expected = &cold.chunks[key]; // 完整目标键必须存在。
        assert_eq!(chunk.plan.cells, expected.plan.cells); // 实际单元、接缝逐项一致。
        assert_eq!(chunk.geometry.vertices, expected.geometry.vertices); // 真实零高网格一致。
        assert_eq!(chunk.geometry.triangles, expected.geometry.triangles); // 没有修改三角剖分。
        assert_eq!(
            chunk.geometry.base_grid_points,
            expected.geometry.base_grid_points
        ); // 高度采样位置一致。
    } // 耗时不参与等价判断。
    layout // 返回真实可烘焙候选。
} // 不访问测试专用缓存内部结构。

#[test] // 移动往返和身份改变都必须等价于从头规划。
fn retained_planning_layout_matches_cold_after_movement_and_identity_changes() {
    // 防止缓存留下旧近场细化。
    let pool = TaskPoolBuilder::new().num_threads(3).build(); // 有界专用池。
    let initial = small_request(); // 真实两层配置。
    let mut previous_input = Arc::clone(&initial); // 已采用请求模拟。
    let mut previous = Arc::new(warm_layout(&pool, &initial, None)); // 首载生成真实快照。
    for offset in [0.25, 1.0, 3.0, 0.0, -1.25] {
        // 穿格、走远、掉头和返回原点。
        let mut next = (*initial).clone(); // 内容不变，仅改变真实玩家位置。
        next.anchor.x += offset; // 无预测中心或预取。
        let next = Arc::new(next); // 完整冻结身份。
        let layout = warm_layout(&pool, &next, Some((previous_input, previous))); // 每次以上一次结果继续，而非永远使用首载。
        previous_input = next; // 模拟成功采用新请求。
        previous = Arc::new(layout); // 只保留最新完整布局。
    } // 旧精度不能不断累积。
    for field in 0..4 {
        // 分别改变所有缓存关键内容。
        let mut next = (*previous_input).clone(); // 独立单字段变化。
        match field {
            // 几何未变也不能跨地图或根共享实体关联布局。
            0 => next.map_id = MapId::parse("MAP-planning-cache-other").unwrap(), // 新地图。
            1 => next.root = Entity::from_bits(19),                               // 新根代次。
            2 => {
                next.spec =
                    SparseHeightSpec::new([0.0; 2], [513.0; 2], 0.125, -128.0, 256.0).unwrap()
            } // 不同尾格规格。
            _ => next.lods[0].max_distance_meters = 3.0, // 同图保存新距离。
        } // 配置不得被旧规划吞掉。
        let next = Arc::new(next); // 冻结新身份。
        let layout = warm_layout(
            &pool,
            &next,
            Some((Arc::clone(&previous_input), Arc::clone(&previous))),
        ); // 生产层执行失效或重算。
        if field < 3 {
            assert_eq!(layout.reused_blocks, 0);
        } // 跨地图/根/规格禁止共享旧拓扑。
    } // 每项同时逐格对照冷规划。
} // 无窗口回归不宣称真实游戏跟随延迟。

#[test] // 高度修订可复用空间规划，但必须继续烘焙实际脏页。
fn retained_planning_cache_does_not_skip_changed_height_cooking() {
    // 缓存身份不能取代源身份。
    let pool = TaskPoolBuilder::new().num_threads(3).build(); // 独立有界池。
    let initial = small_request(); // 玩家位于作者页交点。
    let previous = Arc::new(warm_layout(&pool, &initial, None)); // 真正含规划快照。
    let before = blank_stamps(&previous); // 旧版显式平地身份。
    let mut input = (*initial).clone(); // 同位置的新高度内容。
    input.live_revision = Some(2); // 模拟雕刻，不移动中心。
    let input = Arc::new(input); // 完整内容身份变化。
    let layout = warm_layout(&pool, &input, Some((initial, Arc::clone(&previous)))); // 允许复用纯空间结果。
    assert_eq!(layout.rebuilt_blocks, 0); // 零高拓扑不应被高度修订重建。
    let key = SparseHeightTileKey { x: 4, z: 4 }; // 玩家脚下唯一作者页。
    let mut tile = SparseHeightTile::blank(&input.spec, key).unwrap(); // 只分配局部源。
    tile.set([0, 0], input.spec.encode_height(3.0).unwrap())
        .unwrap(); // 真实非平顶点。
    let source = Arc::new(tile); // 不可变新快照。
    let mut stamps = blank_stamps(&layout); // 当前完整块源依赖。
    for stamp in stamps.values_mut() {
        // 只令接触此页的块变脏。
        for entry in &mut stamp.sources {
            if entry.0 == key {
                entry.2 = Arc::as_ptr(&source) as usize;
            }
        } // 与运行时相同的局部Arc身份。
    } // 其余源保持不变。
    let reusable: BTreeSet<_> = stamps
        .iter()
        .filter(|(key, stamp)| before.get(key) == Some(*stamp))
        .map(|(key, _)| *key)
        .collect(); // 实际源相等才复用物理形状。
    let dirty = layout.chunks.len() - reusable.len(); // 应重烘焙的真实块数。
    assert!(dirty > 0 && dirty < layout.chunks.len()); // 同时覆盖脏块和保留块。
    let cooked = build::cook(
        &input,
        layout,
        stamps,
        reusable,
        BTreeMap::from([(key, source)]),
        BTreeMap::new(),
    )
    .unwrap(); // 真实Avian构造。
    assert_eq!(cooked.shapes.len(), dirty); // 规划缓存不能让脏形状被短路。
    assert!(cooked.shapes.iter().any(|(_, _, shape)| {
        shape
            .shape()
            .as_trimesh()
            .unwrap()
            .vertices()
            .iter()
            .any(|point| point.y > 2.0)
    })); // 实际物理顶点含新高度。
    assert!(cooked.layout.as_ref().unwrap().planning_cache.is_some()); // 快照随完整形状候选传递。
} // 不冒充已经执行物理接触验证。

#[test] // 真实含缓存候选只有成功发布才成为下一轮基线。
fn planning_cache_survives_failed_publication_and_is_cleared_on_reset() {
    // 检查所有权生命周期而非空夹具。
    let pool = TaskPoolBuilder::new().num_threads(3).build(); // 独立有界后台池。
    let mut world = World::new(); // 无窗口ECS世界。
    let mut input = (*small_request()).clone(); // 小型真实几何。
    input.root = world.spawn_empty().id(); // 父节点必须实际存在。
    let input = Arc::new(input); // 完整请求。
    let layout = warm_layout(&pool, &input, None); // 首次完整规划。
    let cooked = build::cook(
        &input,
        layout,
        BTreeMap::new(),
        BTreeSet::new(),
        BTreeMap::new(),
        BTreeMap::new(),
    ); // 缺签名必须真实失败。
    assert!(cooked.is_err()); // 半成品没有可采用快照。
    let layout = warm_layout(&pool, &input, None); // 重新取得完整合法候选。
    let stamps = blank_stamps(&layout); // 补齐真实源身份。
    let cooked = build::cook(
        &input,
        layout,
        stamps,
        BTreeSet::new(),
        BTreeMap::new(),
        BTreeMap::new(),
    )
    .unwrap(); // 正式形状集合。
    let expected = Arc::clone(cooked.layout.as_ref().unwrap()); // 追踪完整布局身份。
    let mut state = LayeredCollisionRuntime {
        map_id: Some(input.map_id.clone()),
        root: Some(input.root),
        desired: Some(Arc::clone(&input)),
        ticket: 7,
        ..default()
    }; // 生产发布所需身份。
    let mut region = TerrainCollisionRegion {
        center: input.anchor,
        ..default()
    }; // 首载没有旧承托。
    let mut feedback = MapCollisionLodStatus::default(); // 独立诊断。
    let mut runtime = DynamicMapRuntimeState::default(); // 地图就绪门。
    let mut queue = CommandQueue::default(); // 同一次发布原子提交。
    publication::adopt(
        &mut Commands::new(&mut queue, &world),
        &mut state,
        &mut region,
        &mut feedback,
        &mut runtime,
        cooked,
    )
    .unwrap(); // 真实成功发布。
    queue.apply(&mut world); // 安装完整形状集合。
    assert!(Arc::ptr_eq(
        state.applied_layout.as_ref().unwrap(),
        &expected
    )); // 成功才持有此快照。
    let mut changed = (*input).clone(); // 后续请求需要自己的候选。
    changed.anchor.x += 0.5; // 移动刷新。
    let changed = Arc::new(changed); // 冻结新请求。
    let layout = warm_layout(
        &pool,
        &changed,
        Some((Arc::clone(&input), Arc::clone(&expected))),
    ); // 从已采用快照准备。
    let stamps = blank_stamps(&layout); // 完整身份。
    let mut bad = build::cook(
        &changed,
        layout,
        stamps,
        BTreeSet::new(),
        BTreeMap::new(),
        BTreeMap::new(),
    )
    .unwrap(); // 真正拥有新规划的候选。
    bad.bounds.pop(); // 模拟发布前发现集合不完整。
    state.desired = Some(changed); // 当前允许尝试此候选。
    assert!(
        publication::adopt(
            &mut Commands::new(&mut queue, &world),
            &mut state,
            &mut region,
            &mut feedback,
            &mut runtime,
            bad
        )
        .is_err()
    ); // 预检在ECS写入前拒收。
    lifecycle::fail(
        &mut state,
        &mut feedback,
        &mut DynamicMapRuntimeStatus::default(),
        "候选失败测试".into(),
    ); // 真实失败处理保留上版。
    assert!(Arc::ptr_eq(
        state.applied_layout.as_ref().unwrap(),
        &expected
    )); // 失败规划不能污染已采用缓存。
    assert_eq!(state.applied.as_deref(), Some(input.as_ref())); // 已采用请求仍是旧完整版本。
    lifecycle::reset(&mut Commands::new(&mut queue, &world), &mut state); // 换图正式清理入口。
    queue.apply(&mut world); // 旧地面一同清理。
    assert!(state.applied_layout.is_none() && state.applied.is_none()); // 规划缓存随根代次退出。
    assert_eq!(state.ticket, 8); // 迟到候选失去发布权。
} // 保留外部expected只为断言，不模拟运行时历史缓存。
