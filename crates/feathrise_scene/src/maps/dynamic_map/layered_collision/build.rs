//! 后台窗口布局与局部采样烘焙；所有规模相关循环均不运行在主帧。
pub(super) use super::build_parallel::{cook_parallel, prepare_layout_parallel};
use super::{geometry, planning, resources::*, types::*};
use avian3d::prelude::{Collider, TrimeshFlags}; // 后台构造完整三角形状。
use bevy::prelude::*; // 世界坐标。
use feathrise_map_content::SparseHeightSpec; // 作者格点与后台采样合同。
use feathrise_map_content::sparse_height::{
    SparseCraterTiles, SparseHeightTile, SparseHeightTileKey, sample_crater_height,
}; // 同源采样。
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Instant,
}; // 有界集合与构建测量。 // 单一规划、接缝与完成值合同。 // 运行时使用有界并行，测试保留同算法串行对照。

pub(super) fn select_request_anchor(
    current: &Request,
    previous: Option<&Request>,
    center: Vec2,
    preparing: bool,
    last_player_center: Option<Vec2>, // 上帧真实位置只用于识别跨覆盖跳转，不以累计走路距离取消候选。
) -> Vec2 {
    // 普通移动合并最新位置，但不取消同内容在途候选；覆盖大小和格距均不变。
    let multiplier = current.lods.first().map_or(1, |lod| lod.spacing_multiplier) as f32; // 调用前已经完成配置校验。
    let step = Vec2::new(
        current.spec.actual_spacing_meters[0] as f32,
        current.spec.actual_spacing_meters[1] as f32,
    ) * ((1_u64 << planning::chunk_cells_log2(&current.spec, &current.lods)) as f32
        * multiplier); // 使用实际自适应固定块跨度。
    if let Some(old) = previous.filter(|old| {
        old.map_id == current.map_id
            && old.root == current.root
            && old.spec == current.spec
            && old.manifest_revision == current.manifest_revision
            && old.live_revision == current.live_revision
            && old.crater_revision == current.crater_revision
            && old.holes == current.holes
            && old.lods == current.lods
    }) {
        // 不放宽地图、根、地形、配置和覆盖修订合同。
        let delta = center - old.anchor; // 相对候选真正冻结的中心，不相对不断移动的相机。
        let trigger = (step.min_element() * 0.25).min(current.lods[0].max_distance_meters * 0.25); // 在细块与近场范围内提前更新。
        let jumped_outside = last_player_center.is_some_and(|last| {
            // 普通累计移动不等于传送；只检查本帧跨越整个配置覆盖的跳变。
            let radius = current
                .lods
                .last()
                .expect("已校验非空LOD")
                .max_distance_meters; // 不增加预取半径。
            center.distance_squared(last) > radius * radius // 覆盖外跳转直接准备落点，旧任务仍占槽直到完成。
        }); // 覆盖内传送沿用当前完整候选，随后追赶；缺少实际承托仍由活动门保护。
        if !jumped_outside && (preparing || delta.length() <= trigger) {
            return old.anchor;
        } // 同内容在途任务必须获得完成机会，不能由LOD0半径决定有效期限。
    } // 跨覆盖跳转或内容变化立即换代；普通移动等待当前批次完成再追赶。
    center // 新候选围绕真实玩家落点，固定作者格点只约束采样，不移动圆心。
} // 不扩大任务并发、不接纳旧内容、不改变显示焦点。

pub(super) fn sample_corners(spec: &SparseHeightSpec, grid: [f64; 2]) -> [[u64; 2]; 4] {
    // 原始格点的唯一所有者。
    let a = [
        grid[0].floor().clamp(0.0, spec.cells[0] as f64) as u64,
        grid[1].floor().clamp(0.0, spec.cells[1] as f64) as u64,
    ]; // 下侧格点。
    let b = [(a[0] + 1).min(spec.cells[0]), (a[1] + 1).min(spec.cells[1])]; // 有限尾格裁剪。
    [a, [b[0], a[1]], [a[0], b[1]], b] // a,b,c,d与作者三角剖分一致。
} // 每个顶点最多四个源点。

#[cfg(test)] // 冷构造对照入口；运行时首载通过cached入口传None走相同算法。
pub(super) fn prepare_layout(request: &Request) -> Result<Layout, String> {
    prepare_layout_cached(request, None) // 测试显式禁用复用，核对完整几何与依赖。
} // 运行时移动优先传入上一完整采用版本。

