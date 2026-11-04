//! 竞争份额、关系循环与非法配置的纯逻辑回归。

use bevy::prelude::{Rect, Vec2}; // 配置验证测试构造有限非空世界范围。

use crate::{
    VegetationCandidateSource, VegetationClaimSampling, VegetationCompetition,
    VegetationDensityField, VegetationFootprint, VegetationFootprintScale,
    VegetationFootprintShape, VegetationLayerConfig, VegetationPrototypeId, VegetationRealization,
}; // 构造无内容类别的最小合法层并逐项注入错误。

use super::*; // 纯竞争测试直接覆盖私有水填充和公共关系式助手。

fn group(mode: VegetationCompetitionMode) -> VegetationCompetitionGroup {
    VegetationCompetitionGroup {
        id: VegetationCompetitionGroupId(7), // 测试只需一个稳定组。
        mode,                                // 每个断言显式选择竞争语义。
        selection_cell_size: 1.0,            // 固定世界选择格使用合法正尺寸。
        selection_seed: 11,                  // 稳定种子不影响单点纯解析器。
    }
}

fn demand(layer: u64, density: f32, priority: i32, weight: f32) -> VegetationLayerDemand {
    VegetationLayerDemand {
        layer: VegetationLayerId(layer), // 简短数字保持断言可读。
        density,                         // 测试精确输入归一化需求。
        claim_density: density,          // 常规测试的所有权与实际密度保持一致。
        priority,                        // 验证层级容量消费顺序。
        weight: VegetationWeight::new(weight).unwrap(), // 测试只构造受检正权重。
    }
}

fn density(shares: &[VegetationLayerShare], layer: u64) -> f32 {
    shares
        .iter()
        .find(|share| share.layer == VegetationLayerId(layer))
        .unwrap()
        .density // 所有模式必须为每个输入层返回显式份额。
}

fn configured_layer(
    id: u64,
    group: u64,
    excludes_groups: Vec<VegetationCompetitionGroupId>,
) -> VegetationLayerConfig {
    VegetationLayerConfig {
        id: VegetationLayerId(id),            // 测试稳定层身份。
        prototype: VegetationPrototypeId(id), // 原型不参与配置图。
        source: VegetationCandidateSource::DenseGrid {
            spacing: 1.0, // 合法米制候选格。
            seed: id,     // 固定层种子。
        },
        density: VegetationDensityField {
            world_bounds: Rect::from_corners(Vec2::ZERO, Vec2::splat(64.0)), // 合法非空输入范围。
            gain: 1.0,                                                       // 不改变归一化需求。
        },
        competition: VegetationCompetition {
            group: VegetationCompetitionGroupId(group), // 调用者选择自身组。
            priority: 0,                                // 图验证不依赖数值优先级。
            weight: VegetationWeight::default(),        // 使用受检正权重。
            claim_sampling: VegetationClaimSampling::Point, // 通用测试保持既有点所有权。
            excludes_groups,                            // 调用者构造跨组排斥边。
        },
        footprint: VegetationFootprint {
            shape: VegetationFootprintShape::Circle { radius: 0.5 }, // 合法实际占用。
            scale: VegetationFootprintScale::InheritTransform,       // 通用测试继承实例缩放。
            exclusion_margin: 0.0,                                   // 无额外硬净空。
            recovery_width: 0.0,                                     // 无软恢复。
        },
        realization: VegetationRealization::GpuOnly, // 后端方式不影响图验证。
    }
}

#[test]
fn additive_keeps_both_full_density_layers() {
    let shares = resolve_competition(
        group(VegetationCompetitionMode::Additive),
        &[demand(1, 1.0, 0, 1.0), demand(2, 1.0, 0, 1.0)],
        0.0,
    )
    .unwrap(); // 合法叠加配置不得失败。
    assert_eq!(density(&shares, 1), 1.0); // 第一层100仍为100。
    assert_eq!(density(&shares, 2), 1.0); // 第二层100仍为100。
}

#[test]
fn weighted_partition_supports_equal_and_one_to_three_weights() {
    let equal = resolve_competition(
        group(VegetationCompetitionMode::WeightedPartition { capacity: 1.0 }),
        &[demand(1, 1.0, 0, 1.0), demand(2, 1.0, 0, 1.0)],
        0.0,
    )
    .unwrap(); // 等权满需求严格均分组容量。
    assert!((density(&equal, 1) - 0.5).abs() < 1.0e-6);
    assert!((density(&equal, 2) - 0.5).abs() < 1.0e-6);
    let weighted = resolve_competition(
        group(VegetationCompetitionMode::WeightedPartition { capacity: 1.0 }),
        &[demand(1, 1.0, 0, 1.0), demand(2, 1.0, 0, 3.0)],
        0.0,
    )
    .unwrap(); // 1:3权重严格得到25/75。
    assert!((density(&weighted, 1) - 0.25).abs() < 1.0e-6);
    assert!((density(&weighted, 2) - 0.75).abs() < 1.0e-6);
}

