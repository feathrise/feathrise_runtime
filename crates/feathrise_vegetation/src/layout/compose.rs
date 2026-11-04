//! 单个固定脏块的基础密度、竞争和跨组足迹合成。

use std::collections::BTreeMap; // 组和层结果使用稳定顺序。

use bevy::prelude::{IVec2, Rect, Transform, Vec2, Vec3Swizzles}; // 世界纹素中心和实例足迹都位于X/Z平面。

use crate::core_types::VegetationWorldCell;
use crate::{
    VegetationCandidateSource, VegetationCompetitionGroup, VegetationCompetitionGroupId,
    VegetationConfigError, VegetationFootprint, VegetationFootprintScale, VegetationLayerConfig,
    VegetationLayerDemand, VegetationLayerId, VegetationResolvedLayer, VegetationResolvedTile,
    VegetationSourceField, deterministic_candidate, footprint_influence, resolve_competition,
    stable_instance_selector, stable_world_selector,
}; // 合成器只组合公共纯函数，不访问ECS或渲染资源。 // 内部整数世界格不进入公共API。

use super::resources::{VEGETATION_TILE_SIDE, VEGETATION_TILE_TEXEL_COUNT, VegetationDirtyTile}; // 所有暂存块固定为64×64纹素。

#[cfg(feature = "blade")] // 无叶片后端的纯布局核心不编译GPU冷路径编码或产生无用代码警告。
mod input; // GPU冷路径输入编码复用本模块的正式源采样与稀疏足迹准备，但不调用CPU竞争解析。

#[cfg(feature = "blade")] // 只有叶片后端需要跨模块取得未决竞争输入。
pub(crate) use input::{VegetationCompetitionInputLayer, encode_competition_tile_inputs}; // 叶片后端只取得未决竞争输入，活动布局仍是唯一CPU回读权威。

pub(crate) fn compose_tile(
    tile: VegetationDirtyTile,
    texel_size: f32,
    groups: &[VegetationCompetitionGroup],
    layers: &[VegetationLayerConfig],
    sources: &BTreeMap<VegetationLayerId, VegetationSourceField>,
) -> Result<VegetationResolvedTile, VegetationConfigError> {
    let tile_world_size = VEGETATION_TILE_SIDE as f32 * texel_size; // 一个块覆盖固定64个当前最高精度纹素。
    let world_min = tile.0.as_vec2() * tile_world_size; // 世界原点对齐保证局部重算与整图一致。
    let world_bounds = Rect::from_corners(world_min, world_min + Vec2::splat(tile_world_size)); // 闭开块范围。
    let mut layer_values = layers
        .iter()
        .map(|layer| (layer.id, vec![0.0; VEGETATION_TILE_TEXEL_COUNT]))
        .collect::<BTreeMap<_, _>>(); // 每层先建立完整零块。
    let group_map = groups
        .iter()
        .map(|group| (group.id, *group))
        .collect::<BTreeMap<_, _>>(); // 配置验证已保证组ID唯一。
    let layers_by_group = group_layers(layers); // 避免每个纹素重复扫描无关组。
    let exclusion_footprints =
        prepare_exclusion_footprints(world_bounds, layers, sources, &group_map, &layers_by_group)?; // 每块只解析一次稀疏候选和组内启用状态，避免每纹素扫描全图稳定落点。
    for y in 0..VEGETATION_TILE_SIDE {
        for x in 0..VEGETATION_TILE_SIDE {
            let index = y * VEGETATION_TILE_SIDE + x; // 行优先索引供GPU上传。
            let world_xz =
                world_min + (Vec2::new(x as f32, y as f32) + Vec2::splat(0.5)) * texel_size; // 使用纹素中心采样。
            let mut base = BTreeMap::<VegetationLayerId, f32>::new(); // 先解析每个组内部竞争。
            for (group_id, group_layers) in &layers_by_group {
                let group = group_map
                    .get(group_id)
                    .ok_or(VegetationConfigError::MissingGroup {
                        layer: group_layers[0].id,
                        group: *group_id,
                    })?; // 独立调用也不允许缺失组。
                let demands = sample_demands(group_layers, sources, world_xz, texel_size)?; // 实际密度点采样，所有权可按输出单元相交采样。
                let selector = stable_world_selector(
                    world_xz,
                    group.selection_cell_size,
                    group.selection_seed,
                ); // 同级覆盖世界稳定。
                for share in resolve_competition(*group, &demands, selector)? {
                    base.insert(share.layer, share.density); // 组内解析结果先不应用跨组排斥。
                }
            }
            for target_layer in layers {
                let mut density = *base.get(&target_layer.id).unwrap_or(&0.0); // 空源或零需求保持零。
                if density > 0.0
                    && let Some(footprints) =
                        exclusion_footprints.get(&target_layer.competition.group)
                {
                    let influence = footprints
                        .iter()
                        .map(|prepared| {
                            footprint_influence(prepared.footprint, prepared.transform, world_xz)
                        })
                        .fold(1.0_f32, f32::min); // 多个占用轮廓按并集取最强排斥，重叠恢复带不会被重复相乘。
                    if influence < 1.0 {
                        density *= influence; // 硬净空得到零，软恢复保持平滑且不影响其他组。
                    }
                }
                layer_values
                    .get_mut(&target_layer.id)
                    .expect("all configured layers must have output storage")[index] = density; // 写入最终目标层。
            }
        }
    }
    Ok(VegetationResolvedTile {
        coordinate: tile, // 保留稀疏块身份。
        world_bounds,     // 后端按世界范围上传或采样。
        layers: layer_values
            .into_iter()
            .map(|(layer, density)| VegetationResolvedLayer { layer, density })
            .collect(), // BTreeMap保证稳定层顺序。
    })
}

