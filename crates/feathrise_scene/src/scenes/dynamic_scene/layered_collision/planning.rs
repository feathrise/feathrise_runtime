//! 完整世界格径向选级及2:1邻接平衡，不裁圆、不使用任意三角剖分。
use super::types::{CellPlan, ChunkKey, ChunkPlan, Plan}; // 纯计划。
use bevy::prelude::Vec2; // 玩家中心。
use feathrise_scene_core::residency::TerrainSupportRows; // 实际覆盖索引。
use feathrise_scene_content::{SceneCollisionLod, sparse_height::SparseHeightSpec}; // 作者格。
use std::{
    collections::{BTreeMap, HashSet, VecDeque}, // 有序发布身份与后台邻接查找分离。
    sync::Arc,
}; // 有界后台集合。
pub(super) const CHUNK_CELLS: u64 = 32; // 通常32单元。
pub(super) const MAX_CHUNKS: usize = 512; // 管理块预算不变。
const MAX_CELLS: usize = MAX_CHUNKS * 1024; // 完整叶硬预算。
type CellKey = (u8, u64, u64); // 精度级、基础格X、Z。
#[path = "planning_cache.rs"] // 跨次缓存仅保存未平衡径向组，不改变2:1规则。
mod planning_cache; // 纯后台计算，生命周期由已采用布局持有。
#[cfg(test)] // 工作量只供验证，不增加运行时计数。
pub(super) use planning_cache::take_cache_counts; // 重建组、复用组、整计划复用。
pub(super) use planning_cache::{PlanCache, plan_cached}; // 生产布局统一入口。
#[cfg(test)] // 旧测试中的有序集合对照仍可使用。
use std::collections::BTreeSet; // 不增加生产集合。
#[cfg(test)] // 仅测试统计工作量，避免计时噪声掩盖重复扫描。
std::thread_local! { static WORK_COUNTS: std::cell::Cell<[usize; 3]> = const { std::cell::Cell::new([0; 3]) }; } // 邻接查询、平衡全扫描、距离计算。
#[cfg(test)] // 参考与生产测试路径共享计数器，但不共享算法。
fn count_work(slot: usize) {
    WORK_COUNTS.with(|counts| {
        let mut value = counts.get();
        value[slot] += 1;
        counts.set(value);
    });
} // 每线程隔离。
#[cfg(test)] // 调用者在单个完整计划后读取并清零。
pub(super) fn take_work_counts() -> [usize; 3] {
    WORK_COUNTS.with(|counts| counts.replace([0; 3]))
} // 返回邻接、全扫描、距离次数。

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
    #[cfg(test)] // 生产构建无计数开销。
    count_work(2); // 距离选级工作量。
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
fn select_band(spec: &SparseHeightSpec, center: Vec2, key: CellKey, bands: &[(f64, u8)]) -> usize {
    // 同一叶的选级和管理分组共用此结果。
    let distance = distance_squared(spec, center, key); // 点到完整格边界的原始距离。
    band_for_distance(distance, bands) // 初始筛选也复用同一公式。
} // 每个新叶仅计算一次距离。
fn band_for_distance(distance: f64, bands: &[(f64, u8)]) -> usize {
    // 接收已经计算的距离避免最外层重复计算。
    bands
        .iter()
        .position(|(radius_squared, _)| distance <= *radius_squared)
        .unwrap_or(bands.len() - 1) // 最外完整格允许保守外扩。
} // 缓存仅当前候选叶，不跨次刷新保存。

