//! 冻结 cbb9d932 的完整规划算法，仅测试编译；防止优化与参考同时改变。
//! 完整世界格径向选级及2:1邻接平衡，不裁圆、不使用任意三角剖分。
use super::super::types::{CellPlan, ChunkKey, ChunkPlan, Plan}; // 纯计划。
use bevy::prelude::Vec2; // 玩家中心。
use feathrise_scene_core::residency::TerrainSupportRows; // 实际覆盖索引。
use feathrise_scene_content::{SceneCollisionLod, sparse_height::SparseHeightSpec}; // 作者格。
use std::{
    collections::{BTreeMap, BTreeSet, HashSet}, // 有序发布身份与后台邻接查找分离。
    sync::Arc,
}; // 有界后台集合。
pub(super) const CHUNK_CELLS: u64 = 32; // 通常32单元。
pub(super) const MAX_CHUNKS: usize = 512; // 管理块预算不变。
const MAX_CELLS: usize = MAX_CHUNKS * 1024; // 完整叶硬预算。
type CellKey = (u8, u64, u64); // 精度级、基础格X、Z。

pub(super) fn chunk_cells_log2(spec: &SparseHeightSpec, lods: &[SceneCollisionLod]) -> u8 {
    // 近场刷新粒度。
    lods.first()
        .map_or(5, |lod| layer_chunk_cells_log2(spec, lod, 0.0)) // 不改变精度。
} // 远层独立。
fn layer_chunk_cells_log2(spec: &SparseHeightSpec, lod: &SceneCollisionLod, previous: f32) -> u8 {
    // 管理分组。
    let spacing = spec.actual_spacing_meters[0].max(spec.actual_spacing_meters[1]); // 宽轴。
    let mut log2 = CHUNK_CELLS.trailing_zeros() as u8; // 至多32格。
    let width = f64::from(lod.max_distance_meters) - f64::from(previous); // 独立带宽。
    while log2 > 0 && spacing * f64::from(lod.spacing_multiplier) * (1_u64 << log2) as f64 > width {
        // 小范围局部更新。
        log2 -= 1; // 保持对齐。
    } // 不继承近层小块。
    log2 // 稳定身份。
} // 不裁几何。
pub(super) fn chunk_bounds(spec: &SparseHeightSpec, key: ChunkKey) -> ([u64; 2], [u64; 2]) {
    // 管理盒。
    let span = 1_u64 << (key.cells_log2 + key.level); // 基础格跨度。
    let lo = [u64::from(key.x) * span, u64::from(key.z) * span]; // 世界固定原点。
    let hi = [
        (lo[0] + span).min(spec.cells[0]),
        (lo[1] + span).min(spec.cells[1]),
    ]; // 场景尾边。
    (lo, hi) // 实际覆盖另有行索引。
} // 不冒充完整地面。
fn distance_squared(spec: &SparseHeightSpec, center: Vec2, key: CellKey) -> f64 {
    super::count_work(2); // 测试工作量统计，不改变冻结算法。
    // 点到格子的距离。
    let step = 1_u64 << key.0; // 格跨度。
    [key.1, key.2]
        .into_iter()
        .enumerate()
        .map(|(axis, value)| {
            // 分轴钳制。
            let lo =
                f64::from(spec.world_min[axis]) + value as f64 * spec.actual_spacing_meters[axis]; // 世界左界。
            let hi = f64::from(spec.world_min[axis])
                + (value + step).min(spec.cells[axis]) as f64 * spec.actual_spacing_meters[axis]; // 右界。
            let point = f64::from(center[axis]); // 玩家位置。
            (point - point.clamp(lo, hi)).powi(2) // 接触近层时保守细化。
        })
        .sum() // 平方距离。
} // 无预取。
fn split(
    spec: &SparseHeightSpec,
    leaves: &mut BTreeSet<CellKey>,
    key: CellKey,
) -> Result<(), String> {
    // 一次有限四分。
    if key.0 == 0 || !leaves.remove(&key) {
        return Ok(());
    } // 已处理或最细。
    let step = 1_u64 << (key.0 - 1); // 子格半边。
    for dx in [0, step] {
        for dz in [0, step] {
            // 至多四子格。
            let child = (key.0 - 1, key.1 + dx, key.2 + dz); // 世界固定格。
            if child.1 < spec.cells[0] && child.2 < spec.cells[1] {
                leaves.insert(child);
            } // 只裁场景外。
        }
    } // 父格覆盖不变。
    if leaves.len() > MAX_CELLS {
        return Err("碰撞完整单元超过524288预算，请减小距离或提高倍率".into());
    } // 不降精度。
    Ok(()) // 完整候选。
} // 后台调用。
fn at(
    leaves: &BTreeSet<CellKey>,
    index: Option<&HashSet<CellKey>>, // 后台固定快照索引；测试可退回原有树查询核对等价。
    max_level: u8,
    hint_level: u8, // 相邻叶通常与当前叶同精度，先尝试一次直接命中。
    x: i64,
    z: i64,
    spec: &SparseHeightSpec,
) -> Option<CellKey> {
    super::count_work(0); // 测试邻接探测次数，不改变查询顺序。
    // 唯一覆盖叶查询。
    if x < 0 || z < 0 || x >= spec.cells[0] as i64 || z >= spec.cells[1] as i64 {
        return None;
    } // 场景外。
    if let Some(index) = index {
        // 参考树查询保留原0..max顺序，避免对照双方一起改变策略。
        let step = 1_u64 << hint_level; // 使用当前叶精度构造邻居键。
        let key = (hint_level, x as u64 / step * step, z as u64 / step * step); // 与原查询相同的对齐公式。
        if index.contains(&key) {
            // 无重叠叶保证命中同级就是唯一覆盖叶。
            return Some(key); // 大片层内部不再从最细层逐级查到本层。
        } // 精度边界才进入完整查找。
    } // 不改变参考路径。
    for level in 0..=max_level {
        // 合法高度层数有界。
        if index.is_some() && level == hint_level {
            // 索引路径已试过此级，参考路径不能跳过。
            continue; // 不重复哈希同一个不存在的键。
        } // 其余级依然完整扫描，不假设平衡已经完成。
        let step = 1_u64 << level; // 父格跨度。
        let key = (level, x as u64 / step * step, z as u64 / step * step); // 对齐键。
        if index.map_or_else(|| leaves.contains(&key), |index| index.contains(&key)) {
            // 每层邻接不再重复进行对数树搜索。
            return Some(key);
        } // 无重叠唯一叶。
    } // 不补块。
    None // 无覆盖。
} // 层数有限。
fn probes(key: CellKey, spec: &SparseHeightSpec) -> [(i64, i64); 4] {
    // 每边外侧探测点。
    let step = 1_u64 << key.0; // 边长。
    let hi = [
        (key.1 + step).min(spec.cells[0]),
        (key.2 + step).min(spec.cells[1]),
    ]; // 尾格。
    let mid = [
        ((key.1 + hi[0]) / 2).min(hi[0] - 1),
        ((key.2 + hi[1]) / 2).min(hi[1] - 1),
    ]; // 内部基础格。
    [
        (key.1 as i64 - 1, mid[1] as i64),
        (hi[0] as i64, mid[1] as i64),
        (mid[0] as i64, key.2 as i64 - 1),
        (mid[0] as i64, hi[1] as i64),
    ] // 西东南北。
} // 所有细叶主动检查，粗边不漏小细邻。
pub(super) fn plan(
    spec: &SparseHeightSpec,
    lods: &[SceneCollisionLod],
    center: Vec2,
) -> Result<Plan, String> {
    plan_indexed(spec, lods, center, true) // 运行时与保存预检共用相同的后台优化。
} // 不增加跨帧缓存或主线程扫描。
pub(super) fn plan_indexed(
    spec: &SparseHeightSpec,  // 原始作者格。
    lods: &[SceneCollisionLod], // 用户距离和倍率保持不变。
    center: Vec2,             // 当前后台请求中心。
    indexed: bool,            // false只用于与优化前的树查询做逐单元等价回归。
) -> Result<Plan, String> {
    // 所有规模工作仍在后台。
    // 后台纯算法。
    spec.validate()?; // 作者格校验。
    feathrise_scene_content::validate_collision_lods(lods, spec)?; // 配置校验。
    if !center.is_finite() {
        return Err("碰撞中心无效".into());
    } // 拒绝NaN。
    let outer = lods.last().ok_or("碰撞层级为空")?; // 最远层。
    let max_level = outer.spacing_multiplier.trailing_zeros() as u8; // 最大精度指数。
    let step = 1_u64 << max_level; // 最粗完整格。
    let spacing = spec.actual_spacing_meters.map(|value| value * step as f64); // 世界格距。
    let radius = f64::from(outer.max_distance_meters); // 用户半径。
    let ranges: [(i64, i64); 2] = std::array::from_fn(|axis| {
        // 最粗格枚举盒。
        let c = f64::from(center[axis]) - f64::from(spec.world_min[axis]); // 相对场景原点。
        let count = spec.cells[axis].div_ceil(step) as i64; // 含尾格。
        (
            ((c - radius) / spacing[axis]).floor().max(0.0) as i64,
            (((c + radius) / spacing[axis]).floor() as i64 + 1).clamp(0, count),
        ) // 左闭右开。
    }); // 后续精确格距离筛选。
    if ranges.iter().any(|(lo, hi)| lo >= hi) {
        return Ok(Plan {
            chunks: BTreeMap::new(),
            outer_boundary: Vec::new(),
            supported_rows: None,
        });
    } // 场景外。
    if (ranges[0].1 - ranges[0].0) as u64 * (ranges[1].1 - ranges[1].0) as u64
        > (MAX_CELLS * 2) as u64
    {
        return Err("碰撞覆盖超过完整单元预算，请减小距离或提高倍率".into());
    } // 枚举前硬门。
    let mut leaves = BTreeSet::new(); // 无重叠叶。
    let mut rows = BTreeMap::<i64, [i64; 2]>::new(); // 连续行覆盖。
    for z in ranges[1].0..ranges[1].1 {
        for x in ranges[0].0..ranges[0].1 {
            // 最粗完整格。
            let key = (max_level, x as u64 * step, z as u64 * step); // 基础格原点。
            if distance_squared(spec, center, key) <= radius * radius {
                // 只选择与圆相交格。
                leaves.insert(key); // 后续细分不删除覆盖。
                rows.entry(z)
                    .and_modify(|range| range[1] = x + 1)
                    .or_insert([x, x + 1]); // 每行连续。
            } // 边缘自然有棱角。
        }
    } // 不生成凸包。
    if leaves.len() > MAX_CELLS {
        return Err("碰撞完整单元超过预算".into());
    } // 初始硬门。
    for level in (1..=max_level).rev() {
        // 粗到细。
        let keys: Vec<_> = leaves
            .range((level, 0, 0)..=(level, u64::MAX, u64::MAX)) // 树本来按level排序，不再每层扫描所有细叶。
            .copied()
            .filter(|key| {
                // 一次固定层。
                let d = distance_squared(spec, center, *key); // 到格边距离。
                lods.iter()
                    .find(|lod| d <= f64::from(lod.max_distance_meters).powi(2))
                    .is_some_and(|lod| lod.spacing_multiplier.trailing_zeros() < u32::from(level)) // 支持跳级。
            })
            .collect(); // 受叶预算限制。
        for key in keys {
            split(spec, &mut leaves, key)?;
        } // 保守精度细化。
    } // 径向选级完成。
    let index = loop {
        // 平衡完成时把最后一轮有效索引交给接缝分组，避免再建同一张表。
        super::count_work(1); // 原实现每轮完整扫描的次数。
        // 必要的2:1邻接平衡。
        let index: Option<HashSet<_>> = indexed.then(|| leaves.iter().copied().collect()); // 一轮只建一次索引，split之后下一轮重建，不使用失效索引。
        let mut coarse = BTreeSet::new(); // 合并重复粗邻。
        for key in &leaves {
            for (x, z) in probes(*key, spec) {
                // 从细叶检查。
                if let Some(other) = at(&leaves, index.as_ref(), max_level, key.0, x, z, spec) {
                    // 同级快速命中后仍完整覆盖未平衡的跳级邻居。
                    if other.0 > key.0 + 1 {
                        coarse.insert(other);
                    }
                } // 不允许跨两级。
            }
        } // 只增加必要过渡叶。
        if coarse.is_empty() {
            break index; // 叶集合未再改变，此索引可直接复用。
        } // 已平衡。
        for key in coarse {
            split(spec, &mut leaves, key)?;
        } // 不改变用户配置。
    }; // 无长距离扇形，最终邻接复用平衡的只读索引。
    let band_cells: Vec<_> = lods
        .iter()
        .enumerate()
        .map(|(band, lod)| {
            // 管理带宽只依赖配置，不再每个格重复计算。
            let previous = band
                .checked_sub(1)
                .map_or(0.0, |i| lods[i].max_distance_meters); // 各层独立带宽。
            layer_chunk_cells_log2(spec, lod, previous) // 复用原公式，精度不变。
        })
        .collect(); // 层数规模的小型后台表。
    let mut groups = BTreeMap::<ChunkKey, Vec<CellPlan>>::new(); // 管理块归并。
    for key in &leaves {
        // 每叶恰归属一个块。
        let distance = distance_squared(spec, center, *key); // 管理粒度取实际径向带，不按倍率反查第一层。
        let band = lods
            .iter()
            .position(|lod| distance <= f64::from(lod.max_distance_meters).powi(2))
            .unwrap_or(lods.len() - 1); // 同倍率远层也保留自己的大管理块。
        let cells_log2 = band_cells[band]; // 配置预计算结果，远层不会碎成近层小块。
        let span = 1_u64 << (cells_log2 + key.0); // 管理跨度。
        let chunk = ChunkKey {
            level: key.0,
            band: band as u32,
            cells_log2,
            x: (key.1 / span) as u32,
            z: (key.2 / span) as u32,
        }; // 稳定键。
        let mut finer_edges = 0; // 普通格不拆边。
        for (side, (x, z)) in probes(*key, spec).into_iter().enumerate() {
            // 平衡后的边。
            if at(&leaves, index.as_ref(), max_level, key.0, x, z, spec)
                .is_some_and(|other| other.0 < key.0)
            {
                // 同一叶索引只改变查找成本。
                finer_edges |= 1 << side;
            } // 粗侧一格模板。
        } // 不影响内部两三角。
        groups.entry(chunk).or_default().push(CellPlan {
            origin: [key.1, key.2],
            finer_edges,
        }); // 单元列表。
        if groups.len() > MAX_CHUNKS {
            return Err("碰撞覆盖超过512块预算，请减小距离或提高倍率".into());
        } // 保留硬门。
    } // 完整分组。
    let chunks = groups
        .into_iter()
        .map(|(key, cells)| {
            // 摘要仅在后台生成。
            let mut bytes = Vec::with_capacity(cells.len() * 17); // 每格两坐标加一个接缝字节。
            for cell in &cells {
                bytes.extend_from_slice(&cell.origin[0].to_le_bytes());
                bytes.extend_from_slice(&cell.origin[1].to_le_bytes());
                bytes.push(cell.finer_edges);
            } // 明确稳定字节序。
            let signature = feathrise_scene_content::authoring_io::bytes_hash(&bytes); // 复用已有SHA256，不新增依赖。
            (
                key,
                ChunkPlan {
                    key,
                    finer_edges: 0,
                    signature,
                    cells: Some(Arc::new(cells)),
                },
            ) // 无自由剖分。
        })
        .collect(); // 主线程只比较固定长度摘要。
    let supported_rows = Some(Arc::new(TerrainSupportRows {
        origin: spec.world_min.map(f64::from),
        spacing,
        bounds: [
            Vec2::from_array(spec.world_min),
            Vec2::from_array(spec.world_max),
        ],
        rows,
    })); // 同版真实覆盖。
    Ok(Plan {
        chunks,
        outer_boundary: Vec::new(),
        supported_rows,
    }) // 整版发布。
} // 失败保留旧版。
