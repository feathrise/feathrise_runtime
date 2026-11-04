//! 跨刷新共享固定粗格组的径向决策；证书失效组重建，完整2:1平衡仍使用权威冷算法。
use super::*; // 共用作者格距离、band和最终平衡函数。
const ROOTS_PER_GROUP: i64 = 8; // 每组最多64个最粗格，避免为每个远处单格分配Arc。
type GroupKey = (i64, i64); // 世界固定最粗格组坐标，不随玩家平移。

#[derive(Debug)] // Layout诊断可显示缓存身份，不公开可变数据。
pub(in super::super) struct PlanCache {
    // 当前及一个候选可共享，绝不链接历史版本。
    spec: SparseHeightSpec,                       // 完整作者规格变化时冷回退。
    lods: Vec<MapCollisionLod>,                   // 距离、倍率及管理带均纳入身份。
    center: Vec2, // 完整计划的计算位置，平衡新增叶band也依赖该位置。
    groups: BTreeMap<GroupKey, Arc<RadialGroup>>, // 只保留本次枚举窗口的组。
    plan: Arc<Plan>, // 同位置径向叶不变时共享已经验证过的完整计划。
} // 高度修订不参与纯拓扑身份，调用者仍单独检查高度源。

#[derive(Debug)] // 不包含历史cache/父指针，移出窗口即可释放。
struct RadialGroup {
    // 组内叶存连续数组，避免跨帧保留大型逐叶树节点。
    ranges: [(i64, i64); 2], // 实际最粗枚举边界变化时必须重判，即使证书仍有效。
    anchor: Vec2,            // 证书生成位置；复用时禁止重置，防止累计漂移越界。
    margin: f64,             // 所有径向判定到阈值的最小距离，已扣浮点保守误差。
    leaves: Vec<(CellKey, usize)>, // 本组未平衡径向叶及管理band。
    rows: BTreeMap<i64, [i64; 2]>, // 实际最粗选中格的局部覆盖区间。
} // 只共享不可变组。

#[cfg(test)] // 计数不进入正式游戏。
std::thread_local! { static COUNTS: std::cell::Cell<[usize; 3]> = const { std::cell::Cell::new([0; 3]) }; } // 重建组、复用组、完整计划复用。
#[cfg(test)] // 每线程隔离以兼容并行测试。
fn count(slot: usize) {
    COUNTS.with(|counts| {
        let mut value = counts.get();
        value[slot] += 1;
        counts.set(value);
    });
} // 只加本线程工作量。
#[cfg(test)] // 调用者先清零再执行一个候选。
pub(in super::super) fn take_cache_counts() -> [usize; 3] {
    COUNTS.with(|counts| counts.replace([0; 3]))
} // 返回后复位。

