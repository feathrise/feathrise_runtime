//! 近场回归共享的合法配置、显式落点、活动布局和无渲染应用fixture。

use std::sync::Arc; // 测试块与正式活动布局保持相同不可变共享语义。

use bevy::prelude::{App, IVec2, Rect, Transform, Vec2}; // 构造真实插件应用和世界空间输入。

use crate::{
    // fixture只组合公共内容无关植被配置。
    ActiveVegetationLayout,
    ExplicitVegetationPlacement,
    VegetationCandidateSource,
    VegetationClaimSampling,
    VegetationCompetition,
    VegetationCompetitionGroup,
    VegetationCompetitionGroupId,
    VegetationCompetitionMode,
    VegetationDensityField,
    VegetationDirtyTile,
    VegetationFootprint,
    VegetationFootprintScale,
    VegetationFootprintShape,
    VegetationLayerConfig,
    VegetationLayerId,
    VegetationLayoutState,
    VegetationPrototypeId,
    VegetationRealization,
    VegetationResolvedLayer,
    VegetationResolvedTile,
    VegetationSourceField,
    VegetationWeight,
}; // 完成公共fixture类型导入。

use super::super::VegetationNearFieldPlugin; // 基础应用安装真实近场插件。

pub(super) const TEST_LAYER: VegetationLayerId = VegetationLayerId(1); // 基础fixture唯一层身份。
const TEST_GROUP: VegetationCompetitionGroupId = VegetationCompetitionGroupId(1); // 基础fixture唯一竞争组。
pub(super) const TEST_BOUNDS: Rect = Rect {
    // 单个64米活动块覆盖全部正坐标测试。
    min: Vec2::ZERO,        // 测试块从世界原点开始。
    max: Vec2::splat(64.0), // 64×64一米纹素完整覆盖六十四米。
}; // 完成共享测试世界范围。

pub(super) fn competition_group() -> VegetationCompetitionGroup {
    // 返回最小合法加法竞争组。
    VegetationCompetitionGroup {
        // 竞争规则不改变近场候选选择。
        id: TEST_GROUP,                            // 层引用唯一稳定组身份。
        mode: VegetationCompetitionMode::Additive, // 不引入层间容量分配。
        selection_cell_size: 1.0,                  // 使用合法固定世界选择格。
        selection_seed: 0,                         // 固定种子保证重复确定性。
    } // 返回完整合法竞争组。
} // 完成共享竞争组构造。

pub(super) fn placement(
    // 构造一个满密度稳定显式落点。
    instance_id: u64, // 层内唯一稳定实例ID。
    x: f32,           // 世界X位置。
    z: f32,           // 世界Z位置。
) -> ExplicitVegetationPlacement {
    // 返回完整Transform和单点密度。
    ExplicitVegetationPlacement {
        // 内容类型不进入公共落点结构。
        instance_id,                               // 保存稳定实例身份。
        transform: Transform::from_xyz(x, 0.0, z), // 地面Y固定零以隔离XZ逻辑。
        density: 1.0,                              // 满单点密度保证活动布局一时启用。
    } // 返回完整显式落点。
} // 完成共享显式落点构造。