fn split(
    spec: &SparseHeightSpec,
    leaves: &mut BTreeMap<CellKey, usize>, // 当前叶保存径向band，不再重复计算。
    bands: &[(f64, u8)],                   // 已预计算半径平方与精度。
    center: Vec2,                          // 当前候选的固定中心。
    key: CellKey,
) -> Result<(), String> {
    // 一次有限四分。
    if key.0 == 0 || leaves.remove(&key).is_none() {
        return Ok(());
    } // 已处理或最细。
    let step = 1_u64 << (key.0 - 1); // 子格半边。
    for dx in [0, step] {
        for dz in [0, step] {
            // 至多四子格。
            let child = (key.0 - 1, key.1 + dx, key.2 + dz); // 世界固定格。
            if child.1 < spec.cells[0] && child.2 < spec.cells[1] {
                leaves.insert(child, select_band(spec, center, child, bands)); // 新叶单独选band，不能继承跨环带父叶。
            } // 只裁场景外。
        }
    } // 父格覆盖不变。
    if leaves.len() > MAX_CELLS {
        return Err("碰撞完整单元超过524288预算，请减小距离或提高倍率".into());
    } // 不降精度。
    Ok(()) // 完整候选。
} // 后台调用。
fn at(
    leaves: &BTreeMap<CellKey, usize>,
    index: Option<&HashSet<CellKey>>, // 后台固定快照索引；测试可退回原有树查询核对等价。
    max_level: u8,
    hint_level: u8, // 相邻叶通常与当前叶同精度，先尝试一次直接命中。
    x: i64,
    z: i64,
    spec: &SparseHeightSpec,
) -> Option<CellKey> {
    #[cfg(test)] // 包含平衡与最终接缝查询。
    count_work(0); // 邻接探测次数。
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
        if index.map_or_else(|| leaves.contains_key(&key), |index| index.contains(&key)) {
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
    plan_queued(spec, lods, center) // 运行时与保存预检共用相同的后台优化。
} // 不增加跨帧缓存或主线程扫描。
fn plan_queued(
    spec: &SparseHeightSpec,  // 原始作者格。
    lods: &[SceneCollisionLod], // 用户距离和倍率保持不变。
    center: Vec2,             // 当前后台请求中心。
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
    let bands: Vec<_> = lods
        .iter()
        .map(|lod| {
            (
                f64::from(lod.max_distance_meters).powi(2),
                lod.spacing_multiplier.trailing_zeros() as u8,
            )
        })
        .collect(); // 本次调用只计算一次配置派生值。
    let mut leaves = BTreeMap::new(); // 无重叠叶及各自管理band。
    let mut rows = BTreeMap::<i64, [i64; 2]>::new(); // 连续行覆盖。
    for z in ranges[1].0..ranges[1].1 {
        for x in ranges[0].0..ranges[0].1 {
            // 最粗完整格。
            let key = (max_level, x as u64 * step, z as u64 * step); // 基础格原点。
            let distance = distance_squared(spec, center, key); // 覆盖筛选与管理选级共用一次距离计算。
            if distance <= radius * radius {
                // 只选择与圆相交格。
                leaves.insert(key, band_for_distance(distance, &bands)); // 后续细分不删除覆盖。
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
        let keys: Vec<_> = leaves // 每层只检查所属叶，选级复用生成时记录。
            .range((level, 0, 0)..=(level, u64::MAX, u64::MAX)) // 有序级范围。
            .filter_map(|(&key, &band)| (bands[band].1 < level).then_some(key)) // 保留原保守细化规则。
            .collect(); // 临时列表受叶预算限制。
        for key in keys {
            // 分裂不会遍历失效父叶。
            split(spec, &mut leaves, &bands, center, key)?; // 子叶记录自己的band。
        } // 保守精度细化。
    } // 径向选级完成。
    finish_plan(spec, lods, center, leaves, spacing, rows, None) // 冷路径与缓存路径共用未改动的平衡规则。
} // 缓存只替代前面的径向叶准备。
fn finish_plan(
    // 保持完整候选的同一平衡、接缝与分组路径。
    spec: &SparseHeightSpec,              // 作者网格。
    lods: &[SceneCollisionLod],             // 当前配置。
    center: Vec2,                         // 本次真实请求位置。
    mut leaves: BTreeMap<CellKey, usize>, // 当前版本未平衡叶，绝不沿用旧版平衡细叶。
    spacing: [f64; 2],                    // 最粗覆盖格距。
    rows: BTreeMap<i64, [i64; 2]>,        // 同次径向筛选覆盖。
    previous: Option<&Plan>,              // 只复用逐格完全相同的最终管理块。
) -> Result<Plan, String> {
    // 所有大集合工作仍在后台。
    let max_level = lods
        .last()
        .ok_or("碰撞层级为空")?
        .spacing_multiplier
        .trailing_zeros() as u8; // 已经通过入口校验。
    let bands: Vec<_> = lods
        .iter()
        .map(|lod| {
            (
                f64::from(lod.max_distance_meters).powi(2),
                lod.spacing_multiplier.trailing_zeros() as u8,
            )
        })
        .collect(); // 平衡新叶仍用原选带公式。
    let mut index: HashSet<_> = leaves.keys().copied().collect(); // 仅建一次，后续分裂就地更新。
    #[cfg(test)] // 只有初始化需要完整平衡扫描。
    count_work(1); // 后续仅新增叶入队。
    let mut queue: VecDeque<_> = leaves.keys().copied().collect(); // 初始完整检查一次，之后仅检查新增叶。
    while let Some(key) = queue.pop_front() {
        // 局部传播直到队列耗尽。
        if !index.contains(&key) {
            // 先前粗邻分裂会使队列里的父叶过期。
            continue; // 不重复处理旧身份。
        } // 只检查当前叶。
        for (x, z) in probes(key, spec) {
            // 当前细叶逐边主动检查。
            loop {
                // 分裂后必须继续检查原细叶，跳级可能需要多次四分。
                let Some(other) = at(&leaves, Some(&index), max_level, key.0, x, z, spec) else {
                    break;
                }; // 图外不补地面。
                if other.0 <= key.0 + 1 {
                    break;
                } // 已满足原2:1连接约束。
                split(spec, &mut leaves, &bands, center, other)?; // 单调细化；预算错误原样返回。
                index.remove(&other); // 父身份立即失效，不能留到下一轮。
                let child_step = 1_u64 << (other.0 - 1); // 与split完全一致的子格跨度。
                for dx in [0, child_step] {
                    // 两个横向子格。
                    for dz in [0, child_step] {
                        // 两个纵向子格。
                        let child = (other.0 - 1, other.1 + dx, other.2 + dz); // 固定世界格点。
                        if child.1 < spec.cells[0] && child.2 < spec.cells[1] {
                            // 场景尾部裁切。
                            index.insert(child); // 当前索引与叶集合同步。
                            queue.push_back(child); // 新细叶继续检查外侧粗邻，传播必要过渡。
                        } // 每个实际新叶仅入队一次。
                    } // 不扫描整个窗口。
                } // 原细叶下一轮会查询新覆盖邻叶。
            } // 当前边平衡完成。
        } // 当前叶四边完成。
    } // 最终索引直接复用到接缝分组，不再重建。
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
    for (key, &band) in &leaves {
        // 每叶恰归属一个块，band已在生成时记录。
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
            if at(&leaves, Some(&index), max_level, key.0, x, z, spec)
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
            if let Some(old) = previous
                .and_then(|plan| plan.chunks.get(&key))
                .filter(|old| old.cells.as_deref() == Some(&cells))
            {
                // 已逐格确认含接缝完全相同。
                return (key, old.clone()); // 共享原单元Arc及摘要，不对未变块再次SHA256。
            } // 变化块仍走原稳定字节序。
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

#[cfg(test)] // 私有参考入口只用于后台优化的逐单元回归。
#[path = "planning_tests.rs"] // 独立测试文件不扩充生产规划主体。
mod planning_tests; // 不增加运行时系统。

#[cfg(test)] // 独立冻结参考不进入游戏产物。
#[path = "planning_reference.rs"] // 保留优化前的整轮扫描实现。
#[allow(dead_code)] // 冻结副本中的公开几何辅助函数仅用于保持历史算法完整。
mod planning_reference; // 与队列算法没有共享平衡逻辑。
#[cfg(test)] // 本轮优化基线使用上一版本的哈希查询。
pub(super) fn plan_reference(
    spec: &SparseHeightSpec,
    lods: &[SceneCollisionLod],
    center: Vec2,
) -> Result<Plan, String> {
    // 供独立等价和性能回归。
    planning_reference::plan(spec, lods, center) // 原算法完整执行。
} // 不调用新队列。
#[cfg(test)] // 保留先前测试接口。
fn plan_indexed(
    spec: &SparseHeightSpec,
    lods: &[SceneCollisionLod],
    center: Vec2,
    indexed: bool,
) -> Result<Plan, String> {
    // false仍对应原树查询。
    if indexed {
        plan(spec, lods, center)
    } else {
        planning_reference::plan_indexed(spec, lods, center, false)
    } // 两条独立算法对照。
} // 不暴露运行时开关。