fn group_layers(
    layers: &[VegetationLayerConfig],
) -> BTreeMap<VegetationCompetitionGroupId, Vec<&VegetationLayerConfig>> {
    let mut result = BTreeMap::<VegetationCompetitionGroupId, Vec<_>>::new(); // 每组独立解析容量。
    for layer in layers {
        result
            .entry(layer.competition.group)
            .or_default()
            .push(layer); // 层保留配置切片引用避免复制显式落点表。
    }
    for group_layers in result.values_mut() {
        group_layers.sort_by_key(|layer| layer.id); // 平局和输出始终按稳定层ID。
    }
    result
}

fn sample_demands(
    layers: &[&VegetationLayerConfig],
    sources: &BTreeMap<VegetationLayerId, VegetationSourceField>,
    world_xz: Vec2,
    claim_cell_size: f32,
) -> Result<Vec<VegetationLayerDemand>, VegetationConfigError> {
    layers
        .iter()
        .map(|layer| {
            let source =
                sources
                    .get(&layer.id)
                    .ok_or_else(|| VegetationConfigError::InvalidValue {
                        context: format!(
                            "vegetation layer {:?} source field is not ready",
                            layer.id
                        ),
                    })?;
            Ok(VegetationLayerDemand {
                layer: layer.id, // 竞争输出继续关联无限层身份。
                density: layer.density.apply_at(world_xz, source.sample(world_xz)), // 层世界范围与源图范围分别生效，100%表示候选容量全部启用。
                claim_density: layer.density.apply_at(
                    world_xz,
                    source.sample_claim(
                        world_xz,
                        layer.competition.claim_sampling,
                        claim_cell_size,
                    ),
                ), // 所有权可覆盖相交输出单元，但不会写入本层实际渲染密度。
                priority: layer.competition.priority, // 数值越大越先占组容量。
                weight: layer.competition.weight,     // 同优先级使用受检正权重。
            })
        })
        .collect()
}

#[derive(Clone, Copy)]
struct PreparedExclusionFootprint {
    footprint: VegetationFootprint, // 候选共享层定义的实际尺寸、边距和恢复带。
    transform: Transform,           // 显式或程序候选的世界变换只解析一次。
}

fn prepare_exclusion_footprints(
    tile_bounds: Rect,
    layers: &[VegetationLayerConfig],
    sources: &BTreeMap<VegetationLayerId, VegetationSourceField>,
    groups: &BTreeMap<VegetationCompetitionGroupId, VegetationCompetitionGroup>,
    layers_by_group: &BTreeMap<VegetationCompetitionGroupId, Vec<&VegetationLayerConfig>>,
) -> Result<
    BTreeMap<VegetationCompetitionGroupId, Vec<PreparedExclusionFootprint>>,
    VegetationConfigError,
