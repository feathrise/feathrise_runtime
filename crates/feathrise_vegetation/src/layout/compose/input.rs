//! GPU 加载/换版冷路径使用的未决竞争块输入编码。

use std::collections::BTreeMap; // 正式组、层和足迹索引必须保持与CPU合成器相同的稳定顺序。

use bevy::prelude::{Rect, Vec2}; // 每个64纹素块仍按世界X/Z中心点采样。

use crate::{
    VegetationCompetitionGroup, VegetationCompetitionGroupId, VegetationConfigError,
    VegetationLayerConfig, VegetationLayerId, VegetationSourceField, footprint_influence,
    stable_world_selector,
}; // 编码器只采样正式输入，不调用`resolve_competition`产生参考答案。

use super::super::resources::{VEGETATION_TILE_SIDE, VEGETATION_TILE_TEXEL_COUNT};
use super::{VegetationDirtyTile, group_layers, prepare_exclusion_footprints, sample_demands}; // 复用CPU合成器已经验收的组内采样和稀疏足迹准备口径。

/// 一个竞争组内不随纹素变化的稳定层描述。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VegetationCompetitionInputLayer {
    pub(crate) layer: VegetationLayerId, // GPU目标层索引由稳定层ID查找，不编码进叶片实例ABI。
    pub(crate) priority: i32,            // 数值越大越先参与容量或覆盖解析。
    pub(crate) weight: f32,              // 构造阶段已经验证为有限正值。
}

/// 一个64×64块的未决GPU竞争输入。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VegetationCompetitionTileInputs {
    pub(crate) group: VegetationCompetitionGroup, // 模式、容量或阈值是GPU解析的正式参数。
    pub(crate) layers: Vec<VegetationCompetitionInputLayer>, // 层描述按稳定ID升序且每块完全一致。
    pub(crate) samples: Vec<[f32; 2]>,            // 每纹素逐层保存实际密度与所有权密度。
    pub(crate) contexts: Vec<[f32; 2]>,           // 每纹素保存世界稳定选择值与目标组足迹影响。
}

/// 把一个正式布局块编码成尚未执行组内竞争的GPU输入。
pub(crate) fn encode_competition_tile_inputs(
    tile: VegetationDirtyTile, // 当前`VegetationLayoutCommit`中的稳定块坐标。
    texel_size: f32,           // 与活动布局和CPU参考完全相同的世界纹素边长。
    target_group: VegetationCompetitionGroupId, // 当前绑定层所属竞争组。
    groups: &[VegetationCompetitionGroup], // 当前输入代次的完整受检组配置。
    layers: &[VegetationLayerConfig], // 当前输入代次的完整受检层配置。
    sources: &BTreeMap<VegetationLayerId, VegetationSourceField>, // 已就绪正式源字段。
) -> Result<VegetationCompetitionTileInputs, VegetationConfigError> {
    let group_map = groups
        .iter()
        .map(|group| (group.id, *group))
        .collect::<BTreeMap<_, _>>(); // 与CPU块合成一样按稳定组ID建立只读索引。
    let grouped_layers = group_layers(layers); // 每个纹素只扫描目标竞争组中的相关层。
    let target_layers =
        grouped_layers
            .get(&target_group)
            .ok_or_else(|| VegetationConfigError::InvalidValue {
                context: format!(
                    "vegetation GPU competition target group {:?} has no configured layers",
                    target_group
                ),
            })?; // 无层组不能生成可验证的GPU目标。
    let group = *group_map
        .get(&target_group)
        .ok_or(VegetationConfigError::MissingGroup {
            layer: target_layers[0].id, // 报告第一个稳定引用层便于定位损坏配置。
            group: target_group,        // 保留缺失组的完整公共身份。
        })?;
    let tile_world_size = VEGETATION_TILE_SIDE as f32 * texel_size; // 块范围与CPU合成器逐位同源。
    let world_min = tile.0.as_vec2() * tile_world_size; // 负块使用floor网格而不会向零折返。
    let world_bounds = Rect::from_corners(
        world_min,                                // 当前块闭开范围的世界最小角。
        world_min + Vec2::splat(tile_world_size), // 当前块闭开范围的世界最大外边。
    );
    let exclusion_footprints = prepare_exclusion_footprints(
        world_bounds,    // 只收集可能影响当前块的活动稀疏候选。
        layers,          // 上游候选继续使用完整正式层配置。
        sources,         // 候选启用判断读取同一输入代次。
        &group_map,      // 上游组内解析沿用受检组索引。
        &grouped_layers, // 上游组内层集合保持稳定顺序。
    )?;
    let encoded_layers = target_layers
        .iter()
        .map(|layer| VegetationCompetitionInputLayer {
            layer: layer.id,                        // 稳定ID只用于CPU目标索引与诊断。
            priority: layer.competition.priority,   // GPU直接执行数值优先级语义。
            weight: layer.competition.weight.get(), // 受检权重保留原始f32位。
        })
        .collect::<Vec<_>>();
    let sample_capacity = VEGETATION_TILE_TEXEL_COUNT
        .checked_mul(encoded_layers.len())
        .ok_or_else(|| VegetationConfigError::InvalidValue {
            context: "vegetation GPU competition tile sample capacity overflowed".to_owned(),
        })?; // 无限公共层仍必须服从当前GPU作业的可验证平台容量。
    let mut samples = Vec::with_capacity(sample_capacity); // 行优先纹素内再按稳定层顺序排列。
    let mut contexts = Vec::with_capacity(VEGETATION_TILE_TEXEL_COUNT); // 每纹素只有一个选择值和组影响。
    for y in 0..VEGETATION_TILE_SIDE {
        for x in 0..VEGETATION_TILE_SIDE {
            let world_xz =
                world_min + (Vec2::new(x as f32, y as f32) + Vec2::splat(0.5)) * texel_size; // 与CPU参考完全相同的纹素中心。
            let demands = sample_demands(target_layers, sources, world_xz, texel_size)?; // 只编码未决实际/所有权需求。
            samples.extend(
                demands
                    .iter()
                    .map(|demand| [demand.density, demand.claim_density]),
            ); // GPU按同一稳定层次序执行Additive、Weighted或Priority解析。
            let selector = stable_world_selector(
                world_xz,                  // 固定绝对世界点不依赖图集原点或相机。
                group.selection_cell_size, // 使用正式组选择格边长。
                group.selection_seed,      // 使用正式组选择种子。
            );
            let influence = exclusion_footprints
                .get(&target_group)
                .map(|footprints| {
                    footprints
                        .iter()
                        .map(|prepared| {
                            footprint_influence(
                                prepared.footprint, // 稀疏候选的正式轮廓、边距和恢复宽度。
                                prepared.transform, // 保留实例世界位置、旋转和缩放口径。
                                world_xz,           // 计算当前输出纹素的目标组净空。
                            )
                        })
                        .fold(1.0_f32, f32::min)
                })
                .unwrap_or(1.0); // 没有上游排斥时目标组保持完整密度。
            contexts.push([selector, influence]); // GPU竞争完成后只乘一次同组足迹并集影响。
        }
    }
    Ok(VegetationCompetitionTileInputs {
        group,                  // GPU模式参数来自本次正式输入代次。
        layers: encoded_layers, // 每块输出同一稳定层描述。
        samples,                // 仍未执行任何组内竞争。
        contexts,               // 选择值和足迹影响也不包含最终目标密度。
    })
}
