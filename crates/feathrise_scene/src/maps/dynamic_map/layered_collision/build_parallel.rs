//! 碰撞批次内部有界并行；规划及完整集合归并仍由唯一后台协调器拥有。
use super::{build, parallel, planning, resources::*, types::*}; // 共用串行构造函数及同一几何合同。
use bevy::{prelude::Vec2, tasks::TaskPool}; // 不创建独立线程池，不接触ECS。
use feathrise_map_content::sparse_height::{SparseCraterTiles, SparseHeightTile, SparseHeightTileKey}; // 只读高度源。
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Instant,
}; // 快照共享及墙钟诊断。

pub(super) async fn prepare_layout_parallel(
    // 调用者必须位于后台CPU任务，主帧仅轮询父任务。
    request: Arc<Request>,                         // 完整冻结需求。
    previous: Option<(Arc<Request>, Arc<Layout>)>, // 只允许已经采用的缓存。
    pool: &TaskPool,                               // 复用项目计算池。
    worker_limit: usize,                           // 调用者上限还会由通用worker策略收紧。
) -> Result<Layout, String> {
    // 任何子任务失败都不交付半个布局。
    let start = Instant::now(); // 包含规划、排队及所有worker完成时间。
    let previous = previous.filter(|(old, _)| {
        old.map_id == request.map_id && old.root == request.root && old.spec == request.spec
    }); // 跨图/根/规格绝不复用。
    let (plan, planning_cache) = planning::plan_cached(
        &request.spec,
        &request.lods,
        request.anchor,
        previous
            .as_ref()
            .and_then(|(_, layout)| layout.planning_cache.as_deref()),
    )?; // 仅已采用的同图/根/规格缓存可进入；配置由规划器再次校验。
    let plan = build::plan_with_holes(&request.spec, plan, &request.holes);
    let mut chunks = BTreeMap::new(); // 已命中的共享拓扑。
    let mut changes = Vec::new(); // 只把真实变化块交给worker。
    let mut vertices = 0_usize; // 生成之前核算完整候选最坏顶点数。
    let mut triangles = 0_usize; // 生成之前核算完整候选最坏面数。
    let mut bounded = true; // 不能证明并行峰值时回退原逐块精确路径。
    for (key, chunk) in &plan.chunks {
        // 至多现有固定块上限，不产生逐格大数组副本。
        if let Some(old) = previous
            .as_ref()
            .and_then(|(_, layout)| layout.chunks.get(key))
            .filter(|old| old.plan == *chunk)
        {
            // 接缝摘要参与命中。
            vertices = vertices.saturating_add(old.geometry.vertices.len()); // 共享块也计入完整布局预算。
            triangles = triangles.saturating_add(old.geometry.triangles.len()); // 共享不意味免预算。
            chunks.insert(*key, Arc::clone(old)); // 不深拷贝已有网格或依赖。
        } else {
            // 新进入、精度变化或接缝变化才生成。
            if let Some(cells) = &chunk.cells {
                // 正式规划器提供完整单元列表。
                for cell in cells.iter() {
                    // 普通格最多四顶点，过渡格最多八边界点加中心。
                    let seam = cell.finer_edges & 15 != 0; // 尾格可能不插点，按最坏模板计算仍安全。
                    vertices = vertices.saturating_add(if seam { 9 } else { 4 }); // 不依赖跨单元顶点去重才能成立的上界。
                    triangles = triangles.saturating_add(if seam { 8 } else { 2 }); // 不改变实际三角化。
                } // 相邻格共享顶点只会降低实际占用。
            } else {
                bounded = false;
            } // 兼容旧夹具完整块模板，交原精确预算路径处理。
            changes.push((*key, chunk.clone())); // 单元向量为Arc，只有小型身份副本。
        } // 全部变化块统一派发。
    } // 尚未启动任何worker或分配任何新网格。
    if !bounded || vertices > 1_048_576 || triangles > 2_097_152 || !request.holes.is_empty() || request.stitches.active() {
        // 洞口会增加边界顶点，走后台精确预算路径，避免普通格上界漏计裁切输出。
        // 保守上界超预算不等于实际超预算。
        let mut layout = build::prepare_layout_from_plan(
            &request,
            previous
                .as_ref()
                .map(|(old, layout)| (old.as_ref(), layout.as_ref())),
            plan,
            start,
        )?; // 复用已算好的计划走原精确预算，不重复整窗口规划、不新增误拒。
        layout.planning_cache = Some(planning_cache); // 回退仅改变几何构建策略，不丢失完整规划快照。
        return Ok(layout); // 失败路径不会留下可复用半成品。
    } // 只有完整最坏几何量已获准后才允许多worker分配。
    let reused_blocks = chunks.len(); // 真实缓存命中数。
    let build_request = Arc::clone(&request); // 所有worker共享规格，不复制高度或地图。
    let completed = parallel::map(pool, changes, worker_limit, move |(key, plan)| {
        // helper保证全部worker实际退出后才返回。
        Ok((key, build::layout_chunk(&build_request.spec, plan,&build_request.stitches)?))
    })
    .await?; // 只等待后台协调器，不阻塞游戏主帧。
    let mut rebuilt_blocks = 0; // 空块不计为有效拓扑重建。
    for (key, chunk) in completed {
        // 返回顺序稳定，归并无共享写锁。
        if let Some(chunk) = chunk {
            // 与原串行路径相同，丢弃空几何。
            chunks.insert(key, chunk); // 唯一协调器写目标集合。
            rebuilt_blocks += 1; // 精确计量实际生成块。
        } // 完成一块不等于允许发布一块。
    } // 所有拓扑已成功。
    let tiles = chunks
        .values()
        .flat_map(|chunk| chunk.tiles.iter().copied())
        .collect(); // 完整依赖集合保持BTree稳定去重。
    Ok(Layout {
        // 单一完整布局进入后续IO阶段。
        planning_cache: Some(planning_cache), // 直到整批形状成功采用才成为下一请求的规划基线。
        chunks,                               // 已复用与新建块的全集。
        tiles,                                // 全候选真实依赖。
        supported_polygon: plan.outer_boundary, // 原规划几何覆盖不变。
        supported_rows: plan.supported_rows,  // 同版承托覆盖。
        elapsed_ms: start.elapsed().as_secs_f32() * 1000.0, // 包含规划和归并。
        rebuilt_blocks,                       // 本批真实重建。
        reused_blocks,                        // 本批真实复用。
    }) // 未经过发布身份校验前没有物理副作用。
} // 不改变范围、精度或预取行为。

