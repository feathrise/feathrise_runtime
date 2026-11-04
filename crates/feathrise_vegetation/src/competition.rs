//! 竞争模式解析与配置图验证。

use std::collections::{BTreeMap, BTreeSet}; // 确定性排序保证错误链和布局结果跨运行稳定。
use std::error::Error; // 公共配置错误实现标准错误接口。
use std::fmt; // 诊断信息需要包含完整层或组循环链。

use crate::{
    VegetationCompetitionGroup, VegetationCompetitionGroupId, VegetationCompetitionMode,
    VegetationLayerId, VegetationWeight,
}; // 单点解析器只读取竞争组、层身份、密度、优先级和受检权重。

mod validation; // 配置、候选、足迹、实现方式与有向图验证独立于数值分配算法。

pub(crate) use validation::validate_configuration; // 布局核心在写入状态前执行完整原子验证。
use validation::{validate_demands, validate_directed_graph, validate_group}; // 公共单点解析和关系式辅助器复用同一验证规则。

const DENSITY_EPSILON: f32 = 1.0e-6; // 容量水填充分配的浮点终止阈值。

/// 单个布局采样点上一层尚未解析的归一化需求。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VegetationLayerDemand {
    pub layer: VegetationLayerId, // 输出与输入按稳定层ID关联。
    pub density: f32,             // 零至一表示本层候选容量需求。
    pub claim_density: f32,       // 零至一表示优先覆盖所有权，不会扩大被选中层的实际输出。
    pub priority: i32,            // 数值越大越先占用组容量。
    pub weight: VegetationWeight, // 同优先级需求的相对竞争权重。
}

/// 单个布局采样点上一层解析后的归一化份额。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VegetationLayerShare {
    pub layer: VegetationLayerId, // 保留层ID，不把层编码进渲染后端实例ABI。
    pub density: f32,             // 后端直接读取，不再遍历其他层。
}

/// 所有配置拒绝原因；错误发生时活动布局保持不变。
#[derive(Clone, Debug, PartialEq)]
pub enum VegetationConfigError {
    DuplicateLayer(VegetationLayerId), // 同一稳定层ID不得出现两次。
    DuplicateGroup(VegetationCompetitionGroupId), // 同一竞争组ID不得出现两次。
    MissingGroup {
        layer: VegetationLayerId,            // 报出引用缺失组的具体层。
        group: VegetationCompetitionGroupId, // 缺失的竞争或排斥组。
    },
    InvalidValue {
        context: String, // 精确指出字段和关联层或组。
    },
    PriorityCycle {
        chain: Vec<VegetationLayerId>, // 首尾相同的完整关系式循环链。
    },
    ExclusionCycle {
        chain: Vec<VegetationCompetitionGroupId>, // 首尾相同的完整跨组排斥循环链。
    },
}

impl fmt::Display for VegetationConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateLayer(layer) => {
                write!(formatter, "duplicate vegetation layer {layer:?}")
            }
            Self::DuplicateGroup(group) => {
                write!(
                    formatter,
                    "duplicate vegetation competition group {group:?}"
                )
            }
            Self::MissingGroup { layer, group } => {
                write!(
                    formatter,
                    "vegetation layer {layer:?} references missing group {group:?}"
                )
            }
            Self::InvalidValue { context } => formatter.write_str(context),
            Self::PriorityCycle { chain } => {
                write!(formatter, "vegetation priority cycle: {chain:?}")
            }
            Self::ExclusionCycle { chain } => {
                write!(formatter, "vegetation exclusion cycle: {chain:?}")
            }
        }
    }
}

impl Error for VegetationConfigError {} // 消费项目可用任意标准错误聚合库传播配置失败。

/// 解析一个竞争组在单个世界采样点上的所有层需求。
pub fn resolve_competition(
    group: VegetationCompetitionGroup,
    demands: &[VegetationLayerDemand],
    selector: f32,
) -> Result<Vec<VegetationLayerShare>, VegetationConfigError> {
    validate_group(group)?; // 单点解析也必须拒绝非法容量、阈值或世界选择网格。
    validate_demands(demands)?; // 防止非有限密度或重复层让分配结果不确定。
    let mut ordered = demands.to_vec(); // 后续排序不得改写消费者原切片。
    ordered.sort_by_key(|demand| demand.layer); // 所有平局使用稳定层ID顺序解决。
    match group.mode {
        VegetationCompetitionMode::Additive => Ok(ordered
            .into_iter()
            .map(|demand| VegetationLayerShare {
                layer: demand.layer,                     // 叠加模式只钳制需求，不改变层身份。
                density: demand.density.clamp(0.0, 1.0), // 每层100%仍代表自身全部候选。
            })
            .collect()),
        VegetationCompetitionMode::WeightedPartition { capacity } => {
            resolve_weighted_partition(ordered, capacity)
        }
        VegetationCompetitionMode::PriorityOverride { claim_threshold } => {
            resolve_priority_override(ordered, claim_threshold, selector)
        }
    }
}

