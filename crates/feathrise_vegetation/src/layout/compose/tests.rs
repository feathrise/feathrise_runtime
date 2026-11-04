//! 脏块合成、跨组足迹链和输出单元所有权的纯逻辑回归。

use bevy::prelude::{Rect, Vec2, Vec3}; // 构造固定64米测试块和显式占用中心。

use crate::{
    ExplicitVegetationPlacement, VegetationClaimSampling, VegetationCompetition,
    VegetationCompetitionGroup, VegetationCompetitionMode, VegetationDensityField,
    VegetationFootprintScale, VegetationFootprintShape, VegetationPrototypeId,
    VegetationRealization, VegetationWeight,
}; // 通过公共正交配置覆盖项目可组合的空间语义。

use super::*; // 直接调用父模块的纯块合成器和私有辅助函数。

fn test_bounds() -> Rect {
    Rect::from_corners(Vec2::ZERO, Vec2::splat(64.0)) // 一米纹素正好覆盖一个固定64×64块。
}

fn additive_group(id: VegetationCompetitionGroupId) -> VegetationCompetitionGroup {
    VegetationCompetitionGroup {
        id,                                        // 调用者提供稳定竞争组身份。
        mode: VegetationCompetitionMode::Additive, // 足迹链测试不改变组内基础密度。
        selection_cell_size: 1.0,                  // 使用合法固定世界选择格。
        selection_seed: id.0,                      // 每组稳定种子与身份一致便于诊断。
    }
}

fn layer(
    id: u64,
    group: VegetationCompetitionGroupId,
    source: VegetationCandidateSource,
    excludes_groups: Vec<VegetationCompetitionGroupId>,
    realization: VegetationRealization,
) -> VegetationLayerConfig {
    VegetationLayerConfig {
        id: VegetationLayerId(id),            // 测试层使用短稳定ID。
        prototype: VegetationPrototypeId(id), // 原型身份不参与布局数值。
        source,                               // 调用者选择规则网格或显式落点。
        density: VegetationDensityField {
            world_bounds: test_bounds(), // 所有测试层覆盖完整固定块。
            gain: 1.0,                   // 保留源字段归一化值。
        },
        competition: VegetationCompetition {
            group,                                          // 层参与调用者指定组。
            priority: 0,                                    // 默认同级不影响足迹测试。
            weight: VegetationWeight::default(),            // 默认等权。
            claim_sampling: VegetationClaimSampling::Point, // 默认保持点所有权。
            excludes_groups,                                // 调用者声明跨组净空边。
        },
        footprint: VegetationFootprint {
            shape: VegetationFootprintShape::Circle { radius: 2.0 }, // 足以覆盖测试中心纹素。
            scale: VegetationFootprintScale::InheritTransform,       // 通用实体轮廓继承实例缩放。
            exclusion_margin: 0.0,                                   // 测试不额外扩大硬净空。
            recovery_width: 0.0,                                     // 测试使用清晰的零或一边界。
        },
        realization, // 调用者锁定GPU或实体实现方式。
    }
}

fn full_sources(
    layers: &[VegetationLayerConfig],
) -> BTreeMap<VegetationLayerId, VegetationSourceField> {
    layers
        .iter()
        .map(|layer| {
            (
                layer.id, // 每层按自身稳定ID注册输入。
                VegetationSourceField::new(
                    test_bounds(),
                    64,
                    64,
                    vec![1.0; 64 * 64], // 全块满密度隔离足迹与竞争行为。
                )
                .unwrap(),
            )
        })
        .collect() // 稳定映射直接传给合成器。
}

#[test]
fn explicit_footprints_clear_target_group_without_scanning_per_texel() {
    let target_group = VegetationCompetitionGroupId(1); // 被排斥的地表组。
    let source_group = VegetationCompetitionGroupId(2); // 输出占用的高层组。
    let target_layer = layer(
        1,
        target_group,
        VegetationCandidateSource::DenseGrid {
            spacing: 1.0, // 目标层每米一个候选。
            seed: 1,      // 固定分布种子。
        },
        Vec::new(),
        VegetationRealization::GpuOnly,
    );
    let source_layer = layer(
        2,
        source_group,
        VegetationCandidateSource::ExplicitPlacements(vec![
            ExplicitVegetationPlacement {
                instance_id: 7, // 小整数ID仍经过稳定重哈希启用。
                transform: Transform::from_translation(Vec3::new(32.5, 0.0, 32.5)), // 当前块中心的有效占用。
                density: 1.0, // 显式候选完整启用。
            },
            ExplicitVegetationPlacement {
                instance_id: 8, // 远处清单项验证块级稀疏过滤。
                transform: Transform::from_translation(Vec3::new(10_000.0, 0.0, 10_000.0)), // 不触及当前块。
                density: 1.0, // 满密度也不能影响远块。
            },
        ]),
        vec![target_group],
        VegetationRealization::Entities,
    );
    let groups = [additive_group(target_group), additive_group(source_group)]; // 两组只由跨组足迹建立关系。
    let layers = [target_layer, source_layer]; // 目标层与占用层共同参与合成。
    let tile = compose_tile(
        VegetationDirtyTile(IVec2::ZERO),
        1.0,
        &groups,
        &layers,
        &full_sources(&layers),
    )
    .unwrap();
    let target = tile
        .layers
        .iter()
        .find(|resolved| resolved.layer == VegetationLayerId(1))
        .unwrap(); // 读取被排斥目标层。
    assert_eq!(target.density[32 * 64 + 32], 0.0); // 实例中心纹素位于实际足迹内。
    assert_eq!(target.density[0], 1.0); // 远离占用的块角保持完整目标密度。
}