#[cfg(test)] // 串行权威对照；生产串行回退直接消费已经生成的计划。
pub(super) fn prepare_layout_cached(
    request: &Request,                     // 当前冻结身份。
    previous: Option<(&Request, &Layout)>, // 不使用失败或尚未采用的候选缓存。
) -> Result<Layout, String> {
    let start = Instant::now(); // 包含规划、几何以及页依赖生成，不冒充物理步耗时。
    let previous = previous.filter(|(old, _)| {
        old.map_id == request.map_id && old.root == request.root && old.spec == request.spec
    }); // 高度修订不改变零高度拓扑，跨图/根/规格则绝不复用。
    let plan = planning::plan(
        &request.spec,
        &request.lods, // 唯一距离权威；删除预取及额外覆盖路径。
        request.anchor,
    )?; // 含接缝的精确512块预算。
    let plan = plan_with_holes(&request.spec, plan, &request.holes);
    prepare_layout_from_plan(request, previous, plan, start) // 并行预算回退也复用同一已计算计划，不能重新扫描整个窗口。
} // 冷构造与串行对照只规划一次。

pub(super) fn prepare_layout_from_plan(
    // 已规划窗口的串行精确预算构造器。
    request: &Request,                     // 已冻结且已验证的规格。
    previous: Option<(&Request, &Layout)>, // 调用者已验证地图、根和规格身份。
    plan: Plan,                            // 消费已有完整计划，不重新规划。
    start: Instant,                        // 从原批次开始计时，包含预检和回退成本。
) -> Result<Layout, String> {
    // 保留原有逐块几何预算与错误文本。
    let mut chunks = BTreeMap::new(); // 稳定块顺序。
    let mut tiles = BTreeSet::new(); // 所有源页去重。
    let mut vertex_count = 0_usize; // 圆边节点不能绕过整批几何内存上限。
    let mut triangle_count = 0_usize; // 除512块之外再限制实际三角面总量。
    let mut reused_blocks = 0_usize; // 计量真正跳过网格和页依赖生成的块。
    let mut rebuilt_blocks = 0_usize; // 不是物理形状重建数量。
    for (key, plan) in plan.chunks {
        if let Some(old) = previous
            .and_then(|(_, layout)| layout.chunks.get(&key))
            .filter(|old| old.plan == plan)
        {
            // 接缝摘要也是复用身份，不能只比较世界块键。
            vertex_count += old.geometry.vertices.len(); // 复用也计入完整候选几何预算。
            triangle_count += old.geometry.triangles.len(); // 防止缓存绕过资源上限。
            if vertex_count > 1_048_576 || triangle_count > 2_097_152 {
                // 与冷构造一致。
                return Err("圆形碰撞整批几何超过顶点/三角面预算，请调整距离或倍率".into()); // 失败保留旧版。
            } // 不降精度。
            tiles.extend(old.tiles.iter().copied()); // 只合并已缓存依赖键，不重新逐顶点求页。
            chunks.insert(key, Arc::clone(old)); // 大数组共享，主线程和后台均不复制未变块。
            reused_blocks += 1; // 记录真实命中。
            continue; // 核心优化：geometry::build_chunk之前短路。
        } // 仅进入、精度或接缝变化的块继续生成。
        // 每块拓扑只在后台生成。
        let Some(chunk) = layout_chunk_with_holes(&request.spec, plan, &request.holes,&request.stitches)? else {
            continue;
        }; // 串行与并行共用几何及依赖生成，避免两套接缝规则。
        let geometry = &chunk.geometry; // 本轮新增的不可变零高度拓扑。
        vertex_count += geometry.vertices.len(); // 每块自身有独立硬上限，累计无溢出风险。
        triangle_count += geometry.triangles.len(); // 空间细分不能隐式无限增加候选量。
        if vertex_count > 1_048_576 || triangle_count > 2_097_152 {
            return Err("圆形碰撞整批几何超过顶点/三角面预算，请调整距离或倍率".into());
        } // 失败保留上一有效版本，不暗中降精度。
        tiles.extend(chunk.tiles.iter().copied()); // 依赖位置不等于实际驻留页；空白页由加载阶段直接跳过。
        chunks.insert(key, chunk); // 保存几何与实际依赖。
        rebuilt_blocks += 1; // 页依赖也在这次生成。
    } // 全部布局成功。
    Ok(Layout {
        planning_cache: None, // 冷串行参考不启用跨次缓存；生产预算回退由调用者补回同一已算快照。
        chunks,
        tiles,
        supported_polygon: plan.outer_boundary,
        supported_rows: plan.supported_rows, // 完整格覆盖和几何一起进入候选。
        elapsed_ms: start.elapsed().as_secs_f32() * 1000.0, // 整个后台第一阶段。
        rebuilt_blocks,                      // 真实布局重建。
        reused_blocks,                       // 真实布局共享。
    }) // 等待IO的唯一布局，边界与网格同版。
} // 全平图也通过相同有限分块路径。

pub(super) fn layout_chunk(
    spec: &SparseHeightSpec,
    plan: ChunkPlan,
    stitches:&feathrise_environment::vegetation::blade::GpuBladeClipmapStitches,
) -> Result<Option<Arc<LayoutChunk>>, String> {
    layout_chunk_with_holes(spec, plan, &[],stitches)
}