pub(super) async fn cook_parallel(
    // 采样及Avian形状在有限worker中完成。
    request: Arc<Request>,                       // 冻结需求身份。
    layout: Layout,                              // 完整并且已通过几何预算的候选。
    mut stamps: BTreeMap<ChunkKey, SourceStamp>, // 每块源身份可移动给唯一构造任务。
    reusable: BTreeSet<ChunkKey>,                // 复用形状不重复采样。
    sources: BTreeMap<SparseHeightTileKey, Arc<SparseHeightTile>>, // 全批共享高度页映射。
    craters: SparseCraterTiles,                  // 全批共享会话破坏。
    pool: &TaskPool,                             // 项目后台计算池。
    worker_limit: usize,                         // 并行上限而非强制线程数。
) -> Result<Cooked, String> {
    // 失败保留旧完整物理版本。
    let start = Instant::now(); // 记录本阶段真实墙钟。
    let mut plans = BTreeMap::new(); // 包含保留块的完整目标集合。
    let mut bounds = Vec::new(); // 活动门完整覆盖。
    let mut changes = Vec::new(); // 只包含实际待cooking块。
    for (key, chunk) in &layout.chunks {
        // 在启动任何worker前校验所有边界与源身份。
        let (lo, hi) = planning::chunk_bounds(&request.spec, *key); // 原固定地图边界。
        let a = request
            .spec
            .grid_point_world(lo[0], lo[1])
            .ok_or("碰撞下界无效")?; // 不允许坏元数据进入worker。
        let b = request
            .spec
            .grid_point_world(hi[0], hi[1])
            .ok_or("碰撞上界无效")?; // 与串行构造同一坐标。
        bounds.push((
            Vec2::new(a[0] as f32, a[1] as f32),
            Vec2::new(b[0] as f32, b[1] as f32),
        )); // 整批完整覆盖。
        plans.insert(*key, chunk.plan.clone()); // 计划大数组保持Arc共享。
        if !reusable.contains(key) {
            // 仅变脏块需要高度输出与新Avian结构。
            changes.push((
                Arc::clone(chunk),
                stamps.remove(key).ok_or("碰撞块缺少源身份")?,
            )); // stamp移动，不给每个worker复制整份映射。
        } // 保留块仍在目标plans内。
    } // 只有完整元数据通过后启动有限worker。
    let sources = Arc::new(sources); // 所有worker共享唯一映射，不按线程复制整图。
    let craters = Arc::new(craters); // 会话高度同样只共享快照。
    let worker_sources = Arc::clone(&sources); // 闭包持有所有worker共同的引用。
    let worker_craters = Arc::clone(&craters); // helper返回时必须归还闭包所有权。
    let shapes = parallel::map(pool, changes, worker_limit, move |(chunk, stamp)| {
        // 输出保持输入稳定键顺序。
        let collider = build::cook_chunk(&request.spec, &chunk, &worker_sources, &worker_craters,&request.stitches)?;
        Ok((chunk.plan.clone(), stamp, collider)) // 完整块只放入后台结果。
    })
    .await?; // 全部结束才返回，包括其他worker失败的情况。
    let sources = Arc::try_unwrap(sources).map_err(|_| "碰撞worker未归还高度快照".to_string())?; // 取回唯一映射，不深拷贝兜底。
    let craters = Arc::try_unwrap(craters).map_err(|_| "碰撞worker未归还浅坑快照".to_string())?; // 严格收回所有子任务占有。
    Ok(Cooked {
        // 主世界继续执行原完整替换与身份校验。
        shapes,                                              // 仅变脏块的新物理形状。
        plans,                                               // 保留块与变脏块全集。
        sources,                                             // 原独占源映射。
        craters,                                             // 原独占会话映射。
        bounds,                                              // 完整覆盖边界。
        supported_polygon: layout.supported_polygon.clone(), // 兼容旧合同。
        supported_rows: layout.supported_rows.clone(),       // 同版覆盖Arc。
        elapsed_ms: start.elapsed().as_secs_f32() * 1000.0,  // 包含等待worker与归并。
        layout: Some(Arc::new(layout)),                      // 只有完整采用后才成为下次缓存。
    }) // 没有逐块提前发布路径。
} // 旧候选换代仍由父票据拒绝，实际CPU占槽直到本函数结束。
