//! 近场层的纯候选收集、工作量门控和有界当帧并行执行。

use std::collections::BTreeMap; // 上一活动集合只读参与滞回半径选择。

use bevy::prelude::{IVec2, Vec2, Vec3Swizzles}; // 候选生成与距离精筛使用世界二维位置。
use bevy::tasks::{ComputeTaskPool, ParallelSlice, TaskPool}; // 当帧重工作复用Bevy计算线程而不新建OS线程。

use crate::{
    // 纯计算沿用通用植被候选与活动布局语义。
    ActiveVegetationLayout, // 当前完整活动密度布局供所有worker并发只读采样。
    VegetationCandidate,    // 程序网格和显式清单统一转为同一候选结构。
    VegetationCandidateSource, // 候选源决定使用空间索引或整数世界格枚举。
    VegetationLayerConfig,  // 层提供原型、实现半径和候选源。
    VegetationLayerId,      // 稳定层身份参与最终联合主键排序。
    VegetationRealization,  // 只有HybridNearField层进入工作列表。
    deterministic_candidate, // 所有后端继续共享稳定候选ID、变换和密度。
}; // 完成纯候选计算依赖导入。

use super::ActiveNearFieldProxy; // 局部结果最终由主线程合并进公开运行资源。
use super::spatial_index::{
    // 显式层只查询观察者附近桶而不再扫描整份清单。
    ExplicitPlacementSpatialIndex, // 单层索引供当前layer工作项只读借用。
    ExplicitPlacementSpatialIndexes, // 配置级索引集合用于组装稳定工作列表。
}; // 完成空间索引类型导入。

const MIN_PARALLEL_LAYER_CHECKS: usize = 1_024; // 单层至少达到该检查量才值得占用一个并行分片。
const MIN_PARALLEL_TOTAL_CHECKS: usize = 4_096; // 总检查量不足时串行避免任务派发和合并反而变慢。
const MAX_PARALLEL_WORKERS: usize = 4; // 近场最多占四个计算worker，为Bevy其他帧任务保留容量。

pub(super) type NearFieldEntry = (
    // 局部结果使用连续Vec，避免worker争用共享BTreeMap节点。
    (VegetationLayerId, u64), // 层ID与稳定实例ID构成确定性联合主键。
    ActiveNearFieldProxy,     // 原型和最终Transform供主线程发布消息。
); // 完成单个近场局部结果别名。

/// 一层近场纯计算的只读输入与预估工作量。
#[derive(Clone, Copy)] // 轻量借用工作描述可按稳定层顺序切片给ComputeTaskPool。
pub(super) struct NearFieldLayerWork<'a> {
    // 工作项不持有World、Commands、ResMut或消息写入器。
    pub(super) layer: &'a VegetationLayerConfig, // 同代受检层配置只读借用，并供纯策略回归构造工作项。
    pub(super) explicit_index: Option<&'a ExplicitPlacementSpatialIndex>, // 仅显式候选源拥有额外空间索引。
    pub(super) estimated_checks: usize, // 调度门槛使用粗筛候选检查上界，并供门槛回归注入固定值。
} // 完成单层只读工作描述。

/// 本次候选收集实际采用的执行策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)] // 纯策略可由单测锁定边界且不依赖真实线程调度。
pub(super) enum NearFieldExecutionMode {
    // 串行与并行产生完全相同的稳定结果集合。
    Serial,                      // 小任务、单线程或缺任务池时直接在当前系统线程完成。
    Parallel { workers: usize }, // 大任务按层分片并在返回前等待同一帧完整结果。
} // 完成近场执行策略定义。