fn displacement(a: Vec2, b: Vec2) -> f64 {
    // f32世界位置先升精度再相减，不能先做f32长度。
    (f64::from(a.x) - f64::from(b.x)).hypot(f64::from(a.y) - f64::from(b.y)) // 对AABB距离变化的1-Lipschitz上界。
} // 不预测移动方向。
fn retain_margin(margin: &mut f64, distance: f64, radii: &[f64], error: f64) {
    // 所有阈值都影响band，包含同倍率薄环。
    let distance = distance.sqrt(); // 原判定仍用平方距离，平方根仅用于保守证书。
    for &radius in radii {
        // 有限配置列表。
        *margin = margin.min(((distance - radius).abs() - error).max(0.0)); // 临界点证书归零，必须重算。
    } // 不能只检查最终精度倍率。
} // 使用严格位移<裕量，等号也重新计算。
fn build_group(
    // 失效组从最粗格重新规划，天然允许细叶合并而非只分裂。
    spec: &SparseHeightSpec, // 作者格。
    bands: &[(f64, u8)],     // 原平方阈值与精度。
    radii: &[f64],           // 米制阈值只用于证书。
    center: Vec2,            // 当前请求位置。
    max_level: u8,           // 最粗格指数。
    ranges: [(i64, i64); 2], // 该组与原枚举矩形的交集。
    error: f64,              // 与世界坐标量级相关的保守数值余量。
    leaf_budget: usize,      // 扣除已处理组后的剩余单元预算，避免一个失效组再分配整份窗口上限。
) -> Result<RadialGroup, String> {
    // 返回未平衡叶，不缓存旧版接缝传播。
    let mut group = RadialGroup {
        ranges,
        anchor: center,
        margin: f64::INFINITY,
        leaves: Vec::new(),
        rows: BTreeMap::new(),
    }; // 新证书只属于本次位置。
    let step = 1_u64 << max_level; // 最粗格跨度。
    let mut pending = Vec::new(); // 深度优先待检查项，上界受格子和层级预算限制。
    for z in ranges[1].0..ranges[1].1 {
        // 固定世界粗格行。
        for x in ranges[0].0..ranges[0].1 {
            // 固定世界粗格列。
            let key = (max_level, x as u64 * step, z as u64 * step); // 与冷算法相同世界坐标。
            let distance = distance_squared(spec, center, key); // 原精确平方距离。
            retain_margin(&mut group.margin, distance, radii, error); // 未选中的外圈格也必须记录证书。
            if distance <= bands.last().unwrap().0 {
                // 仅原圆相交粗格建立覆盖。
                group
                    .rows
                    .entry(z)
                    .and_modify(|range| range[1] = x + 1)
                    .or_insert([x, x + 1]); // 保存同版实际覆盖。
                pending.push((key, band_for_distance(distance, bands))); // 根距离只计算一次。
            } // 图外或圆外不创建碰撞。
        } // 每组至多64个最粗根。
    } // 根判定和径向细化分开，覆盖不会被子叶删除。
    while let Some((key, band)) = pending.pop() {
        // 子树深度优先生成与冷算法相同的最终径向叶。
        if key.0 > bands[band].1 {
            // 必须细化时不能保留父叶。
            let child_step = 1_u64 << (key.0 - 1); // 四叉子格跨度。
            for dx in [0, child_step] {
                // 两列子格。
                for dz in [0, child_step] {
                    // 两行子格。
                    let child = (key.0 - 1, key.1 + dx, key.2 + dz); // 固定世界子格。
                    if child.1 < spec.cells[0] && child.2 < spec.cells[1] {
                        // 沿用地图尾部裁切。
                        let distance = distance_squared(spec, center, child); // 每子节点只计算一次。
                        retain_margin(&mut group.margin, distance, radii, error); // 分裂与最终band的全部决策都纳入证书。
                        pending.push((child, band_for_distance(distance, bands))); // 子格不能继承父band。
                    } // 不生成地图外子格。
                } // 保持原有效覆盖。
            } // 离开近层后重建时不会进入此分支，自动合并。
        } else {
            // 不再需要径向细化。
            group.leaves.push((key, band)); // 最终原始叶，平衡后续统一执行。
            if group.leaves.len() > leaf_budget {
                return Err("碰撞完整单元超过524288预算，请减小距离或提高倍率".into());
            } // 达到剩余额度立即交回冷算法确认原错误，组间临时叶不翻倍。
        } // 不缓存任何由旧邻居强迫生成的细格。
    } // 完成整组纯径向规划。
    group.leaves.sort_unstable_by_key(|(key, _)| *key); // 与BTree冷算法相同顺序，也便于稳定内容比较。
    Ok(group) // 证书可严格证明同一组全部判定保持不变。
} // 错误回到权威冷算法确定原有拒绝语义。

