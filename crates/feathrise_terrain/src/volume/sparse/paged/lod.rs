//! 体积表面复用高度地形的米制LOD环带；跨档边界保留原边，避免独立简化产生裂缝。
use crate::{TerrainMeshInput, volume::sparse::simplify::simplify_display_mesh};
use bevy::math::Vec3;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct VolumeDisplayLod {
    pub outer_half_extent_meters: f32,
    pub spacing_meters: f32,
}

/// 必须在体积计算worker调用；输入来自已版本绑定的精细/代理集合。
/// 源体素和缓存保持原精度，观察距离只改变本次派生几何。
pub fn apply_display_lods(
    mesh: TerrainMeshInput,
    bands: &[VolumeDisplayLod],
    foci: &[[f32; 3]],
    protected_radius: f32,
) -> Result<TerrainMeshInput, String> {
    if mesh.indices.len() % 3 != 0
        || mesh
            .indices
            .iter()
            .any(|i| *i as usize >= mesh.positions.len())
        || mesh
            .positions
            .iter()
            .any(|p| !Vec3::from_array(*p).is_finite())
    {
        return Err("体积LOD输入网格无效".into());
    }
    if bands.is_empty() {
        return Ok(mesh);
    }
    if foci.iter().any(|p| !Vec3::from_array(*p).is_finite())
        || bands.len() > 16
        || foci.is_empty()
        || foci.len() > 2
        || !protected_radius.is_finite()
        || protected_radius < 0.0
        || bands.iter().any(|b| {
            !b.outer_half_extent_meters.is_finite()
                || b.outer_half_extent_meters <= 0.0
                || !b.spacing_meters.is_finite()
                || b.spacing_meters <= 0.0
        })
        || bands.windows(2).any(|b| {
            b[0].outer_half_extent_meters > b[1].outer_half_extent_meters
                || b[0].spacing_meters > b[1].spacing_meters
        })
    {
        return Err("体积显示LOD计划无效".into());
    }
    let classify = |p: [f32; 3]| {
        let p = Vec3::from_array(p);
        if foci
            .iter()
            .any(|f| (p - Vec3::from_array(*f)).abs().max_element() <= protected_radius)
        {
            return 0;
        }
        let distance = foci
            .iter()
            .map(|f| {
                let d = (p - Vec3::from_array(*f)).abs();
                d.x.max(d.z)
            })
            .fold(f32::INFINITY, f32::min);
        bands
            .iter()
            .position(|b| distance <= b.outer_half_extent_meters)
            .unwrap_or(bands.len() - 1)
            + 1
    };
    let mut groups: Vec<Vec<u32>> = (0..=bands.len()).map(|_| Vec::new()).collect();
    for triangle in mesh.indices.chunks_exact(3) {
        // 三角跨档时使用更细一档；两个分区共享的顶点和一环随后由简化器锁定。
        let level = triangle
            .iter()
            .map(|i| classify(mesh.positions[*i as usize]))
            .min()
            .unwrap();
        groups[level].extend_from_slice(triangle);
    }
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    let mut welded = HashMap::new();
    for (level, group) in groups.into_iter().enumerate() {
        if group.is_empty() {
            continue;
        }
        let mut remap = HashMap::new();
        let mut local_positions = Vec::new();
        let mut local_indices = Vec::with_capacity(group.len());
        for index in group {
            let local = *remap.entry(index).or_insert_with(|| {
                let i = local_positions.len() as u32;
                local_positions.push(mesh.positions[index as usize]);
                i
            });
            local_indices.push(local);
        }
        let input = TerrainMeshInput {
            positions: local_positions,
            indices: local_indices,
        };
        let output = if level == 0 {
            input
        } else {
            simplify_display_mesh(input, bands[level - 1].spacing_meters / 16.0)?
        };
        let mut mapping = Vec::with_capacity(output.positions.len());
        for p in output.positions {
            // 消除正负零的不同位型；各档保留边坐标才能在同一聚合中重新焊接。
            let key = p.map(|v| if v == 0.0 { 0 } else { v.to_bits() });
            mapping.push(*welded.entry(key).or_insert_with(|| {
                let i = positions.len() as u32;
                positions.push(p);
                i
            }));
        }
        indices.extend(output.indices.into_iter().map(|i| mapping[i as usize]));
    }
    Ok(TerrainMeshInput { positions, indices })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protected_player_geometry_and_disconnected_cave_surfaces_survive_lod() {
        let input = TerrainMeshInput {
            positions: vec![
                [0., 0., 0.],
                [1., 0., 0.],
                [0., 0., 1.],
                [100., -4., 0.],
                [101., -4., 0.],
                [100., -4., 1.],
            ],
            indices: vec![0, 1, 2, 3, 4, 5],
        };
        let output = apply_display_lods(
            input,
            &[
                VolumeDisplayLod {
                    outer_half_extent_meters: 16.,
                    spacing_meters: 0.25,
                },
                VolumeDisplayLod {
                    outer_half_extent_meters: 1000.,
                    spacing_meters: 8.,
                },
            ],
            &[[0., 0., 0.]],
            2.,
        )
        .unwrap();
        assert_eq!(output.indices.len(), 6);
        assert!(output.positions.contains(&[100., -4., 0.]));
        assert!(output.positions.contains(&[1., 0., 0.]));
    }
    #[test]
    fn invalid_lod_order_is_rejected_before_derivation() {
        let input = TerrainMeshInput {
            positions: vec![],
            indices: vec![],
        };
        assert!(
            apply_display_lods(
                input,
                &[
                    VolumeDisplayLod {
                        outer_half_extent_meters: 32.,
                        spacing_meters: 2.
                    },
                    VolumeDisplayLod {
                        outer_half_extent_meters: 16.,
                        spacing_meters: 1.
                    }
                ],
                &[[0.; 3]],
                0.
            )
            .is_err()
        );
    }
    #[test]
    fn independently_simplified_bands_share_exact_seams() {
        let side = 33u32;
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for z in 0..side {
            for x in 0..side {
                positions.push([x as f32, 0., z as f32]);
            }
        }
        for z in 0..side - 1 {
            for x in 0..side - 1 {
                let a = z * side + x;
                indices.extend([a, a + side, a + 1, a + 1, a + side, a + side + 1]);
            }
        }
        let out = apply_display_lods(
            TerrainMeshInput { positions, indices },
            &[
                VolumeDisplayLod {
                    outer_half_extent_meters: 8.,
                    spacing_meters: 1.,
                },
                VolumeDisplayLod {
                    outer_half_extent_meters: 128.,
                    spacing_meters: 16.,
                },
            ],
            &[[16., 0., 16.]],
            2.,
        )
        .unwrap();
        let mut edges = std::collections::BTreeMap::new();
        for t in out.indices.chunks_exact(3) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        for ((a, b), count) in edges {
            assert!(count <= 2);
            if count == 1 {
                let a = out.positions[a as usize];
                let b = out.positions[b as usize];
                assert!(
                    (a[0] == 0. && b[0] == 0.)
                        || (a[0] == 32. && b[0] == 32.)
                        || (a[2] == 0. && b[2] == 0.)
                        || (a[2] == 32. && b[2] == 32.),
                    "LOD环内出现未焊接的裂边：{a:?} {b:?}"
                );
            }
        }
    }
}