pub(super) fn plan_with_holes(
    spec: &SparseHeightSpec,
    mut plan: Plan,
    holes: &[feathrise_map_content::MapTerrainCutout],
) -> Plan {
    if holes.is_empty() {
        return plan;
    }
    for (key, chunk) in &mut plan.chunks {
        let (lo, hi) = planning::chunk_bounds(spec, *key);
        let a = spec.grid_point_world(lo[0], lo[1]).expect("受检分块下界");
        let b = spec.grid_point_world(hi[0], hi[1]).expect("受检分块上界");
        let mut bytes = Vec::with_capacity(holes.len() * 16);
        for hole in holes.iter().filter(|hole| {
            hole.intersects(
                Vec2::new(a[0] as f32, a[1] as f32),
                Vec2::new(b[0] as f32, b[1] as f32),
            )
        }) {
            for value in hole.min.into_iter().chain(hole.max) {
                bytes.extend(value.to_le_bytes());
            }
        }
        if bytes.is_empty() {
            continue;
        } // 洞口只污染实际相交块，不让远处地形全部重烘焙。
        let mut signature = chunk.signature.as_bytes().to_vec();
        signature.extend(&bytes);
        chunk.signature =
            feathrise_map_content::content_hash(signature.as_slice()).expect("内存切片读取不会失败"); // 后台摘要使旧封口 Collider 不能通过局部高度缓存复用。
    }
    plan
}

pub(super) fn layout_chunk_with_holes(
    spec: &SparseHeightSpec,
    plan: ChunkPlan,
    holes: &[feathrise_map_content::MapTerrainCutout],
    stitches:&feathrise_environment::vegetation::blade::GpuBladeClipmapStitches,
) -> Result<Option<Arc<LayoutChunk>>, String> {
    // 单块独立任务，不借用其他块输出。
    let mut geometry = geometry::build_chunk(spec, &plan, |_| Some(0.0))?; // 高度加载后才烘焙。
    geometry::cut_holes(spec, &mut geometry, holes)?;
    super::stitches::refine(spec,&mut geometry,stitches)?;
    if geometry.triangles.is_empty() {
        return Ok(None);
    } // 不生成空Collider或伪承托块。
    let mut required = BTreeSet::new(); // 本块局部源签名。
    for grid in &geometry.base_grid_points {
        // 不扫描未被采样的中间区域。
        for point in sample_corners(spec, *grid) {
            // 也覆盖有限尾格扇形中心。
            let (tile, _) = spec
                .tile_for_grid_point(point[0], point[1])
                .ok_or("碰撞顶点越过地图")?; // 唯一归属。
            required.insert(tile); // 跨作者块边界保留双方。
        } // 当前顶点全部源点。
    } // 精确依赖完整就绪。
    Ok(Some(Arc::new(LayoutChunk {
        plan,
        geometry,
        tiles: required,
    }))) // 不可变零高度网格由后续候选共享。
} // 串行对照及worker共同使用。

pub(super) fn sample_height(
    spec: &SparseHeightSpec,
    tiles: &BTreeMap<SparseHeightTileKey, Arc<SparseHeightTile>>,
    craters: &SparseCraterTiles,
    grid: [f64; 2],
) -> Result<f32, String> {
    // 同源原始三角面采样。
    let points = sample_corners(spec, grid); // 四个源格点有唯一权威。
    let mut heights = [0.0; 4]; // 固定栈空间。
    for (index, point) in points.into_iter().enumerate() {
        // 每点合成作者与会话破坏。
        let (key, local) = spec
            .tile_for_grid_point(point[0], point[1])
            .ok_or("碰撞采样超出地图")?; // 受检归属。
        let code = match tiles.get(&key) {
            Some(tile) => tile.get_height(spec, local).ok_or("碰撞高度页缺少格点")?,
            None => spec.flat_height_code(),
        }; // 非平页缺失在调度层阻止派发。
        heights[index] = sample_crater_height(spec, craters, point, spec.decode_height(code)); // 相同边界永远采同一源。
    } // 三角内线性插值不使用会改变鞍面高度的双线性。
    let x = (grid[0] - points[0][0] as f64) as f32; // 有限尾格内横坐标。
    let z = (grid[1] - points[0][1] as f64) as f32; // 有限尾格内纵坐标。
    Ok(if x + z <= 1.0 {
        heights[0] + x * (heights[1] - heights[0]) + z * (heights[2] - heights[0])
    } else {
        heights[3] + (1.0 - z) * (heights[1] - heights[3]) + (1.0 - x) * (heights[2] - heights[3])
    }) // 共享作者对角线。
} // 不修改显示精度。