#[test]
fn weighted_partition_preserves_shortage_and_passes_remaining_capacity_down() {
    let shares = resolve_competition(
        group(VegetationCompetitionMode::WeightedPartition { capacity: 1.0 }),
        &[
            demand(1, 0.25, 10, 1.0), // 高层只要求四分之一，不得放大。
            demand(2, 1.0, 0, 1.0),   // 低层可使用剩余四分之三。
        ],
        0.0,
    )
    .unwrap();
    assert!((density(&shares, 1) - 0.25).abs() < 1.0e-6);
    assert!((density(&shares, 2) - 0.75).abs() < 1.0e-6);
}

#[test]
fn priority_override_threshold_zero_clears_lower_layer() {
    let shares = resolve_competition(
        group(VegetationCompetitionMode::PriorityOverride {
            claim_threshold: 0.0,
        }),
        &[
            demand(1, 1.0, 0, 1.0), // 低优先级层满密度。
            demand(2, 0.1, 1, 1.0), // 任意非零高优先级层。
        ],
        0.5,
    )
    .unwrap();
    assert_eq!(density(&shares, 1), 0.0); // 低层被严格清除。
    assert!((density(&shares, 2) - 0.1).abs() < 1.0e-6); // 高层保留自身需求。
}

#[test]
fn priority_claim_can_clear_lower_layer_without_expanding_render_density() {
    let mut low = demand(1, 1.0, 0, 1.0); // 低层在当前输出点有完整实际密度。
    low.claim_density = 1.0; // 低层正常声明同一点所有权。
    let mut high = demand(2, 0.0, 1, 1.0); // 高层实际点采样为零，不应凭空渲染。
    high.claim_density = 1.0; // 相交源采样单元仍声明该输出单元归高层。
    let shares = resolve_competition(
        group(VegetationCompetitionMode::PriorityOverride {
            claim_threshold: 0.0,
        }),
        &[low, high],
        0.5,
    )
    .unwrap();
    assert_eq!(density(&shares, 1), 0.0); // 高优先级所有权清除低层。
    assert_eq!(density(&shares, 2), 0.0); // 高层仍输出自己的实际零密度，不扩大可见范围。
}

#[test]
fn relational_priority_cycle_reports_closed_full_chain() {
    let mut builder = PriorityConstraintBuilder::default(); // 使用关系式辅助器模拟外部配置。
    builder
        .higher_than(VegetationLayerId(1), VegetationLayerId(2))
        .higher_than(VegetationLayerId(2), VegetationLayerId(3))
        .higher_than(VegetationLayerId(3), VegetationLayerId(1)); // 构造A>B>C>A。
    let error = builder.build().unwrap_err(); // 循环不得静默降级到某个数值顺序。
    assert_eq!(
        error,
        VegetationConfigError::PriorityCycle {
            chain: vec![
                VegetationLayerId(1),
                VegetationLayerId(2),
                VegetationLayerId(3),
                VegetationLayerId(1),
            ],
        }
    );
}

#[test]
fn exclusion_cycle_reports_closed_full_group_chain() {
    let groups = [1, 2, 3].map(|id| group(VegetationCompetitionMode::Additive).with_id(id)); // 三个合法组构造A>B>C>A。
    let layers = vec![
        configured_layer(10, 1, vec![VegetationCompetitionGroupId(2)]),
        configured_layer(20, 2, vec![VegetationCompetitionGroupId(3)]),
        configured_layer(30, 3, vec![VegetationCompetitionGroupId(1)]),
    ];
    assert_eq!(
        validate_configuration(&groups, &layers),
        Err(VegetationConfigError::ExclusionCycle {
            chain: vec![
                VegetationCompetitionGroupId(1),
                VegetationCompetitionGroupId(2),
                VegetationCompetitionGroupId(3),
                VegetationCompetitionGroupId(1),
            ],
        })
    ); // 循环排斥不得静默选择解析顺序。
}

#[test]
fn duplicate_layer_missing_group_and_non_finite_spacing_are_rejected() {
    let groups = [group(VegetationCompetitionMode::Additive)]; // 唯一合法组ID为七。
    let valid = configured_layer(1, 7, Vec::new());
    assert_eq!(
        validate_configuration(&groups, &[valid.clone(), valid.clone()]),
        Err(VegetationConfigError::DuplicateLayer(VegetationLayerId(1)))
    ); // 重复稳定层身份必须原子拒绝。
    assert_eq!(
        validate_configuration(&groups, &[configured_layer(2, 99, Vec::new())]),
        Err(VegetationConfigError::MissingGroup {
            layer: VegetationLayerId(2),
            group: VegetationCompetitionGroupId(99),
        })
    ); // 自身竞争组缺失时不能创建隐式默认组。
    let mut invalid_spacing = configured_layer(3, 7, Vec::new());
    invalid_spacing.source = VegetationCandidateSource::DenseGrid {
        spacing: f32::NAN, // 非有限候选间距会破坏世界格索引。
        seed: 3,
    };
    assert!(matches!(
        validate_configuration(&groups, &[invalid_spacing]),
        Err(VegetationConfigError::InvalidValue { .. })
    )); // NaN不得进入布局代次。
}

trait TestGroupId {
    fn with_id(self, id: u64) -> Self;
}

impl TestGroupId for VegetationCompetitionGroup {
    fn with_id(mut self, id: u64) -> Self {
        self.id = VegetationCompetitionGroupId(id); // 测试便捷替换稳定组ID而不复制模式字段。
        self
    }
}