/// 组装全部近场层工作并按稳定层ID排序。
pub(super) fn build_layer_work<'a>(
    // 工作量估算不生成候选或扫描显式清单全集。
    layers: &'a [VegetationLayerConfig], // 当前完整层配置切片。
    indexes: &'a ExplicitPlacementSpatialIndexes, // 与configuration_generation一致的显式索引集合。
    observers: &[Vec2],                  // 已由系统按位模式稳定排序的观察者位置。
) -> Vec<NearFieldLayerWork<'a>> {
    // 返回值只在本次系统调用期间使用。
    let mut work = Vec::new(); // 非近场层不会进入分片或支付任务开销。
    for layer in layers {
        // 每层独立估算附近候选工作量。
        let VegetationRealization::HybridNearField { exit_radius, .. } = layer.realization else {
            // 仅混合近场层需要代理差分。
            continue; // GPUOnly和Entities层由其他后端负责。
        }; // 完成近场实现筛选。
        let explicit_index = indexes.get(layer.id); // 程序网格自然得到None并继续使用整数格枚举。
        let estimated_checks = estimate_layer_checks(
            // 估算仅访问附近桶长度或计算程序网格格数。
            layer,          // 当前层候选源与退出半径来源。
            explicit_index, // 显式层使用空间索引避免按总落点数误触发并行。
            observers,      // 多观察者工作量采用保守饱和累计。
            exit_radius,    // 最大滞回半径完整覆盖所有可能保留代理。
        ); // 完成当前层工作量估算。
        work.push(NearFieldLayerWork {
            // 保存只读引用供串行或并行路径共同消费。
            layer,            // 保留当前受检层配置。
            explicit_index,   // 保留当前层可选空间索引。
            estimated_checks, // 保留调度门槛无需重复估算。
        }); // 完成当前层工作项登记。
    } // 完成全部近场层工作描述组装。
    work.sort_by_key(|item| item.layer.id); // 配置输入顺序不影响并行分片或结果确定性。
    work // 返回稳定层ID升序工作列表。
} // 完成近场层工作组装。

