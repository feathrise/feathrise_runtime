//! 相机、窗口和帧无关的世界稳定候选分布。

use bevy::prelude::{IVec2, Transform, Vec2, Vec3, Vec3Swizzles}; // 候选由整数世界格确定最终X/Z变换。

use crate::{ActiveVegetationLayout, VegetationCandidateSource, VegetationLayerId}; // 所有后端共享同一候选与活动布局语义。

/// 由程序化网格或显式落点得到的稳定候选。
#[derive(Clone, Debug, PartialEq)]
pub struct VegetationCandidate {
    pub instance_id: u64,     // GPU实例与近场实体代理共同使用的稳定身份。
    pub transform: Transform, // 后端不需要再次抖动或随机旋转。
    pub density: f32,         // 显式落点可携带单点需求，网格候选默认为一。
}

/// 从整数世界格确定性地产生一个候选。
pub fn deterministic_candidate(
    source: &VegetationCandidateSource,
    world_cell: IVec2,
) -> Option<VegetationCandidate> {
    match source {
        VegetationCandidateSource::DenseGrid { spacing, seed } => {
            Some(grid_candidate(world_cell, *spacing, 0.0, *seed)) // 规则密网格保持单元中心位置。
        }
        VegetationCandidateSource::JitteredGrid {
            spacing,
            jitter,
            seed,
        } => Some(grid_candidate(
            world_cell,
            *spacing,
            jitter.clamp(0.0, 0.5),
            *seed,
        )), // 受检配置下钳制不改变值，仅保护独立函数调用。
        VegetationCandidateSource::ExplicitPlacements(placements) => {
            if world_cell.y != 0 || world_cell.x < 0 {
                return None; // 显式源把非负X索引映射为稳定表项。
            }
            placements
                .get(world_cell.x as usize)
                .map(|placement| VegetationCandidate {
                    instance_id: placement.instance_id, // 保留离线清单或存档稳定ID。
                    transform: placement.transform,     // 保留完整内容变换。
                    density: placement.density.clamp(0.0, 1.0), // 后端只消费归一化需求。
                })
        }
    }
}

fn grid_candidate(world_cell: IVec2, spacing: f32, jitter: f32, seed: u64) -> VegetationCandidate {
    let hash = hash_world_cell(world_cell, seed); // 一个候选的全部随机属性来自同一稳定格哈希。
    let jitter_x = signed_unit(hash) * jitter * spacing; // X偏移严格留在本单元范围内。
    let jitter_z = signed_unit(splitmix64(hash)) * jitter * spacing; // 再混合得到独立Z偏移。
    let center = (world_cell.as_vec2() + Vec2::splat(0.5)) * spacing; // floor格在负世界坐标同样连续。
    VegetationCandidate {
        instance_id: hash, // 稳定哈希直接作为程序化实例ID。
        transform: Transform::from_translation(Vec3::new(
            center.x + jitter_x,
            0.0,
            center.y + jitter_z,
        )), // 高度由高度场、地面吸附或项目适配器后续填写。
        density: 1.0,      // 100%候选容量由密度场另行控制。
    }
}

/// 返回固定世界选择网格上的零至一稳定哈希。
pub fn stable_world_selector(world_xz: Vec2, cell_size: f32, seed: u64) -> f32 {
    let safe_cell_size = if cell_size.is_finite() && cell_size > 0.0 {
        cell_size // 合法配置保持项目声明的固定世界尺度。
    } else {
        1.0 // 独立调用的失效保护；正式布局验证会原子拒绝非法值。
    };
    let cell = (world_xz / safe_cell_size).floor().as_ivec2(); // floor保证负坐标不会向零折返。
    unit_float(hash_world_cell(cell, seed)) // 相机和帧不进入哈希输入。
}

/// 把任意稳定实例ID与层盐值映射为零至一选择阈值。
pub fn stable_instance_selector(instance_id: u64, salt: u64) -> f32 {
    unit_float(splitmix64(instance_id ^ splitmix64(salt))) // 连续显式ID也必须获得均匀稳定阈值。
}

/// 判断一个确定性候选是否被当前已解析活动布局选中。
///
/// 所有实现后端共用这一入口，避免叶片、Mesh和近场代理分别解释密度概率。
pub fn resolved_candidate_enabled(
    layout: &ActiveVegetationLayout,
    layer: VegetationLayerId,
    candidate: &VegetationCandidate,
) -> bool {
    let density = layout.sample(layer, candidate.transform.translation.xz()) * candidate.density; // 只读取当前原子版本，不遍历其他层。
    stable_instance_selector(candidate.instance_id, layer.0) < density // 稳定实例阈值不依赖相机、帧或窗口。
}

pub(crate) fn hash_world_cell(cell: IVec2, seed: u64) -> u64 {
    let x = cell.x as i64 as u64; // 保留负数二进制身份，避免正负坐标碰撞。
    let z = cell.y as i64 as u64; // 二维Vec2的Y在植被语义中表示世界Z。
    splitmix64(seed ^ splitmix64(x) ^ splitmix64(z.rotate_left(32))) // 两轴和层种子充分混合。
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15); // 固定奇数增量与WGSL端可逐项复刻。
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9); // 第一轮雪崩。
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB); // 第二轮雪崩。
    value ^ (value >> 31) // 输出完整64位世界稳定散列。
}