#[cfg(test)] // 保留逐块串行基准；正式运行统一走同构造器的有界并行入口。
pub(super) fn cook(
    request: &Request,
    layout: Layout,
    stamps: BTreeMap<ChunkKey, SourceStamp>,
    reusable: BTreeSet<ChunkKey>,
    sources: BTreeMap<SparseHeightTileKey, Arc<SparseHeightTile>>,
    craters: SparseCraterTiles,
) -> Result<Cooked, String> {
    // CPU任务阶段二。
    let start = Instant::now(); // 墙钟仅在构建时采集。
    let mut shapes = Vec::new(); // 仅缺失或变脏块产生新形状。
    let mut plans = BTreeMap::new(); // 全部目标用于完整替换校验。
    let mut bounds = Vec::new(); // 活动门只读取已采用几何覆盖。
    for (key, chunk) in &layout.chunks {
        // 最大512叶。
        let (lo, hi) = planning::chunk_bounds(&request.spec, *key); // 地图内固定边界。
        let a = request
            .spec
            .grid_point_world(lo[0], lo[1])
            .ok_or("碰撞下界无效")?; // 格点换算。
        let b = request
            .spec
            .grid_point_world(hi[0], hi[1])
            .ok_or("碰撞上界无效")?; // 格点换算。
        bounds.push((
            Vec2::new(a[0] as f32, a[1] as f32),
            Vec2::new(b[0] as f32, b[1] as f32),
        )); // 无重叠叶覆盖。
        plans.insert(*key, chunk.plan.clone()); // 保留块也进入完整集合，圆形裁切身份共享不可变数据。
        if reusable.contains(key) {
            continue;
        } // 未变块不重采样、不重新cooking。
        let collider = cook_chunk(&request.spec, chunk, &sources, &craters,&request.stitches)?;
        shapes.push((
            chunk.plan.clone(), // 布局缓存继续持有相同身份。
            stamps.get(key).ok_or("碰撞块缺少源身份")?.clone(),
            collider,
        )); // 绑定局部源身份。
    } // 任一块失败则整个候选不发布。
    Ok(Cooked {
        shapes,
        plans,
        sources,
        craters,
        bounds,
        supported_polygon: layout.supported_polygon.clone(), // 小型兼容轮廓，新规则格路径为空。
        supported_rows: layout.supported_rows.clone(),       // 共享同版真实覆盖。
        elapsed_ms: start.elapsed().as_secs_f32() * 1000.0,
        layout: Some(Arc::new(layout)), // 成功返回仍不是发布，只有adopt才能成为缓存基线。
    }) // 主线程仅采用完整结果。
} // 不借用World或Assets。

pub(super) fn cook_chunk(
    // 单块采样与形状构造仅写独占输出。
    spec: &SparseHeightSpec, // 当前冻结格点合同。
    chunk: &LayoutChunk,     // 不可变缓存拓扑。
    sources: &BTreeMap<SparseHeightTileKey, Arc<SparseHeightTile>>, // 全worker共享源快照。
    craters: &SparseCraterTiles, // 全worker共享浅坑快照。
    stitches:&feathrise_environment::vegetation::blade::GpuBladeClipmapStitches,
) -> Result<Collider, String> {
    // 不发布部分物理结果。
    let mut vertices = chunk.geometry.vertices.clone(); // 只有真正变脏形状才复制待写高度，缓存始终保持零高度。
    for (vertex, grid) in vertices.iter_mut().zip(&chunk.geometry.base_grid_points) {
        // 全部高度在后台填充。
        let min=Vec2::from_array(spec.world_min);let max=Vec2::from_array(spec.world_max);
        let point=Vec2::new(vertex.x,vertex.z);
        let on_edge=stitches.edges.iter().enumerate().any(|(e,info)|info.x>=2.0 &&
            (point[if e<2 {0}else{1}]-if e%2==0 {min[if e<2 {0}else{1}]}else{max[if e<2 {0}else{1}]}).abs()<0.00001);
        let base=if on_edge {0.0}else{sample_height(spec,sources,craters,*grid)?};
        vertex.y=stitches.height(point,base,min,max);
    } // 顶点高度完整后才构造形状。
    let triangles=chunk.geometry.triangles.iter().copied().filter(|t|{
        let [a,b,c]=t.map(|i|vertices[i as usize]);(b-a).cross(c-a).length_squared()>1.0e-16
    }).collect();
    Collider::try_trimesh_with_config(
        // 内部边修复不能替代跨块接缝。
        vertices,                         // 独占高度输出交给物理形状。
        triangles, // 收敛后的零面积边面不进入物理；有效边仍使用共同端点。
        TrimeshFlags::FIX_INTERNAL_EDGES, // 保持既有Avian构造配置。
    )
    .map_err(|error| format!("地形洞口碰撞烘焙失败：{error}")) // 损坏拓扑返回可回退错误，不能使后台任务panic。
} // 串行对照及worker共同使用。