/// 使用正式Bevy ComputeTaskPool按门槛收集全部近场候选。
pub(super) fn collect_near_field_candidates(
    // 结果完整返回后主线程才允许写消息和运行资源。
    work: &[NearFieldLayerWork<'_>], // 稳定层ID升序只读工作列表。
    observers: &[Vec2],              // 当前全部观察者X/Z位置。
    layout: &ActiveVegetationLayout, // 当前活动布局只读快照。
    active: &BTreeMap<(VegetationLayerId, u64), ActiveNearFieldProxy>, // 上一活动集合决定进入或退出半径。
) -> Vec<NearFieldEntry> {
    // 连续局部结果随后确定性合并进最终BTreeMap。
    collect_with_task_pool(
        // 单线程测试环境与正式应用共用同一执行入口。
        work,                                           // 传入稳定近场层工作列表。
        observers,                                      // 传入只读观察者位置。
        layout,                                         // 传入只读活动布局。
        active,                                         // 传入只读上一活动集合。
        ComputeTaskPool::try_get().map(|pool| &**pool), // 解包Bevy全局池新类型；缺池时串行且不初始化第二套线程。
    ) // 返回本次完整稳定候选集合。
} // 完成正式任务池候选收集入口。

pub(super) fn collect_with_task_pool(
    // 独立入口允许回归用本地TaskPool比较串并行等价性。
    work: &[NearFieldLayerWork<'_>], // 全部只读层工作。
    observers: &[Vec2],              // 全部只读观察者位置。
    layout: &ActiveVegetationLayout, // 全部worker共享不可变活动布局。
    active: &BTreeMap<(VegetationLayerId, u64), ActiveNearFieldProxy>, // 全部worker共享不可变滞回集合。
    task_pool: Option<&TaskPool>,                                      // None或不足线程时严格串行。
) -> Vec<NearFieldEntry> {
    // 无论调度顺序如何都返回稳定升序去重结果。
    let mode = task_pool // 只有已经存在的池才能启用按层并行。
        .map(|pool| execution_mode(work, pool.thread_num())) // 线程数与工作量共同决定执行模式。
        .unwrap_or(NearFieldExecutionMode::Serial); // 逻辑单测与无池环境不创建隐藏线程。
    let mut entries: Vec<NearFieldEntry> = match (mode, task_pool) {
        // 策略和池必须同时允许才进入并行分支。
        (NearFieldExecutionMode::Parallel { workers }, Some(pool)) => {
            let batches = build_parallel_batches(work, workers); // 重层优先轮转到不同有界批次，避免相邻重层落入同一Task。
            batches
                .par_chunk_map(pool, 1, |_batch_index, batch_slice| {
                    // 每个单元素切片严格对应一个有界worker局部批次。
                    let mut local = Vec::new(); // 每个任务独占连续局部结果，不锁共享BTreeMap。
                    for layer_work in &batch_slice[0] {
                        // 一个worker顺序处理轮转分配给自己的独立层。
                        local.extend(collect_layer_candidates(
                            // 每层精筛仍复用完全相同的纯函数。
                            layer_work, // 当前层配置、索引和估算。
                            observers,  // 共享只读观察者位置。
                            layout,     // 共享只读活动布局。
                            active,     // 共享只读上一活动集合。
                        )); // 完成当前层局部结果追加。
                    } // 完成当前worker分片全部层。
                    local // Task完成后一次交回局部Vec。
                }) // 同步scope等待本帧全部分片完成。
                .into_iter() // 按输入分片顺序消费局部Vec。
                .flatten() // 局部Vec拼为单一连续候选序列。
                .collect() // 收集供最终全局稳定排序和去重。
        } // 完成按需并行分支。
        _ => work // 小工作量、单线程或缺池统一走串行路径。
            .iter() // 按稳定层ID升序处理。
            .flat_map(|layer_work| {
                // 每层仍产生自己的局部连续结果。
                collect_layer_candidates(layer_work, observers, layout, active) // 串行与并行共享精筛实现。
            }) // 完成全部层候选展开。
            .collect(), // 收集为最终稳定候选序列。
    }; // 完成原始候选收集。
    entries.sort_by_key(|entry| entry.0); // worker完成顺序和观察者重叠都不能影响最终键顺序。
    entries.dedup_by(|left, right| left.0 == right.0); // 同层稳定ID只保留一个完全相同代理结果。
    entries // 返回可由主线程直接构建BTreeMap的稳定结果。
} // 完成可测试任务池候选收集。

pub(super) fn build_parallel_batches<'a>(
    // 只在大工作量并行分支分配，轻载串行路径不支付批次规划成本。
    work: &[NearFieldLayerWork<'a>], // 当前稳定层工作列表。
    workers: usize,                  // execution_mode已经限制为二至四且不超过层数。
) -> Vec<Vec<NearFieldLayerWork<'a>>> {
    // 每个外层元素将产生恰好一个Task和一份局部Vec。
    let mut ordered = work.to_vec(); // 复制轻量只读工作描述，不复制层配置或空间索引。
    ordered.sort_by(|left, right| {
        // 重工作优先分配，估算相同则按层ID保持确定性。
        right // 先比较右侧工作量形成降序。
            .estimated_checks // 读取右侧候选检查上界。
            .cmp(&left.estimated_checks) // 大检查量排在前面。
            .then_with(|| left.layer.id.cmp(&right.layer.id)) // 相同工作量按稳定层ID升序。
    }); // 完成确定性负载排序。
    let mut batches = (0..workers).map(|_| Vec::new()).collect::<Vec<_>>(); // 只创建受限数量非空候选批次槽。
    for (index, layer_work) in ordered.into_iter().enumerate() {
        // 依工作量降序轮转分配全部层。
        batches[index % workers].push(layer_work); // 最重的前workers层必定落到不同任务。
    } // 完成全部层有界负载分配。
    batches // workers不超过层数，因此每个批次至少包含一层。
} // 完成有界并行批次规划。

/// 根据预估候选检查量和池容量选择串行或有界并行。
pub(super) fn execution_mode(
    // 纯函数使门槛不依赖实际任务完成时间或机器抖动。
    work: &[NearFieldLayerWork<'_>], // 当前全部近场层工作估算。
    pool_threads: usize,             // 已初始化ComputeTaskPool可用线程数。
) -> NearFieldExecutionMode {
    // 返回本帧固定执行策略。
    let heavy_layers = work // 至少两个独立重层才能从按层并行中获益。
        .iter() // 逐层读取预估检查数。
        .filter(|item| item.estimated_checks >= MIN_PARALLEL_LAYER_CHECKS) // 轻层不计入可并行重分片。
        .count(); // 得到独立重层数量。
    let total_checks = work.iter().fold(0usize, |total, item| {
        // 饱和累计避免异常规模回绕成轻任务。
        total.saturating_add(item.estimated_checks) // 累加全部层粗筛候选上界。
    }); // 完成总工作量估算。
    if pool_threads <= 1 // 单线程池无法获得并行收益。
        || heavy_layers < 2 // 单一重层按本方案不拆内层循环。
        || total_checks < MIN_PARALLEL_TOTAL_CHECKS
    // 总量不足时任务调度可能更慢。
    {
        // 任一门槛不满足都保持当前线程串行。
        return NearFieldExecutionMode::Serial; // 明确避免轻负载频繁派发任务。
    } // 完成并行门槛快速拒绝。
    let workers = pool_threads // 从真实池容量推导有界worker数。
        .div_ceil(2) // 最多占约半池，为Bevy调度和其他计算保留容量。
        .clamp(1, MAX_PARALLEL_WORKERS) // 近场绝不超过固定四worker上限。
        .min(work.len()); // worker不超过实际层工作项数量。
    if workers < 2 {
        // 半池策略只得到一个worker时并行没有意义。
        NearFieldExecutionMode::Serial // 保持直接串行避免scope开销。
    } else {
        // 至少两个有界worker可以同时处理独立层。
        NearFieldExecutionMode::Parallel { workers } // 返回当帧同步并行策略。
    } // 完成最终worker容量判断。
} // 完成近场执行策略选择。

fn estimate_layer_checks(
    // 粗筛估算不执行密度采样或精确距离判断。
    layer: &VegetationLayerConfig, // 当前层候选源。
    explicit_index: Option<&ExplicitPlacementSpatialIndex>, // 显式源对应的附近桶索引。
    observers: &[Vec2],            // 当前全部观察者位置。
    exit_radius: f32,              // 当前层最大查询半径。
) -> usize {
    // 返回候选检查上界而非最终活动数量。
    match &layer.source { // 不同候选源使用各自天然空间结构估算。
        VegetationCandidateSource::ExplicitPlacements(placements) => explicit_index // 正常路径始终存在同配置代索引。
            .map(|index| index.estimated_candidate_checks(observers, exit_radius)) // 只累计附近桶长度。
            .unwrap_or(placements.len()), // 防御性失配回退与完整扫描工作量一致。
        VegetationCandidateSource::DenseGrid { spacing, .. } // 两类程序网格使用相同整数窗口大小。
        | VegetationCandidateSource::JitteredGrid { spacing, .. } => { // jitter不会改变需枚举的保守格范围。
            let cell_radius = ((exit_radius / *spacing).ceil() as usize).saturating_add(1); // 保持原实现额外一格且防异常比例回绕。
            let diameter = cell_radius.saturating_mul(2).saturating_add(1); // 得到单观察者方形格窗口边长。
            diameter // 从单轴长度开始饱和计算二维格数。
                .saturating_mul(diameter) // 得到单观察者候选格上界。
                .saturating_mul(observers.len()) // 多观察者重复格保守计入调度工作量。
        } // 完成程序网格工作量估算。
    } // 完成候选源分派。
} // 完成单层粗筛工作估算。

fn collect_layer_candidates(
    // 一层纯计算只返回局部Vec，不写共享集合或ECS。
    work: &NearFieldLayerWork<'_>,   // 当前层配置、空间索引和估算。
    observers: &[Vec2],              // 当前全部观察者位置。
    layout: &ActiveVegetationLayout, // 当前活动密度布局。
    active: &BTreeMap<(VegetationLayerId, u64), ActiveNearFieldProxy>, // 上一活动集合决定滞回半径。
) -> Vec<NearFieldEntry> {
    // 返回当前层稳定排序去重结果。
    let VegetationRealization::HybridNearField {
        // 工作列表保证该匹配必定成功。
        enter_radius, // 新候选必须进入内圈。
        exit_radius,  // 已活动候选可保留到外圈。
    } = work.layer.realization
    else {
        // 防御未来工作组装规则漂移。
        return Vec::new(); // 非近场层绝不产生代理。
    }; // 完成当前层滞回半径提取。
    let mut result = Vec::new(); // 当前层独占局部连续结果。
    let mut context = NearFieldCandidateContext {
        // 候选源枚举共享相同只读精筛上下文。
        layer: work.layer,   // 当前层稳定ID和原型。
        observers,           // 当前观察者集合。
        enter_radius,        // 当前层进入半径。
        exit_radius,         // 当前层退出半径。
        layout,              // 当前完整活动布局。
        active,              // 上一活动代理集合。
        result: &mut result, // 当前层独占输出Vec。
    }; // 完成精筛上下文构造。
    match &work.layer.source { // 显式清单使用空间索引，程序源继续使用附近世界格。
        VegetationCandidateSource::ExplicitPlacements(placements) => { // 静态落点先通过均匀网格粗筛。
            let placement_indices = work // 正常配置代路径使用附近桶下标。
                .explicit_index // 取得当前层只读空间索引。
                .map(|index| index.query_indices(observers, exit_radius)) // 查询外圈矩形并在下标层去重。
                .unwrap_or_else(|| (0..placements.len()).collect()); // 防御索引失配时宁可保持旧正确行为而不漏碰撞。
            for placement_index in placement_indices { // 只把附近原清单项恢复为公共确定性候选。
                if let Some(candidate) = deterministic_candidate( // 沿用显式落点密度钳制与完整Transform语义。
                    &work.layer.source, // 当前显式候选源。
                    IVec2::new(placement_index as i32, 0), // 公共生成器以非负X下标映射清单项。
                ) { // 有效usize下标和受检清单保证通常为Some。
                    context.consider(candidate); // 执行原有滞回距离与活动布局精筛。
                } // 完成当前显式落点候选恢复。
            } // 完成附近显式落点精筛。
        } // 完成显式候选源处理。
        VegetationCandidateSource::DenseGrid { spacing, .. } // 规则网格按观察者附近整数单元生成。
        | VegetationCandidateSource::JitteredGrid { spacing, .. } => { // 抖动网格使用相同保守单元窗口。
            let cell_radius = ((exit_radius / *spacing).ceil() as i32).saturating_add(1); // 保持原外圈覆盖并防异常比例回绕。
            for observer in observers { // 每个观察者分别生成自己的程序候选窗口。
                let center_cell = (*observer / *spacing).floor().as_ivec2(); // 负坐标继续使用floor世界格。
                for dz in -cell_radius..=cell_radius { // Z轴遍历完整闭区间。
                    for dx in -cell_radius..=cell_radius { // X轴遍历完整闭区间。
                        let cell = center_cell + IVec2::new(dx, dz); // 恢复当前绝对整数世界格。
                        if let Some(candidate) = deterministic_candidate(&work.layer.source, cell) { // 同种子生成稳定ID和Transform。
                            context.consider(candidate); // 多观察者重复候选稍后按联合主键去重。
                        } // 完成当前程序世界格候选生成。
                    } // 完成当前Z行全部X格。
                } // 完成当前观察者全部Z行。
            } // 完成全部观察者程序候选窗口。
        } // 完成程序网格候选源处理。
    } // 完成当前层候选源分派。
    result.sort_by_key(|entry| entry.0); // 观察者顺序和程序窗口重叠不影响局部结果顺序。
    result.dedup_by(|left, right| left.0 == right.0); // 同层稳定候选只保留一个代理描述。
    result // 返回当前层独占稳定局部结果。
} // 完成单层候选收集。

/// 单层候选枚举共享的只读精筛输入和独占局部输出。
struct NearFieldCandidateContext<'a> {
    // 该纯上下文可安全存在于任一ComputeTaskPool worker。
    layer: &'a VegetationLayerConfig,   // 当前层稳定ID和原型。
    observers: &'a [Vec2],              // 当前全部观察者位置。
    enter_radius: f32,                  // 非活动候选进入阈值。
    exit_radius: f32,                   // 已活动候选退出阈值。
    layout: &'a ActiveVegetationLayout, // 当前活动密度布局只读快照。
    active: &'a BTreeMap<(VegetationLayerId, u64), ActiveNearFieldProxy>, // 上一活动集合只读快照。
    result: &'a mut Vec<NearFieldEntry>, // 当前layer或worker独占局部输出。
} // 完成单层精筛上下文定义。

impl NearFieldCandidateContext<'_> {
    // 所有候选源统一经过完全相同的最终语义判断。
    fn consider(&mut self, candidate: VegetationCandidate) {
        // 候选按值传入避免保存临时生成引用。
        let key = (self.layer.id, candidate.instance_id); // 不同层可以复用相同世界格哈希或显式ID。
        let candidate_xz = candidate.transform.translation.xz(); // 距离和布局密度均在世界X/Z。
        let was_active = self.active.contains_key(&key); // 已进入代理使用更大退出半径。
        let radius = if was_active {
            // 滞回状态只读取上一完整活动集合。
            self.exit_radius // 已活动代理越过外圈才回收。
        } else {
            // 尚未活动代理必须真正进入内圈。
            self.enter_radius // 新代理使用更小进入半径。
        }; // 完成当前候选精确半径选择。
        if !self // 任一观察者满足精确圆形距离即可保留候选。
            .observers // 遍历当前稳定观察者位置切片。
            .iter() // 只读计算平方距离避免开方。
            .any(|observer| observer.distance_squared(candidate_xz) <= radius * radius)
        // 保持原闭区间边界语义。
        {
            // 候选不在任何观察者实际滞回圆内。
            return; // 粗筛正方形四角候选在此被准确剔除。
        } // 完成精确距离门。
        if !crate::resolved_candidate_enabled(self.layout, self.layer.id, &candidate) {
            // 所有后端共享当前活动密度选择。
            return; // 未被活动布局选中的候选不发布近场代理。
        } // 完成活动布局启用门。
        self.result.push((
            // 只写当前layer或worker独占Vec，不发生跨线程锁竞争。
            key, // 保存稳定联合主键供最终排序合并。
            ActiveNearFieldProxy {
                // 保存消费者需要的当前完整代理描述。
                prototype: self.layer.prototype, // 同一机制支持任意内容原型。
                transform: candidate.transform,  // 近场代理和远场继续共享最终候选变换。
            }, // 完成活动代理描述构造。
        )); // 完成当前通过精筛候选登记。
    } // 完成统一候选精筛。
} // 完成精筛上下文实现。