pub(super) fn explicit_layer(
    // 构造合法显式HybridNearField层。
    id: VegetationLayerId,                        // 稳定层身份。
    placements: Vec<ExplicitVegetationPlacement>, // 层内显式落点清单。
    enter_radius: f32,                            // 新代理进入半径。
    exit_radius: f32,                             // 活动代理退出半径。
) -> VegetationLayerConfig {
    // 返回可直接交给公共验证的完整配置。
    VegetationLayerConfig {
        // 填充近场外字段的合法最小值。
        id,                                                                // 保存稳定层身份。
        prototype: VegetationPrototypeId(id.0 + 10), // 每层使用独立内容无关原型。
        source: VegetationCandidateSource::ExplicitPlacements(placements), // 使用调用者显式清单。
        density: VegetationDensityField {
            // 声明活动布局采样范围。
            world_bounds: TEST_BOUNDS, // 正坐标测试落点位于单块内。
            gain: 1.0,                 // 不改变活动布局密度。
        }, // 完成层密度声明。
        competition: VegetationCompetition {
            // 构造最小合法竞争规则。
            group: TEST_GROUP,                              // 引用唯一测试组。
            priority: 0,                                    // 使用默认优先级。
            weight: VegetationWeight::default(),            // 使用合法等权。
            claim_sampling: VegetationClaimSampling::Point, // 单点采样足以验证近场。
            excludes_groups: Vec::new(),                    // 不测试跨组排斥。
        }, // 完成竞争规则。
        footprint: VegetationFootprint {
            // 构造合法小圆足迹。
            shape: VegetationFootprintShape::Circle { radius: 0.5 }, // 半米足迹不影响距离测试。
            scale: VegetationFootprintScale::InheritTransform,       // 沿用公共缩放口径。
            exclusion_margin: 0.0,                                   // 不增加净空。
            recovery_width: 0.0,                                     // 不增加软恢复。
        }, // 完成足迹声明。
        realization: VegetationRealization::HybridNearField {
            // 启用近场滞回实现。
            enter_radius, // 保存进入内圈。
            exit_radius,  // 保存退出外圈。
        }, // 完成近场实现声明。
    } // 返回完整层配置。
} // 完成共享显式层构造。

pub(super) fn full_layout(layer_ids: &[VegetationLayerId]) -> ActiveVegetationLayout {
    // 构造单块全一活动布局。
    ActiveVegetationLayout {
        // 使用真实公开资源结构。
        revision: 1,     // 固定非零活动版本。
        texel_size: 1.0, // 一米纹素与64米块一致。
        tiles: [(
            // 唯一世界原点块覆盖全部正坐标测试。
            VegetationDirtyTile(IVec2::ZERO), // 稳定原点块身份。
            Arc::new(VegetationResolvedTile {
                // 正式相同的不可变共享块。
                coordinate: VegetationDirtyTile(IVec2::ZERO), // 内部身份与键一致。
                world_bounds: TEST_BOUNDS,                    // 块覆盖0到64米。
                layers: layer_ids // 为每层生成独立满密度数组。
                    .iter() // 按调用者层顺序构造。
                    .map(|layer| VegetationResolvedLayer {
                        // 当前层完整启用。
                        layer: *layer,               // 保存稳定层ID。
                        density: vec![1.0; 64 * 64], // 全纹素满密度。
                    }) // 完成当前解析层。
                    .collect(), // 收集单块全部层。
            }), // 完成共享块构造。
        )] // 完成唯一块键值对。
        .into_iter() // 转换为稳定块表迭代器。
        .collect(), // 收集完整活动布局。
    } // 返回全一活动布局。
} // 完成共享活动布局构造。

pub(super) fn setup_app() -> App {
    // 构造真实插件应用验证ECS生命周期。
    let mut app = App::new(); // 无渲染应用足以驱动核心Update。
    app.add_plugins((crate::VegetationPlugin, VegetationNearFieldPlugin)); // 安装布局和近场插件。
    let layer = explicit_layer(
        // 唯一候选位于世界原点右侧两米。
        TEST_LAYER,                   // 使用基础层ID。
        vec![placement(0, 2.0, 0.0)], // 零ID满密度稳定落点。
        3.0,                          // 三米进入圈。
        5.0,                          // 五米退出圈。
    ); // 完成基础层配置。
    let mut state = app.world_mut().resource_mut::<VegetationLayoutState>(); // 取得公共布局状态。
    state // 写入完整组和层配置。
        .set_configuration(vec![competition_group()], vec![layer]) // 推进配置和总状态代次。
        .unwrap(); // fixture配置必须合法。
    state.set_source_field(
        // 注册全一基础密度源。
        TEST_LAYER,                                                 // 源对应唯一测试层。
        VegetationSourceField::constant(TEST_BOUNDS, 1.0).unwrap(), // 全范围满密度。
    ); // 完成源字段登记。
    app.world_mut().insert_resource(full_layout(&[TEST_LAYER])); // 提供已完成活动布局。
    app // 返回可添加观察者并驱动Update的应用。
} // 完成共享应用fixture。