fn resolve_weighted_partition(
    demands: Vec<VegetationLayerDemand>,
    capacity: f32,
) -> Result<Vec<VegetationLayerShare>, VegetationConfigError> {
    let mut shares: BTreeMap<VegetationLayerId, f32> =
        demands.iter().map(|demand| (demand.layer, 0.0)).collect(); // 即使容量已耗尽也为每个输入层返回显式零。
    let mut tiers = BTreeMap::<i32, Vec<VegetationLayerDemand>>::new(); // 优先级层级先高后低消费容量。
    for demand in demands {
        tiers.entry(demand.priority).or_default().push(demand); // 同级层稍后执行加权水填充。
    }
    let mut remaining = capacity; // 低优先级只能使用高优先级未消费的剩余容量。
    for (_, tier) in tiers.into_iter().rev() {
        if remaining <= DENSITY_EPSILON {
            break; // 容量耗尽后保持剩余层显式零份额。
        }
        let tier_demand = tier.iter().map(|demand| demand.density).sum::<f32>(); // 不足需求不能自动放大。
        let tier_capacity = remaining.min(tier_demand); // 当前优先级最多得到自身总需求。
        let allocation = weighted_capped_allocation(&tier, tier_capacity); // 权重只在容量不足时决定相对份额。
        let consumed = allocation.iter().map(|(_, density)| density).sum::<f32>(); // 实际消耗可能因零需求低于上限。
        for (layer, density) in allocation {
            shares.insert(layer, density); // 写入当前优先级的最终解析份额。
        }
        remaining = (remaining - consumed).max(0.0); // 向下一优先级传递未用容量。
    }
    Ok(shares
        .into_iter()
        .map(|(layer, density)| VegetationLayerShare { layer, density })
        .collect()) // BTreeMap保证输出层顺序跨运行稳定。
}

fn weighted_capped_allocation(
    demands: &[VegetationLayerDemand],
    capacity: f32,
) -> Vec<(VegetationLayerId, f32)> {
    let total_demand = demands.iter().map(|demand| demand.density).sum::<f32>(); // 先识别无需竞争的不足容量情况。
    if total_demand <= capacity + DENSITY_EPSILON {
        return demands
            .iter()
            .map(|demand| (demand.layer, demand.density))
            .collect(); // 总需求不足时严格保持原密度，不因权重放大。
    }
    let mut result = BTreeMap::<VegetationLayerId, f32>::new(); // 每轮锁定已经达到自身需求上限的层。
    let mut active = demands.iter().collect::<Vec<_>>(); // 尚待分配的同级需求。
    let mut remaining = capacity; // 水填充把未用份额重新分给仍有需求的层。
    while !active.is_empty() && remaining > DENSITY_EPSILON {
        let weight_sum = active.iter().map(|demand| demand.weight.get()).sum::<f32>(); // 权重构造器保证总和有限且为正。
        let mut capped_layers = Vec::new(); // 本轮按权重份额超过自身需求的层先封顶。
        for demand in &active {
            let proposed = remaining * demand.weight.get() / weight_sum; // 容量紧张时1:3直接得到25/75。
            if proposed >= demand.density - DENSITY_EPSILON {
                capped_layers.push(demand.layer); // 该层无法消费其权重份额，余量需重分。
            }
        }
        if capped_layers.is_empty() {
            for demand in &active {
                let density = remaining * demand.weight.get() / weight_sum; // 所有剩余层均未触顶，本轮完成。
                result.insert(demand.layer, density.min(demand.density)); // 浮点保护不越过层自身需求。
            }
            remaining = 0.0; // 显式结束水填充。
        } else {
            active.retain(|demand| {
                if capped_layers.contains(&demand.layer) {
                    result.insert(demand.layer, demand.density); // 先满足小需求层。
                    remaining = (remaining - demand.density).max(0.0); // 把剩余容量留给其他层。
                    false // 已封顶层退出后续轮次。
                } else {
                    true // 仍有容量需求的层继续参与权重分配。
                }
            });
        }
    }
    for demand in demands {
        result.entry(demand.layer).or_insert(0.0); // 数值极小容量下也返回完整层集合。
    }
    result.into_iter().collect() // 层ID稳定排序供布局逐字节比较。
}