> {
    let mut by_target_group =
        BTreeMap::<VegetationCompetitionGroupId, Vec<PreparedExclusionFootprint>>::new(); // 当前块只保存真正可能影响目标组的稀疏足迹。
    let excluders_by_target = exclusion_layers_by_target(layers); // 候选自身也要先经过上游排斥，DAG语义不能让已清除层继续输出足迹。
    let mut activation = CandidateActivationContext {
        sources,
        groups,
        layers_by_group,
        excluders_by_target: &excluders_by_target,
        cache: BTreeMap::new(),
    }; // 同一块内重复遇到的稳定候选只解析一次完整上游链。
    for source_layer in layers {
        if source_layer.competition.excludes_groups.is_empty() {
            continue; // 不输出净空的层不需要枚举任何候选。
        }
        let candidates =
            collect_intersecting_active_candidates(source_layer, tile_bounds, &mut activation)?; // 候选启用状态包含组内竞争及全部上游跨组排斥。
        for candidate in candidates {
            let prepared = PreparedExclusionFootprint {
                footprint: source_layer.footprint, // 同层候选共享占用规则。
                transform: candidate.transform,    // 每个候选保留自身旋转与缩放。
            };
            for target_group in &source_layer.competition.excludes_groups {
                by_target_group
                    .entry(*target_group)
                    .or_default()
                    .push(prepared); // 一个高层可以向多个明确目标组输出同一占用轮廓。
            }
        }
    }
    Ok(by_target_group)
}

fn exclusion_layers_by_target(
    layers: &[VegetationLayerConfig],
) -> BTreeMap<VegetationCompetitionGroupId, Vec<&VegetationLayerConfig>> {
    let mut result = BTreeMap::<VegetationCompetitionGroupId, Vec<_>>::new(); // 目标组映射到所有向它输出净空的源层。
    for layer in layers {
        for target in &layer.competition.excludes_groups {
            result.entry(*target).or_default().push(layer); // 同一源层可排斥多个明确目标组。
        }
    }
    for excluders in result.values_mut() {
        excluders.sort_by_key(|layer| layer.id); // 递归与缓存遍历保持世界稳定顺序。
    }
    result
}

/// 候选启用判断的完整只读布局上下文。
///
/// 排斥图已经在配置写入时验证为无环，因此递归只沿“源组 → 目标组”的反向上游查询，
/// 最终必定到达没有上游排斥的组；缓存避免一个块内反复解析同一稳定实例。
struct CandidateActivationContext<'a> {
    sources: &'a BTreeMap<VegetationLayerId, VegetationSourceField>,
    groups: &'a BTreeMap<VegetationCompetitionGroupId, VegetationCompetitionGroup>,
    layers_by_group: &'a BTreeMap<VegetationCompetitionGroupId, Vec<&'a VegetationLayerConfig>>,
    excluders_by_target: &'a BTreeMap<VegetationCompetitionGroupId, Vec<&'a VegetationLayerConfig>>,
    cache: BTreeMap<(VegetationLayerId, u64), bool>, // 层ID解除不同层复用相同实例ID的歧义。
}