#[test]
fn excluded_middle_group_cannot_emit_ghost_footprint_downstream() {
    let low_group = VegetationCompetitionGroupId(1); // 最下游地被组。
    let middle_group = VegetationCompetitionGroupId(2); // 会排斥地被但自身受高层排斥。
    let high_group = VegetationCompetitionGroupId(3); // 排斥中间组的最高占用层。
    let target_layer = layer(
        1,
        low_group,
        VegetationCandidateSource::DenseGrid {
            spacing: 1.0, // 地被在完整测试块每米生成候选。
            seed: 1,      // 固定种子保持结果稳定。
        },
        Vec::new(),
        VegetationRealization::GpuOnly,
    );
    let explicit_layer = |id, group, excludes_groups| {
        layer(
            id,
            group,
            VegetationCandidateSource::ExplicitPlacements(vec![ExplicitVegetationPlacement {
                instance_id: id, // 实例ID与测试层一致便于追踪递归缓存。
                transform: Transform::from_translation(Vec3::new(32.5, 0.0, 32.5)), // 两层占用重叠在块中心。
                density: 1.0, // 两个显式候选都满足自身密度。
            }]),
            excludes_groups,
            VegetationRealization::Entities,
        )
    };
    let middle_layer = explicit_layer(2, middle_group, vec![low_group]); // B→C。
    let high_layer = explicit_layer(3, high_group, vec![middle_group]); // A→B，整体图无环。
    let groups = [
        additive_group(low_group),
        additive_group(middle_group),
        additive_group(high_group),
    ];
    let layers = [target_layer, middle_layer, high_layer]; // 三层共同验证上游递归启用状态。
    crate::competition::validate_configuration(&groups, &layers)
        .expect("A排斥B、B排斥C是合法无环关系");
    let tile = compose_tile(
        VegetationDirtyTile(IVec2::ZERO),
        1.0,
        &groups,
        &layers,
        &full_sources(&layers),
    )
    .unwrap();
    let center = 32 * 64 + 32; // 三层显式落点所在输出纹素。
    let density = |layer| {
        tile.layers
            .iter()
            .find(|resolved| resolved.layer == VegetationLayerId(layer))
            .unwrap()
            .density[center]
    };
    assert_eq!(density(3), 1.0); // A在自己的组中保持活动。
    assert_eq!(density(2), 0.0); // A的足迹清除B。
    assert_eq!(density(1), 1.0); // 已清除的B不得再生成幽灵足迹清除C。
}

#[test]
fn overlapping_source_cell_claim_clears_lower_layer_without_growing_crop_density() {
    let group_id = VegetationCompetitionGroupId(4); // 草和作物在同一优先覆盖组竞争。
    let mut grass = layer(
        1,
        group_id,
        VegetationCandidateSource::DenseGrid {
            spacing: 1.0, // 草层每米一个候选。
            seed: 1,      // 固定候选种子。
        },
        Vec::new(),
        VegetationRealization::GpuOnly,
    );
    grass.competition.priority = 0; // 草是被作物所有权覆盖的低层。
    let mut crop = layer(
        2,
        group_id,
        VegetationCandidateSource::DenseGrid {
            spacing: 1.0, // 作物候选密度不影响本次数值场断言。
            seed: 2,      // 与草分布保持独立。
        },
        Vec::new(),
        VegetationRealization::GpuOnly,
    );
    crop.competition.priority = 1; // 作物声明优先于普通草。
    crop.competition.claim_sampling = VegetationClaimSampling::SourceCellOverlap; // 作物按源采样单元相交取得输出所有权。
    let layers = [grass, crop]; // 两层共同解析一个优先覆盖组。
    let group = VegetationCompetitionGroup {
        id: group_id, // 与两层竞争声明一致。
        mode: VegetationCompetitionMode::PriorityOverride {
            claim_threshold: 0.0, // 任意非零相交源单元都取得所有权。
        },
        selection_cell_size: 1.0, // 同级选择使用固定一米世界格。
        selection_seed: 4,        // 固定选择种子。
    };
    let mut crop_values = vec![0.0; 65 * 65]; // 端点式一米源图默认全部为零。
    crop_values[65 + 1] = 1.0; // 仅世界(1,1)源样本声明作物。
    let sources = [
        (
            VegetationLayerId(1),
            VegetationSourceField::constant(test_bounds(), 1.0).unwrap(), // 草在整个块满密度。
        ),
        (
            VegetationLayerId(2),
            VegetationSourceField::new(test_bounds(), 65, 65, crop_values).unwrap(), // 作物保持一米端点式输入。
        ),
    ]
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    let tile = compose_tile(
        VegetationDirtyTile(IVec2::ZERO),
        1.0,
        &[group],
        &layers,
        &sources,
    )
    .unwrap();
    let output_index = 64 + 1; // 世界中心(1.5,1.5)最近点为零，但单元与(1,1)源单元相交。
    let density = |layer| {
        tile.layers
            .iter()
            .find(|resolved| resolved.layer == VegetationLayerId(layer))
            .unwrap()
            .density[output_index]
    };
    assert_eq!(density(1), 0.0); // 作物相交所有权清除普通草。
    assert_eq!(density(2), 0.0); // 作物实际点密度仍为零，不能向外扩大可见作物。
}
