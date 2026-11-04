//! 洞内静水按统一世界格点生成上传块；分块尺度只约束单次上传，不限制覆盖面积。
use super::*;
use std::collections::BTreeMap;

const TILE_CELLS: i64 = 64;

pub(super) fn build(
    ground: ClipmapGroundSnapshot,
    layers: Vec<(Entity, ClipmapWaterSurface)>,
) -> Result<Vec<(Entity, Mesh)>, String> {
    let mut out = Vec::new();
    for (entity, layer) in layers {
        let origin = layer.world_min;
        let mask_step = (layer.world_max - origin)
            / (UVec2::from_array(layer.resolution) - UVec2::ONE).as_vec2();
        // 原生体素只派生附近窗口的水面；R8作者分辨率不能把运行时几何LOD重新放大到整层像素量。
        let step = if ground.is_density_surface() {
            ground.base_cell_size()
        } else {
            mask_step.min(ground.base_cell_size())
        };
        if !step.is_finite() || step.cmple(Vec2::ZERO).any() {
            return Err("洞内水面格距无效".into());
        }
        let (min, max) = layer.world_coverage_bounds();
        let mut tiles = BTreeMap::<_, Vec<_>>::new();
        for hole in ground.volume_footprints() {
            let a = Vec2::from_array(hole.min).max(min);
            let b = Vec2::from_array(hole.max).min(max);
            if a.cmpge(b).any() {
                continue;
            }
            let tile_min = ((a - origin) / step / TILE_CELLS as f32).floor();
            let tile_max = ((b - origin) / step / TILE_CELLS as f32).ceil();
            // 这里仅保护坐标格式，不再以面积、总顶点数或块数拒绝作者水面。
            if tile_min.min_element() < i64::MIN as f32 || tile_max.max_element() >= i64::MAX as f32
            {
                return Err("洞内水面格点超出坐标表示范围".into());
            }
            for z in tile_min.y as i64..tile_max.y as i64 {
                for x in tile_min.x as i64..tile_max.x as i64 {
                    tiles.entry((z, x)).or_default().push(hole);
                }
            }
        }
        for ((tz, tx), holes) in tiles {
            let mut positions = Vec::new();
            let mut uv = Vec::new();
            let mut depth = Vec::new();
            let mut indices = Vec::new();
            for z in 0..=TILE_CELLS {
                for x in 0..=TILE_CELLS {
                    // 由全局整数格点一次计算，不能从前块浮点终点累加，保证边界逐位一致。
                    let p = origin
                        + Vec2::new((tx * TILE_CELLS + x) as f32, (tz * TILE_CELLS + z) as f32)
                            * step;
                    let inside = holes.iter().any(|hole| hole.contains(p));
                    let floor = inside
                        .then(|| {
                            let origin = Vec3::new(p.x, layer.water_level, p.y);
                            ground
                                .three_dimensional_hit(origin, -Vec3::Y, 100000.)
                                .filter(|(_, normal)| normal.y > 0.05)
                                .or_else(|| {
                                    // 跨岸三角还需真实干岸顶点；只接受上方首个朝上面，不能穿过反向洞顶另找地面。
                                    ground
                                        .three_dimensional_hit(origin, Vec3::Y, 100000.)
                                        .filter(|(_, normal)| normal.y > 0.05)
                                })
                        })
                        .flatten()
                        .map(|(point, _)| point.y);
                    // known只表示同代真实河床完整；R8与正水深由片元裁切，安全水深只控制岸边波幅。
                    // 不能用顶点遮罩拒绝跨边缘三角，原生密度窗口的粗格还可能完全错过细R8非零纹素。
                    let valid = floor.is_some();
                    positions.push([p.x, layer.water_level, p.y]);
                    uv.push([p.x, p.y]);
                    depth.push([floor.unwrap_or(layer.water_level), f32::from(valid)]);
                }
            }
            let width = TILE_CELLS as u32 + 1;
            for z in 0..TILE_CELLS as u32 {
                for x in 0..TILE_CELLS as u32 {
                    let i = z * width + x;
                    for tri in [[i, i + width, i + 1], [i + 1, i + width, i + width + 1]] {
                        // 三个角点均需真实河床，至少一角有正水深；保留跨零岸线，整片干地不生成水面。
                        if tri.iter().all(|i| depth[*i as usize][1] > 0.5)
                            && tri.iter().any(|i| depth[*i as usize][0] < layer.water_level)
                        {
                            indices.extend(tri);
                        }
                    }
                }
            }
            if indices.is_empty() {
                continue;
            }
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0., 1., 0.]; positions.len()]);
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, depth);
            mesh.insert_indices(Indices::U32(indices));
            out.push((entity, mesh));
        }
    }
    Ok(out)
}