pub(in super::super) fn plan_cached(
    // 只在后台布局任务调用。
    spec: &SparseHeightSpec,      // 当前作者格。
    lods: &[MapCollisionLod],     // 当前用户设置。
    center: Vec2,                 // 当前请求玩家位置，不引入预测中心。
    previous: Option<&PlanCache>, // 仅已采用布局可成为缓存源。
) -> Result<(Plan, Arc<PlanCache>), String> {
    // 当前与候选以Arc共享稳定组。
    spec.validate()?; // 与冷入口相同校验次序。
    feathrise_map_content::validate_collision_lods(lods, spec)?; // 不以缓存绕过保存校验。
    if !center.is_finite() {
        return Err("碰撞中心无效".into());
    } // 拒绝NaN/Inf。
    let previous = previous.filter(|cache| cache.spec == *spec && cache.lods == lods); // 配置或规格不同直接冷建组。
    let outer = lods.last().ok_or("碰撞层级为空")?; // 已校验非空。
    let max_level = outer.spacing_multiplier.trailing_zeros() as u8; // 原最粗层。
    let step = 1_u64 << max_level; // 作者格跨度。
    let spacing = spec.actual_spacing_meters.map(|value| value * step as f64); // 最粗覆盖格距。
    let radius = f64::from(outer.max_distance_meters); // 当前外层半径。
    let ranges: [(i64, i64); 2] = std::array::from_fn(|axis| {
        // 完全保留冷算法枚举矩形。
        let c = f64::from(center[axis]) - f64::from(spec.world_min[axis]); // 相对地图原点。
        let count = spec.cells[axis].div_ceil(step) as i64; // 包含地图尾格。
        (
            ((c - radius) / spacing[axis]).floor().max(0.0) as i64,
            (((c + radius) / spacing[axis]).floor() as i64 + 1).clamp(0, count),
        ) // 原左闭右开公式。
    }); // 边界组必须记录此裁切结果。
    let empty = ranges.iter().any(|(lo, hi)| lo >= hi); // 与冷算法一致，空窗口在预算检查前返回。
    if !empty
        && (ranges[0].1 - ranges[0].0) as u64 * (ranges[1].1 - ranges[1].0) as u64
            > (MAX_CELLS * 2) as u64
    {
        return Err("碰撞覆盖超过完整单元预算，请减小距离或提高倍率".into());
    } // 原枚举硬门。
    let bands: Vec<_> = lods
        .iter()
        .map(|lod| {
            (
                f64::from(lod.max_distance_meters).powi(2),
                lod.spacing_multiplier.trailing_zeros() as u8,
            )
        })
        .collect(); // 原平方阈值。
    let radii: Vec<_> = lods
        .iter()
        .map(|lod| f64::from(lod.max_distance_meters))
        .collect(); // 证书米制半径。
    let scale = spec
        .world_min
        .into_iter()
        .chain(spec.world_max)
        .map(|v| f64::from(v).abs())
        .fold(
            radius
                .max(f64::from(center.x).abs())
                .max(f64::from(center.y).abs())
                .max(1.0),
            f64::max,
        ); // 世界运算的保守量级。
    let error = scale * f64::EPSILON * 128.0; // 远大于距离平方/平方根的舍入误差，临界判定不缓存。
    let mut groups = BTreeMap::new(); // 只构造本次窗口的组索引，不引用旧索引或历史链。
    let mut unchanged = previous.is_some_and(|cache| cache.center == center); // 移动必须重验平衡子叶，提前短路无用的全组叶深比较。
    let mut leaf_count = 0_usize; // 原始叶总预算，避免聚合时超过原硬门。
    if !empty {
        // 图外空计划不枚举任何组。
        for gz in ranges[1].0 / ROOTS_PER_GROUP..=(ranges[1].1 - 1) / ROOTS_PER_GROUP {
            // 覆盖粗格组行。
            for gx in ranges[0].0 / ROOTS_PER_GROUP..=(ranges[0].1 - 1) / ROOTS_PER_GROUP {
                // 覆盖粗格组列。
                let key = (gx, gz); // 世界固定组身份。
                let local = std::array::from_fn(|axis| {
                    let group = [gx, gz][axis];
                    (
                        (group * ROOTS_PER_GROUP).max(ranges[axis].0),
                        ((group + 1) * ROOTS_PER_GROUP).min(ranges[axis].1),
                    )
                }); // 组与原枚举盒精确交集。
                let old = previous.and_then(|cache| cache.groups.get(&key)); // 不存在的进入组必须创建。
                let reusable = old.filter(|group| {
                    group.ranges == local
                        && (group.anchor == center
                            || displacement(group.anchor, center) + error < group.margin)
                }); // 同位置精确安全；移动时严格保守证书。
                let group = if let Some(group) = reusable {
                    // 整组决策已有数学有效范围。
                    #[cfg(test)] // 统计实际减少的组计算。
                    count(1); // 复用组。
                    Arc::clone(group) // 证书锚点不更新。
                } else {
                    // 任一决策可能跨阈值则从粗格重建。
                    #[cfg(test)] // 冷/失效组都计实际重建。
                    count(0); // 重建组。
                    let built = match build_group(
                        spec,
                        &bands,
                        &radii,
                        center,
                        max_level,
                        local,
                        error,
                        MAX_CELLS - leaf_count,
                    ) {
                        Ok(group) => group,
                        Err(_) => {
                            return super::plan(spec, lods, center).map(|plan| {
                                (
                                    plan.clone(),
                                    Arc::new(PlanCache {
                                        spec: spec.clone(),
                                        lods: lods.to_vec(),
                                        center,
                                        groups: BTreeMap::new(),
                                        plan: Arc::new(plan),
                                    }),
                                )
                            });
                        }
                    }; // 超限时保留冷算法的准确错误与预算次序。
                    unchanged = unchanged
                        && old.is_some_and(|old| {
                            old.leaves == built.leaves && old.rows == built.rows
                        }); // 只在仍可能整Plan复用时比较组数据，移动不额外扫描数组。
                    Arc::new(built) // 不保留旧组引用。
                }; // 当前组身份已确定。
                leaf_count += group.leaves.len(); // 各组互斥，没有重复单元。
                if leaf_count > MAX_CELLS {
                    return super::plan(spec, lods, center).map(|plan| {
                        (
                            plan.clone(),
                            Arc::new(PlanCache {
                                spec: spec.clone(),
                                lods: lods.to_vec(),
                                center,
                                groups: BTreeMap::new(),
                                plan: Arc::new(plan),
                            }),
                        )
                    });
                } // 错误同样走原权威预算检查，不偷偷降低精度。
                groups.insert(key, group); // 仅保留新窗口，不累计走过的区域。
            } // 每个组恰好处理一次。
        } // 同一完整请求的全部径向组。
    } // 无覆盖仍可缓存空计划。
    unchanged &= previous.is_some_and(|cache| cache.groups.len() == groups.len()); // 离开窗口的旧组也必须让完整复用失效。
    unchanged &= previous.is_some_and(|cache| cache.center == center); // 原始父叶band不变并不保证平衡新增子叶band不变；移动时必须重新完整平衡/分组。
    let plan = if unchanged {
        // 同位置且径向输入完全相同，确定性2:1和接缝输出必然相同。
        #[cfg(test)] // 统计完整计划真实命中，与移动时组复用区分。
        count(2); // 同位置高度修订可完整复用纯拓扑。
        Arc::clone(&previous.unwrap().plan) // 不再扫描叶、平衡、分组或哈希。
    } else if empty {
        // 与冷计划一致的图外结果。
        Arc::new(Plan {
            chunks: BTreeMap::new(),
            outer_boundary: Vec::new(),
            supported_rows: None,
        }) // 不生成图外物理地面。
    } else {
        // 径向局部更新后继续原完整平衡，不猜测接缝传播半径。
        let leaves = groups
            .values()
            .flat_map(|group| group.leaves.iter().copied())
            .collect(); // 一次批量归并由BTreeMap排序建树，不逐组重复插入有序树；单元唯一性由互斥世界分组保证。
        let mut rows = BTreeMap::<i64, [i64; 2]>::new(); // 当前完整覆盖。
        for group in groups.values() {
            // 共享组只读取连续数组。
            for (&z, &range) in &group.rows {
                rows.entry(z)
                    .and_modify(|row| {
                        row[0] = row[0].min(range[0]);
                        row[1] = row[1].max(range[1]);
                    })
                    .or_insert(range);
            } // 径向圆与矩形行相交保证连续覆盖。
        } // 旧平衡叶没有进入新候选，保证可合并。
        Arc::new(finish_plan(
            spec,
            lods,
            center,
            leaves,
            spacing,
            rows,
            previous.map(|cache| cache.plan.as_ref()),
        )?) // 接缝逐格重新验证，未变块可共享摘要。
    }; // 完整计划已成功才能生成缓存版本。
    let result = plan.as_ref().clone(); // 只克隆最多512个小块身份并共享cells/rows。
    Ok((
        result,
        Arc::new(PlanCache {
            spec: spec.clone(),
            lods: lods.to_vec(),
            center,
            groups,
            plan,
        }),
    )) // 新缓存没有指向上一cache的历史链。
} // 发布与地图/根/高度票据仍由既有驱动统一核验。