fn resolve_priority_override(
    demands: Vec<VegetationLayerDemand>,
    claim_threshold: f32,
    selector: f32,
) -> Result<Vec<VegetationLayerShare>, VegetationConfigError> {
    if !selector.is_finite() {
        return Err(VegetationConfigError::InvalidValue {
            context: "priority override selector must be finite".to_owned(),
        }); // 世界哈希失效时不能静默选择某个层。
    }
    let mut shares = demands
        .iter()
        .map(|demand| VegetationLayerShare {
            layer: demand.layer, // 先为全部输入层建立显式零输出。
            density: 0.0,
        })
        .collect::<Vec<_>>();
    let Some(highest_priority) = demands
        .iter()
        .filter(|demand| demand.claim_density > claim_threshold)
        .map(|demand| demand.priority)
        .max()
    else {
        return Ok(shares); // 没有层超过声明阈值时该组留空。
    };
    let contenders = demands
        .iter()
        .filter(|demand| {
            demand.priority == highest_priority && demand.claim_density > claim_threshold
        })
        .collect::<Vec<_>>(); // 较低优先级无论密度多高都不能参与同级选择。
    let score_sum = contenders
        .iter()
        .map(|demand| demand.claim_density * demand.weight.get())
        .sum::<f32>(); // 声明强度和权重共同决定同级空间归属比例。
    let target = selector.rem_euclid(1.0) * score_sum; // 任意有限选择值都映射到固定零至一周期。
    let mut cursor = 0.0; // 稳定层ID排序后的累计分布边界。
    let mut chosen = contenders.last().copied(); // 浮点尾差回退到最后一个合法竞争者。
    for contender in contenders {
        cursor += contender.claim_density * contender.weight.get(); // 累计所有权权重区间。
        if target < cursor {
            chosen = Some(contender); // 固定世界哈希命中当前层。
            break;
        }
    }
    if let Some(chosen) = chosen
        && let Some(share) = shares.iter_mut().find(|share| share.layer == chosen.layer)
    {
        share.density = chosen.density; // 被选中层保留自己的归一化候选需求。
    }
    Ok(shares)
}

/// 关系式优先级辅助器；数值优先级主接口不需要使用本类型。
#[derive(Clone, Debug, Default)]
pub struct PriorityConstraintBuilder {
    edges: BTreeMap<VegetationLayerId, BTreeSet<VegetationLayerId>>, // 高层指向被其压过的低层。
}

impl PriorityConstraintBuilder {
    /// 声明 `higher` 的优先级高于 `lower`。
    pub fn higher_than(
        &mut self,
        higher: VegetationLayerId,
        lower: VegetationLayerId,
    ) -> &mut Self {
        self.edges.entry(higher).or_default().insert(lower); // 重复声明自动去重。
        self.edges.entry(lower).or_default(); // 没有出边的低层也进入完整图。
        self // 允许项目侧链式构造关系。
    }

    /// 验证关系图并返回由零开始、数值越大越高的优先级。
    pub fn build(&self) -> Result<BTreeMap<VegetationLayerId, i32>, VegetationConfigError> {
        validate_directed_graph(&self.edges)
            .map_err(|chain| VegetationConfigError::PriorityCycle { chain })?; // 循环必须携带完整层ID链。
        let mut indegree = self
            .edges
            .keys()
            .map(|layer| (*layer, 0_usize))
            .collect::<BTreeMap<_, _>>(); // Kahn排序的入度表按层ID稳定排序。
        for targets in self.edges.values() {
            for target in targets {
                *indegree.entry(*target).or_default() += 1; // 高到低的每条边增加低层入度。
            }
        }
        let mut ready = indegree
            .iter()
            .filter_map(|(layer, degree)| (*degree == 0).then_some(*layer))
            .collect::<BTreeSet<_>>(); // 同时就绪时总是先取最小稳定层ID。
        let mut order = Vec::with_capacity(indegree.len()); // 先得到从最高到最低的拓扑序。
        while let Some(layer) = ready.pop_first() {
            order.push(layer); // 当前无更高前置的层加入序列。
            if let Some(targets) = self.edges.get(&layer) {
                for target in targets {
                    let degree = indegree.get_mut(target).expect("graph target must exist");
                    *degree -= 1; // 移除当前高层边。
                    if *degree == 0 {
                        ready.insert(*target); // 所有更高层已处理后才释放低层。
                    }
                }
            }
        }
        let max_priority = order.len().saturating_sub(1) as i32; // 首个拓扑层获得最高数值。
        Ok(order
            .into_iter()
            .enumerate()
            .map(|(index, layer)| (layer, max_priority - index as i32))
            .collect()) // 结果遵守公共“数值越大越高”契约。
    }
}

#[cfg(test)]
mod tests; // 竞争语义与错误链回归单独存放，生产分配实现保持紧凑。
