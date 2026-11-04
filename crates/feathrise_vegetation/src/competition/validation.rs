//! 竞争组、正交层配置和有向关系图的原子验证。

use std::collections::{BTreeMap, BTreeSet}; // 稳定集合保证重复项和循环链诊断跨运行一致。

use crate::{
    VegetationCandidateSource, VegetationCompetitionGroup, VegetationCompetitionMode,
    VegetationFootprintShape, VegetationLayerConfig, VegetationLayerId, VegetationRealization,
}; // 验证器读取所有可能影响布局安全的公开配置字段。

use super::{VegetationConfigError, VegetationLayerDemand}; // 错误与单点需求类型由竞争公共模块定义。

pub(crate) fn validate_configuration(
    groups: &[VegetationCompetitionGroup],
    layers: &[VegetationLayerConfig],
) -> Result<(), VegetationConfigError> {
    let mut group_map = BTreeMap::new(); // 同时检查重复组并供层引用查询。
    for group in groups {
        validate_group(*group)?; // 每个组独立数值必须合法。
        if group_map.insert(group.id, *group).is_some() {
            return Err(VegetationConfigError::DuplicateGroup(group.id)); // 重复ID会让模式选择不确定。
        }
    }
    let mut layer_ids = BTreeSet::new(); // 不允许两个配置共享一个稳定层身份。
    let mut exclusion_edges = group_map
        .keys()
        .map(|group| (*group, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>(); // 全部组进入排斥图，包含无出边节点。
    for layer in layers {
        if !layer_ids.insert(layer.id) {
            return Err(VegetationConfigError::DuplicateLayer(layer.id)); // 原子拒绝重复层。
        }
        validate_layer(layer)?; // 验证候选、密度、足迹和实现参数。
        if !group_map.contains_key(&layer.competition.group) {
            return Err(VegetationConfigError::MissingGroup {
                layer: layer.id,
                group: layer.competition.group,
            });
        }
        for target in &layer.competition.excludes_groups {
            if !group_map.contains_key(target) {
                return Err(VegetationConfigError::MissingGroup {
                    layer: layer.id,
                    group: *target,
                });
            }
            exclusion_edges
                .entry(layer.competition.group)
                .or_default()
                .insert(*target); // 任意层输出的跨组净空都进入全局有向图。
        }
    }
    validate_directed_graph(&exclusion_edges)
        .map_err(|chain| VegetationConfigError::ExclusionCycle { chain })?; // 循环排斥没有稳定解析顺序。
    Ok(())
}

pub(super) fn validate_group(
    group: VegetationCompetitionGroup,
) -> Result<(), VegetationConfigError> {
    if !group.selection_cell_size.is_finite() || group.selection_cell_size <= 0.0 {
        return Err(invalid(format!(
            "competition group {:?} selection_cell_size must be finite and positive",
            group.id
        )));
    }
    match group.mode {
        VegetationCompetitionMode::Additive => {}
        VegetationCompetitionMode::WeightedPartition { capacity } => {
            if !capacity.is_finite() || capacity < 0.0 {
                return Err(invalid(format!(
                    "competition group {:?} capacity must be finite and non-negative",
                    group.id
                )));
            }
        }
        VegetationCompetitionMode::PriorityOverride { claim_threshold } => {
            if !claim_threshold.is_finite() || !(0.0..=1.0).contains(&claim_threshold) {
                return Err(invalid(format!(
                    "competition group {:?} claim_threshold must be within 0..=1",
                    group.id
                )));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_demands(
    demands: &[VegetationLayerDemand],
) -> Result<(), VegetationConfigError> {
    let mut ids = BTreeSet::new(); // 单点输入同样必须保证唯一层身份。
    for demand in demands {
        if !ids.insert(demand.layer) {
            return Err(VegetationConfigError::DuplicateLayer(demand.layer));
        }
        if !demand.density.is_finite() || !(0.0..=1.0).contains(&demand.density) {
            return Err(invalid(format!(
                "vegetation layer {:?} density demand must be within 0..=1",
                demand.layer
            )));
        }
        if !demand.claim_density.is_finite() || !(0.0..=1.0).contains(&demand.claim_density) {
            return Err(invalid(format!(
                "vegetation layer {:?} claim density demand must be within 0..=1",
                demand.layer
            )));
        }
    }
    Ok(())
}

fn validate_layer(layer: &VegetationLayerConfig) -> Result<(), VegetationConfigError> {
    if !layer.density.gain.is_finite() || layer.density.gain < 0.0 {
        return Err(invalid(format!(
            "vegetation layer {:?} density gain must be finite and non-negative",
            layer.id
        )));
    }
    if !layer.density.world_bounds.min.is_finite()
        || !layer.density.world_bounds.max.is_finite()
        || layer.density.world_bounds.width() <= 0.0
        || layer.density.world_bounds.height() <= 0.0
    {
        return Err(invalid(format!(
            "vegetation layer {:?} density world bounds must be finite and non-empty",
            layer.id
        )));
    }
    match &layer.source {
        VegetationCandidateSource::DenseGrid { spacing, .. } => {
            validate_spacing(layer.id, *spacing)?;
        }
        VegetationCandidateSource::JitteredGrid {
            spacing, jitter, ..
        } => {
            validate_spacing(layer.id, *spacing)?;
            if !jitter.is_finite() || !(0.0..=0.5).contains(jitter) {
                return Err(invalid(format!(
                    "vegetation layer {:?} jitter must be within 0..=0.5",
                    layer.id
                )));
            }
        }
        VegetationCandidateSource::ExplicitPlacements(placements) => {
            let mut instance_ids = BTreeSet::new(); // 同一层显式实例ID必须唯一。
            for placement in placements {
                if !instance_ids.insert(placement.instance_id) {
                    return Err(invalid(format!(
                        "vegetation layer {:?} has duplicate explicit instance {}",
                        layer.id, placement.instance_id
                    )));
                }
                if !placement.density.is_finite()
                    || !(0.0..=1.0).contains(&placement.density)
                    || !placement.transform.translation.is_finite()
                    || !placement.transform.scale.is_finite()
                    || !placement.transform.rotation.is_finite()
                {
                    return Err(invalid(format!(
                        "vegetation layer {:?} has an invalid explicit placement",
                        layer.id
                    )));
                }
            }
        }
    }
    validate_footprint(layer)?;
    match layer.realization {
        VegetationRealization::GpuOnly | VegetationRealization::Entities => {}
        VegetationRealization::HybridNearField {
            enter_radius,
            exit_radius,
        } => {
            if !enter_radius.is_finite()
                || !exit_radius.is_finite()
                || enter_radius < 0.0
                || exit_radius <= enter_radius
            {
                return Err(invalid(format!(
                    "vegetation layer {:?} hybrid radii require 0 <= enter < exit",
                    layer.id
                )));
            }
        }
    }
    Ok(())
}

fn validate_spacing(layer: VegetationLayerId, spacing: f32) -> Result<(), VegetationConfigError> {
    if spacing.is_finite() && spacing > 0.0 {
        Ok(())
    } else {
        Err(invalid(format!(
            "vegetation layer {layer:?} candidate spacing must be finite and positive"
        )))
    }
}

fn validate_footprint(layer: &VegetationLayerConfig) -> Result<(), VegetationConfigError> {
    let dimensions_valid = match layer.footprint.shape {
        VegetationFootprintShape::Circle { radius } => radius.is_finite() && radius >= 0.0,
        VegetationFootprintShape::Ellipse { half_extents }
        | VegetationFootprintShape::Rectangle { half_extents }
        | VegetationFootprintShape::MeshAabb { half_extents } => {
            half_extents.is_finite() && half_extents.min_element() >= 0.0
        }
    };
    if !dimensions_valid
        || !layer.footprint.exclusion_margin.is_finite()
        || layer.footprint.exclusion_margin < 0.0
        || !layer.footprint.recovery_width.is_finite()
        || layer.footprint.recovery_width < 0.0
    {
        return Err(invalid(format!(
            "vegetation layer {:?} footprint dimensions, margin and recovery must be finite and non-negative",
            layer.id
        )));
    }
    Ok(())
}

fn invalid(context: String) -> VegetationConfigError {
    VegetationConfigError::InvalidValue { context } // 集中构造便于所有验证分支保持一致错误形态。
}

pub(super) fn validate_directed_graph<Node>(
    graph: &BTreeMap<Node, BTreeSet<Node>>,
) -> Result<(), Vec<Node>>
where
    Node: Copy + Ord,
{
    fn visit<Node>(
        node: Node,
        graph: &BTreeMap<Node, BTreeSet<Node>>,
        states: &mut BTreeMap<Node, u8>,
        stack: &mut Vec<Node>,
    ) -> Result<(), Vec<Node>>
    where
        Node: Copy + Ord,
    {
        states.insert(node, 1); // 一表示节点位于当前DFS递归栈。
        stack.push(node); // 栈顺序用于还原完整循环链。
        if let Some(targets) = graph.get(&node) {
            for target in targets {
                match states.get(target).copied().unwrap_or(0) {
                    0 => visit(*target, graph, states, stack)?, // 未访问节点递归探索。
                    1 => {
                        let start = stack.iter().position(|entry| entry == target).unwrap_or(0);
                        let mut chain = stack[start..].to_vec(); // 从首次出现目标截取真实循环。
                        chain.push(*target); // 首尾相同明确闭合关系。
                        return Err(chain);
                    }
                    _ => {} // 已完成节点不会形成指向当前栈的循环。
                }
            }
        }
        stack.pop(); // 当前分支完成后离开递归栈。
        states.insert(node, 2); // 二表示节点及其后继均已验证。
        Ok(())
    }
    let mut states = BTreeMap::new(); // 节点状态按稳定顺序遍历。
    let mut stack = Vec::new(); // 单次DFS复用循环链栈。
    for node in graph.keys() {
        if states.get(node).copied().unwrap_or(0) == 0 {
            visit(*node, graph, &mut states, &mut stack)?;
        }
    }
    Ok(())
}