impl CandidateActivationContext<'_> {
    fn candidate_is_active(
        &mut self,
        source_layer: &VegetationLayerConfig,
        candidate: &crate::VegetationCandidate,
    ) -> Result<bool, VegetationConfigError> {
        let key = (source_layer.id, candidate.instance_id);
        if let Some(active) = self.cache.get(&key) {
            return Ok(*active); // 同一确定性候选的密度与排斥结果在当前输入代次中不变。
        }
        let group = *self.groups.get(&source_layer.competition.group).ok_or(
            VegetationConfigError::MissingGroup {
                layer: source_layer.id,
                group: source_layer.competition.group,
            },
        )?;
        let group_layers = self
            .layers_by_group
            .get(&source_layer.competition.group)
            .expect("validated group must have its source layer"); // 当前层保证组不为空。
        let candidate_xz = candidate.transform.translation.xz();
        let demands = sample_demands(group_layers, self.sources, candidate_xz, 0.0)?; // 显式或程序候选只在自身中心声明，输出纹素所有权扩张不得制造实体。
        let selector = stable_world_selector(
            candidate_xz,
            group.selection_cell_size,
            group.selection_seed,
        ); // 组内覆盖与布局纹素使用同一固定世界选择。
        let mut resolved_density = resolve_competition(group, &demands, selector)?
            .into_iter()
            .find(|share| share.layer == source_layer.id)
            .map(|share| share.density)
            .unwrap_or(0.0)
            * candidate.density; // 显式单点密度与层分配场正交相乘。
        if resolved_density > 0.0 {
            resolved_density *=
                self.upstream_exclusion_influence(source_layer.competition.group, candidate_xz)?; // 已被上游足迹清除的候选不能继续向下游输出幽灵净空。
        }
        let candidate_selector = stable_instance_selector(candidate.instance_id, source_layer.id.0); // 连续显式ID和程序哈希都获得层独立零至一阈值。
        let active = candidate_selector < resolved_density;
        self.cache.insert(key, active); // 只有整条无环上游链成功后才缓存结果。
        Ok(active)
    }

    fn upstream_exclusion_influence(
        &mut self,
        target_group: VegetationCompetitionGroupId,
        world_xz: Vec2,
    ) -> Result<f32, VegetationConfigError> {
        let Some(excluders) = self.excluders_by_target.get(&target_group).cloned() else {
            return Ok(1.0); // 没有上游边时当前组不受跨组足迹影响。
        };
        let mut influence = 1.0_f32;
        for source_layer in excluders {
            for candidate in candidates_affecting_point(source_layer, world_xz) {
                if self.candidate_is_active(source_layer, &candidate)? {
                    influence = influence.min(footprint_influence(
                        source_layer.footprint,
                        candidate.transform,
                        world_xz,
                    )); // 多个活动上游足迹按并集取最强排斥。
                    if influence <= 0.0 {
                        return Ok(0.0); // 硬净空命中后无需继续枚举其他上游候选。
                    }
                }
            }
        }
        Ok(influence)
    }
}

fn collect_intersecting_active_candidates(
    source_layer: &VegetationLayerConfig,
    tile_bounds: Rect,
    activation: &mut CandidateActivationContext<'_>,
) -> Result<Vec<crate::VegetationCandidate>, VegetationConfigError> {
    let mut candidates = Vec::new(); // 一个64纹素块通常只命中少量稀疏占用原型。
    match &source_layer.source {
        VegetationCandidateSource::ExplicitPlacements(placements) => {
            for index in 0..placements.len() {
                let Some(candidate) =
                    deterministic_candidate(&source_layer.source, IVec2::new(index as i32, 0))
                else {
                    continue; // 索引来自同一表长度，保留防御性分支。
                };
                if !footprint_intersects_tile(
                    source_layer.footprint,
                    candidate.transform,
                    tile_bounds,
                ) {
                    continue; // 全图稳定清单在当前块外的绝大多数项不进入逐纹素循环。
                }
                if activation.candidate_is_active(source_layer, &candidate)? {
                    candidates.push(candidate); // 只缓存当前解析版本中实际启用的显式候选。
                }
            }
        }
        VegetationCandidateSource::DenseGrid { spacing, .. }
        | VegetationCandidateSource::JitteredGrid { spacing, .. } => {
            let footprint_reach = footprint_reach(source_layer.footprint, Transform::IDENTITY); // 程序网格候选没有额外缩放。
            let jitter_reach = match &source_layer.source {
                VegetationCandidateSource::JitteredGrid { jitter, .. } => *jitter * *spacing,
                _ => 0.0, // 规则网格候选固定在单元中心。
            };
            let search_reach = Vec2::splat(footprint_reach + jitter_reach); // 抖动候选可能从相邻格移入当前块。
            let minimum_cell = ((tile_bounds.min - search_reach) / *spacing)
                .floor()
                .as_ivec2(); // 负坐标按floor扩展到最小候选格。
            let maximum_cell = ((tile_bounds.max + search_reach) / *spacing)
                .floor()
                .as_ivec2(); // 闭区间允许边界候选参与软恢复。
            for z in minimum_cell.y..=maximum_cell.y {
                for x in minimum_cell.x..=maximum_cell.x {
                    let cell: VegetationWorldCell = IVec2::new(x, z); // 正负世界格共享相同稳定身份。
                    let Some(candidate) = deterministic_candidate(&source_layer.source, cell)
                    else {
                        continue; // 程序化源当前总是产生候选，保留接口完整性。
                    };
                    if !footprint_intersects_tile(
                        source_layer.footprint,
                        candidate.transform,
                        tile_bounds,
                    ) {
                        continue; // 保守格范围中的角落候选若不相交则立即跳过。
                    }
                    if activation.candidate_is_active(source_layer, &candidate)? {
                        candidates.push(candidate); // 密集分布也只留下当前块附近的活动足迹。
                    }
                }
            }
        }
    }
    candidates.sort_by_key(|candidate| candidate.instance_id); // 显式表顺序改变时仍保持逐字节稳定遍历。
    Ok(candidates)
}