fn unit_float(hash: u64) -> f32 {
    let mantissa = (hash >> 40) as u32; // 取24个高位，完全覆盖f32单位区间精度。
    mantissa as f32 / 16_777_216.0 // 结果严格位于[0,1)。
}

fn signed_unit(hash: u64) -> f32 {
    unit_float(hash) * 2.0 - 1.0 // 把稳定单位值映射到[-1,1)。
}

#[cfg(test)]
mod tests {
    use std::sync::Arc; // 活动布局块在测试中也保持正式不可变共享语义。

    use bevy::prelude::Rect; // 构造活动布局块世界范围。

    use super::*; // 纯候选测试锁定窗口移动和负坐标确定性。

    #[test]
    fn candidates_are_stable_for_positive_and_negative_world_cells() {
        let source = VegetationCandidateSource::JitteredGrid {
            spacing: 2.0, // 每个世界格边长两米。
            jitter: 0.45, // 接近最大抖动以放大坐标错误。
            seed: 42,     // 固定层种子。
        };
        for cell in [IVec2::new(5, 7), IVec2::new(-5, -7), IVec2::new(-1, 0)] {
            let first = deterministic_candidate(&source, cell).unwrap();
            let second = deterministic_candidate(&source, cell).unwrap();
            assert_eq!(first, second); // 重算同格不得重新洗牌。
            let nominal_center = (cell.as_vec2() + Vec2::splat(0.5)) * 2.0;
            let actual = first.transform.translation.xz();
            assert!((actual - nominal_center).abs().max_element() <= 0.9); // 候选始终留在自己的世界格。
        }
    }

    #[test]
    fn stable_selector_does_not_depend_on_window_origin() {
        let world = Vec2::new(-100.25, 203.75); // 同一个绝对世界点。
        let direct = stable_world_selector(world, 0.5, 99); // 从世界坐标直接解析。
        let window_origin = Vec2::new(-128.0, 192.0); // 模拟相机窗口移动后的局部原点。
        let reconstructed = stable_world_selector(window_origin + (world - window_origin), 0.5, 99);
        assert_eq!(direct.to_bits(), reconstructed.to_bits()); // 窗口原点不得进入选择结果。
    }

    #[test]
    fn explicit_placements_keep_stable_ids_and_transforms() {
        let transform = Transform::from_xyz(3.0, 4.0, 5.0);
        let source = VegetationCandidateSource::ExplicitPlacements(vec![
            crate::ExplicitVegetationPlacement {
                instance_id: 77, // 离线稳定树位或岩石ID。
                transform,       // 保留项目给定的完整变换。
                density: 0.8,    // 单点可表达稀疏启用需求。
            },
        ]);
        let candidate = deterministic_candidate(&source, IVec2::ZERO).unwrap();
        assert_eq!(candidate.instance_id, 77);
        assert_eq!(candidate.transform, transform);
        assert_eq!(candidate.density, 0.8);
    }

    #[test]
    fn sequential_explicit_ids_receive_distinct_stable_thresholds() {
        let selectors = (0..16)
            .map(|instance_id| stable_instance_selector(instance_id, 5).to_bits())
            .collect::<std::collections::BTreeSet<_>>(); // 连续离线清单ID不能都落在零阈值。
        assert_eq!(selectors.len(), 16); // 重混合后的十六个阈值必须全部不同。
        assert_eq!(
            stable_instance_selector(7, 5).to_bits(),
            stable_instance_selector(7, 5).to_bits()
        ); // 同一层同一实例跨重算保持逐位一致。
        assert_ne!(
            stable_instance_selector(7, 5).to_bits(),
            stable_instance_selector(7, 6).to_bits()
        ); // 两层复用相同显式ID时由盐值解除相关性。
    }

    #[test]
    fn all_backends_share_the_same_resolved_candidate_decision() {
        let layer = VegetationLayerId(9);
        let tile_id = crate::VegetationDirtyTile(IVec2::ZERO);
        let mut layout = ActiveVegetationLayout {
            revision: 1,
            texel_size: 1.0,
            ..Default::default()
        };
        layout.tiles.insert(
            tile_id,
            Arc::new(crate::VegetationResolvedTile {
                coordinate: tile_id,
                world_bounds: Rect::from_corners(Vec2::ZERO, Vec2::splat(64.0)),
                layers: vec![crate::VegetationResolvedLayer {
                    layer,
                    density: vec![1.0; 64 * 64], // 满密度必须选择任意稳定候选。
                }],
            }),
        );
        let candidate = VegetationCandidate {
            instance_id: 123,
            transform: Transform::from_xyz(5.0, 0.0, 7.0),
            density: 1.0,
        };
        assert!(resolved_candidate_enabled(&layout, layer, &candidate));
        Arc::make_mut(layout.tiles.get_mut(&tile_id).unwrap()).layers[0]
            .density
            .fill(0.0); // 同一候选在零分配场必须被全部后端拒绝。
        assert!(!resolved_candidate_enabled(&layout, layer, &candidate));
    }
}