fn candidates_affecting_point(
    source_layer: &VegetationLayerConfig,
    world_xz: Vec2,
) -> Vec<crate::VegetationCandidate> {
    let mut candidates = Vec::new(); // 上游查询只收集足迹真正覆盖当前世界点的少量候选。
    match &source_layer.source {
        VegetationCandidateSource::ExplicitPlacements(placements) => {
            for index in 0..placements.len() {
                let Some(candidate) =
                    deterministic_candidate(&source_layer.source, IVec2::new(index as i32, 0))
                else {
                    continue; // 索引来自同一稳定表长度。
                };
                if footprint_influence(source_layer.footprint, candidate.transform, world_xz) < 1.0
                {
                    candidates.push(candidate); // 仅保留实际轮廓、margin或恢复带触及当前点的项。
                }
            }
        }
        VegetationCandidateSource::DenseGrid { spacing, .. }
        | VegetationCandidateSource::JitteredGrid { spacing, .. } => {
            let reach = footprint_reach(source_layer.footprint, Transform::IDENTITY);
            let jitter_reach = match &source_layer.source {
                VegetationCandidateSource::JitteredGrid { jitter, .. } => *jitter * *spacing,
                _ => 0.0,
            };
            let search = Vec2::splat(reach + jitter_reach); // 相邻格候选可因抖动移到查询点附近。
            let minimum_cell = ((world_xz - search) / *spacing).floor().as_ivec2();
            let maximum_cell = ((world_xz + search) / *spacing).floor().as_ivec2();
            for z in minimum_cell.y..=maximum_cell.y {
                for x in minimum_cell.x..=maximum_cell.x {
                    let Some(candidate) =
                        deterministic_candidate(&source_layer.source, IVec2::new(x, z))
                    else {
                        continue;
                    };
                    if footprint_influence(source_layer.footprint, candidate.transform, world_xz)
                        < 1.0
                    {
                        candidates.push(candidate); // 保守搜索范围中的角落候选若未触及则跳过。
                    }
                }
            }
        }
    }
    candidates.sort_by_key(|candidate| candidate.instance_id); // 递归上游遍历保持稳定顺序。
    candidates
}

fn footprint_intersects_tile(
    footprint: VegetationFootprint,
    transform: Transform,
    tile_bounds: Rect,
) -> bool {
    let reach = footprint_reach(footprint, transform); // 用包围圆保守覆盖旋转矩形和非等比缩放。
    let center = transform.translation.xz(); // 足迹世界中心来自候选根。
    center.x + reach >= tile_bounds.min.x
        && center.x - reach <= tile_bounds.max.x
        && center.y + reach >= tile_bounds.min.y
        && center.y - reach <= tile_bounds.max.y // 仅把可能触及块的候选加入稀疏索引。
}

fn footprint_reach(footprint: VegetationFootprint, transform: Transform) -> f32 {
    let base = match footprint.shape {
        crate::VegetationFootprintShape::Circle { radius } => radius,
        crate::VegetationFootprintShape::Ellipse { half_extents }
        | crate::VegetationFootprintShape::Rectangle { half_extents }
        | crate::VegetationFootprintShape::MeshAabb { half_extents } => {
            half_extents.length() // 角点到中心的半对角线才是旋转矩形的保守圆形搜索范围。
        }
    };
    let scale = match footprint.scale {
        VegetationFootprintScale::InheritTransform => transform.scale.xz().abs().max_element(), // 显式非等比缩放按最大轴保守扩张搜索圆。
        VegetationFootprintScale::WorldSpace => 1.0, // 固定世界足迹不跟随外观实例缩放扩大搜索。
    };
    base * scale + footprint.exclusion_margin + footprint.recovery_width // 包含实际轮廓、硬净空和软恢复带。
}

#[cfg(test)]
mod tests; // 合成回归独立存放，生产空间算法保持在五百行以内。
