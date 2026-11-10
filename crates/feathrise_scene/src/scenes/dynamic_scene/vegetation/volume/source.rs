//! 有界计算槽冻结三维种植作者点；此处只构造GPU源，不派生逐株渲染变换。
use super::clearance;
use bevy::prelude::*;
use feathrise_environment::vegetation::blade::GpuBladeVolumeSource;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
pub(super) struct Layer {
    pub(super) id: String,
    pub(super) image: Image,
    pub(super) cells: Arc<[[[f32; 4]; 3]]>,
    pub(super) count: u32,
    pub(super) step: f32,
    pub(super) min: Vec3,
    pub(super) max: Vec3,
}
/// 只派生环境窗口里的顶面种植；对象XZ密度继续保留原图，三维字段不被重采样写回。
pub(super) fn build_with_native_density(
    mut roots: Vec<(
        Arc<feathrise_terrain::volume::sparse::paged::VolumeFieldSnapshot>,
        Vec3,
    )>,
    ground: crate::ClipmapGroundSnapshot,
    clearances: Option<crate::BuildingVegetationClearanceSnapshot>,
    blade_spans: BTreeMap<String, f32>,
    native: Option<(
        Arc<super::super::resources::DynamicVegetationBuild>,
        BTreeMap<u64, String>,
    )>,
) -> Result<Vec<Layer>, String> {
    if let Some((source, names)) = &native {
        use feathrise_terrain::volume::sparse::paged::{VolumeFieldSample, VolumeFieldSnapshot};
        let step = ground.base_cell_size().max_element();
        let mut top = VolumeFieldSnapshot::empty(step);
        for hole in ground.volume_footprints() {
            let min = (Vec2::from_array(hole.min) / step).floor().as_ivec2();
            let max = (Vec2::from_array(hole.max) / step).ceil().as_ivec2();
            for z in min.y..=max.y {
                for x in min.x..=max.x {
                    let at = Vec2::new(x as f32, z as f32) * step;
                    let crate::ClipmapGroundHit::Hit(point) =
                        ground.sample_authoritative_surface(at)
                    else {
                        continue;
                    };
                    let Some((point, normal)) = ground.three_dimensional_hit(
                        point + Vec3::Y * step,
                        Vec3::NEG_Y,
                        step * 2.,
                    ) else {
                        continue;
                    };
                    if normal.y <= 0.25 {
                        continue;
                    }
                    let mut vegetation = Vec::new();
                    for layer in &source.blade_layers {
                        let Some(name) = names.get(&layer.runtime_layer_id) else {
                            continue;
                        };
                        let uv = (at - Vec2::from_array(source.world_min))
                            / (Vec2::from_array(source.world_max)
                                - Vec2::from_array(source.world_min));
                        if uv.cmplt(Vec2::ZERO).any() || uv.cmpgt(Vec2::ONE).any() {
                            continue;
                        }
                        let value = sample_density(uv, layer.resolution, &layer.density);
                        if value > 0 {
                            vegetation.push((name.clone(), value));
                        }
                    }
                    let key = (point / step).round().as_ivec3().to_array();
                    top.samples.entry(key).or_insert(VolumeFieldSample {
                        position: point.to_array(),
                        normal: normal.to_array(),
                        surface: Vec::new(),
                        vegetation,
                        puddles: 0,
                    });
                }
            }
        }
        // 三维笔触保持自己的精细作者格；GPU母格所有权优先选择可用的更细支持格，
        // 不把厘米笔触扩大成米级二维派生格，也不为相同草根创建两个所有者。
        roots.push((Arc::new(top), Vec3::ZERO));
    }
    build_core(roots, ground, clearances, blade_spans, native)
}
fn sample_density(uv: Vec2, resolution: [u32; 2], values: &[u8]) -> u8 {
    let last = (UVec2::from_array(resolution) - UVec2::ONE).as_vec2();
    let point = uv * last;
    let lo = point.floor().as_uvec2();
    let hi = (lo + UVec2::ONE).min(UVec2::from_array(resolution) - UVec2::ONE);
    let fraction = point - point.floor();
    let sample = |x: u32, z: u32| f32::from(values[(z * resolution[0] + x) as usize]);
    let a = sample(lo.x, lo.y) * (1. - fraction.x) + sample(hi.x, lo.y) * fraction.x;
    let b = sample(lo.x, hi.y) * (1. - fraction.x) + sample(hi.x, hi.y) * fraction.x;
    (a * (1. - fraction.y) + b * fraction.y).round() as u8
}
/// 工作量随作者格增长的分组、投射与像素打包全部在单个计算任务中完成。
#[cfg(test)]
pub(super) fn build(
    roots: Vec<(
        Arc<feathrise_terrain::volume::sparse::paged::VolumeFieldSnapshot>,
        Vec3,
    )>,
    ground: crate::ClipmapGroundSnapshot,
    clearances: Option<crate::BuildingVegetationClearanceSnapshot>,
    blade_spans: BTreeMap<String, f32>,
) -> Result<Vec<Layer>, String> {
    build_core(roots, ground, clearances, blade_spans, None)
}
fn build_core(
    roots: Vec<(
        Arc<feathrise_terrain::volume::sparse::paged::VolumeFieldSnapshot>,
        Vec3,
    )>,
    ground: crate::ClipmapGroundSnapshot,
    clearances: Option<crate::BuildingVegetationClearanceSnapshot>,
    blade_spans: BTreeMap<String, f32>,
    native: Option<(
        Arc<super::super::resources::DynamicVegetationBuild>,
        BTreeMap<u64, String>,
    )>,
) -> Result<Vec<Layer>, String> {
    let clearance = clearances.map(clearance::Index::new).transpose()?;
    let mut layers: BTreeMap<String, Vec<[f32; 4]>> = BTreeMap::new();
    let mut bounds: BTreeMap<String, (Vec3, Vec3)> = BTreeMap::new();
    let mut steps: BTreeMap<String, f32> = BTreeMap::new();
    let mut bytes = 0usize;
    for (fields, origin) in roots {
        let ids: BTreeSet<_> = fields
            .samples
            .values()
            .flat_map(|s| {
                s.vegetation
                    .iter()
                    .filter(|(_, p)| *p > 0)
                    .map(|(id, _)| id.clone())
            })
            .collect();
        if ids.is_empty() {
            continue;
        }
        let support = support_cells(&fields, &blade_spans, ids.len(), bytes)?;
        for (cell, sample) in &support {
            let p = Vec3::from_array(sample.position) + origin;
            let n = Vec3::from_array(sample.normal);
            // 雕刻移走原表面时不让旧种植点悬空；保留作者值，填回后可自动恢复。
            let Some((p, normal)) = reproject_support(&ground, p, n, fields.cell_size) else {
                continue;
            };
            if n.dot(normal) < 0.25 {
                continue;
            }
            if clearance
                .as_ref()
                .is_some_and(|c| c.excludes(p, fields.cell_size, &ground))
            {
                continue;
            }
            // 采样点和显示法线会随LOD微调；作者格中心、投影主轴及随机身份必须保持稳定。
            let center = Vec3::from_array(cell.map(|v| v as f32)) * fields.cell_size + origin;
            let a = n.abs();
            let axis = if a.x > a.y && a.x > a.z {
                0
            } else if a.y >= a.z {
                1
            } else {
                2
            };
            let mut key = center.x.to_bits().wrapping_mul(73856093)
                ^ center.y.to_bits().wrapping_mul(19349663)
                ^ center.z.to_bits().wrapping_mul(83492791);
            key = (key ^ (key >> 16)).wrapping_mul(2246822519);
            key = (key ^ (key >> 13)).wrapping_mul(3266489917);
            key ^= key >> 16;
            // 24位整数可由f32无损携带；不用NaN位型，避免纹理采样硬件规范化随机身份。
            let metadata = [
                center.x,
                center.y,
                center.z,
                (((key & 0x003fffff) << 2) | axis) as f32,
            ];
            // 空白仅表示概率为零；父格代理可以落在同一承托面的空白格，不能把它当成地形缺失。
            for id in &ids {
                let painted = sample
                    .vegetation
                    .iter()
                    .find(|(key, _)| key == id)
                    .map(|(_, p)| *p);
                let inherited = native
                    .as_ref()
                    .filter(|_| painted.is_none() && n.y > 0.25)
                    .and_then(|(source, names)| {
                        let crate::ClipmapGroundHit::Hit(top) =
                            ground.sample_authoritative_surface(p.xz())
                        else {
                            return None;
                        };
                        if top.distance(p) > fields.cell_size * 0.1 + 0.0001 {
                            return None;
                        }
                        let layer = source
                            .blade_layers
                            .iter()
                            .find(|layer| names.get(&layer.runtime_layer_id) == Some(id))?;
                        let uv = (p.xz() - Vec2::from_array(source.world_min))
                            / (Vec2::from_array(source.world_max)
                                - Vec2::from_array(source.world_min));
                        (uv.cmpge(Vec2::ZERO).all() && uv.cmple(Vec2::ONE).all())
                            .then(|| sample_density(uv, layer.resolution, &layer.density))
                    })
                    .unwrap_or(0);
                let density = painted.unwrap_or(inherited); // 精细支持晕边沿用对象R8；不会把背景草误擦成零。
                let values = layers.entry(id.clone()).or_default();
                values.push([p.x, p.y, p.z, f32::from(density) / 255.0]);
                values.push([normal.x, normal.y, normal.z, fields.cell_size]);
                values.push(metadata);
                let b = bounds.entry(id.clone()).or_insert((p, p));
                b.0 = b.0.min(p);
                b.1 = b.1.max(p);
                steps
                    .entry(id.clone())
                    .and_modify(|step| *step = step.max(fields.cell_size))
                    .or_insert(fields.cell_size);
                bytes += 48;
                if bytes > 8 * 1024 * 1024 {
                    return Err("当前三维植被源窗口超过8MiB上传预算".into());
                }
            }
        }
    }
    let mut upload_remaining = 8 * 1024 * 1024;
    layers
        .into_iter()
        .map(|(id, values)| {
            let count = values.len() as u32 / 3;
            let cells: Arc<[[[f32; 4]; 3]]> = values
                .chunks_exact(3)
                .map(|v| [v[0], v[1], v[2]])
                .collect::<Vec<_>>()
                .into();
            // 空间索引让各LOD在共同父格中心取密度，不在每根草或每个作者面各自抽签。
            let image = GpuBladeVolumeSource::image_from_cells(&cells, upload_remaining)?;
            upload_remaining -= image.data.as_ref().map_or(0, Vec::len);
            let (min, max) = bounds.remove(&id).unwrap();
            let step = steps.remove(&id).unwrap();
            Ok(Layer {
                id,
                image,
                cells,
                count,
                step,
                min,
                max,
            })
        })
        .collect()
}

/// 作者格中心在覆盖域外时，内侧半格仍可能承托草根；不能仅因中心射线落空而丢掉整格。
/// 只沿同一顶面的切平面移动不到半格，GPU随后按实际根裁剪；不把真正的洞内缺口填平。
fn reproject_support(
    ground: &crate::ClipmapGroundSnapshot,
    p: Vec3,
    n: Vec3,
    step: f32,
) -> Option<(Vec3, Vec3)> {
    let project = |p| ground.three_dimensional_hit(p + n * step * 2., -n, step * 4.);
    if let Some(hit) = project(p) {
        return Some(hit);
    }
    if n.y <= 0.25 || n.y.abs() < n.x.abs().max(n.z.abs()) {
        return None;
    }
    let closest = ground
        .volume_footprints()
        .iter()
        .filter_map(|hole| {
            let min = Vec2::from_array(hole.min);
            let max = Vec2::from_array(hole.max);
            let uv = p.xz().clamp(min, max);
            let delta = (uv - p.xz()).abs();
            (delta.max_element() > 0. && delta.max_element() < step * 0.5).then_some((
                delta.length_squared(),
                min,
                max,
                uv,
            ))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let (_, min, max, uv) = closest?;
    let inset = Vec2::splat(step * 0.001).min((max - min) * 0.25);
    let uv = uv.clamp(min + inset, max - inset);
    let offset = uv - p.xz();
    let point = Vec3::new(uv.x, p.y - n.xz().dot(offset) / n.y, uv.y);
    project(point)
}

/// 只补齐现有种植点周围一个最大父格宽度的零概率承托格。
/// 稀疏存档会删除擦空的作者格；补齐使宽代理仍遵守室外的父格抽样，实际地形缺口由后续BVH投射拒绝。
fn support_cells(
    fields: &feathrise_terrain::volume::sparse::paged::VolumeFieldSnapshot,
    spans: &BTreeMap<String, f32>,
    layer_count: usize,
    existing_bytes: usize,
) -> Result<BTreeMap<[i32; 3], feathrise_terrain::volume::sparse::paged::VolumeFieldSample>, String> {
    // 只局部扩展有种植的区域；整洞的地表/积水作者格不能无关地占满植被上传窗口。
    let mut cells: BTreeMap<_, _> = fields
        .samples
        .iter()
        .filter(|(_, s)| s.vegetation.iter().any(|(_, p)| *p > 0))
        .map(|(key, sample)| (*key, sample.clone()))
        .collect();
    let limit = (8 * 1024 * 1024 - existing_bytes) / (48 * layer_count);
    if cells.len() > limit {
        return Err("当前三维植被承托窗口超过上传预算".into());
    }
    for sample in fields.samples.values().filter(|s| !s.vegetation.is_empty()) {
        let span = sample
            .vegetation
            .iter()
            .filter_map(|(id, p)| (*p > 0).then(|| spans.get(id)).flatten())
            .copied()
            .fold(0.0_f32, f32::max);
        if span <= 0.0 {
            continue;
        }
        let n = Vec3::from_array(sample.normal);
        let a = n.abs();
        let axis = if a.x > a.y && a.x > a.z {
            0
        } else if a.y >= a.z {
            1
        } else {
            2
        };
        let uv = if axis == 0 {
            [1, 2]
        } else if axis == 2 {
            [0, 1]
        } else {
            [0, 2]
        };
        let position = Vec3::from_array(sample.position);
        let center = (position / fields.cell_size).round().as_ivec3();
        let halo_float = (span / fields.cell_size).ceil() + 1.0;
        if !halo_float.is_finite() || halo_float > (limit as f32).sqrt() * 0.5 {
            return Err("当前三维植被父格承托邻域超过上传预算".into());
        }
        let halo = halo_float as i32;
        // 索引与源仍受同一8MiB门约束；不以大LOD规格在后台分配无界二维邻域。
        if (halo as i64 * 2 + 1).pow(2) > limit as i64 {
            return Err("当前三维植被父格承托邻域超过上传预算".into());
        }
        for v in -halo..=halo {
            for u in -halo..=halo {
                let mut key = center;
                key[uv[0]] += u;
                key[uv[1]] += v;
                let mut p = key.as_vec3() * fields.cell_size;
                p[axis] = position[axis]
                    - (n[uv[0]] * (p[uv[0]] - position[uv[0]])
                        + n[uv[1]] * (p[uv[1]] - position[uv[1]]))
                        / n[axis];
                key[axis] = (p[axis] / fields.cell_size).round() as i32;
                if cells.contains_key(&key.to_array()) {
                    continue;
                }
                if cells.len() >= limit {
                    return Err("当前三维植被承托窗口超过上传预算".into());
                }
                let mut support = sample.clone();
                support.position = p.to_array();
                support.vegetation.clear();
                support.surface.clear();
                support.puddles = 0;
                if let Some(known) = fields.samples.get(&key.to_array()) {
                    if n.dot(Vec3::from_array(known.normal)) < 0.25 {
                        continue;
                    }
                    support = known.clone(); // 已知空白承托面的实际位置优先于邻面外推。
                }
                cells.insert(key.to_array(), support);
            }
        }
    }
    Ok(cells)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_xz_projection_keeps_fine_three_dimensional_author_cells() {
        use super::super::super::resources::{
            DynamicBladeLayerBuild, DynamicVegetationBuild, DynamicVegetationGroundSource,
        };
        use feathrise_terrain::volume::sparse::paged::{VolumeFieldSample, VolumeFieldSnapshot};
        let shape = avian3d::prelude::Collider::trimesh(
            vec![
                Vec3::new(0., 2., 0.),
                Vec3::new(4., 2., 0.),
                Vec3::new(0., 2., 4.),
                Vec3::new(4., 2., 4.),
            ],
            vec![[0, 2, 1], [1, 2, 3]],
        );
        let ground = crate::ClipmapGroundSnapshot::from_density_mesh(
            feathrise_scene_content::SceneId::parse("MAP-native-plant").unwrap(),
            Entity::from_bits(9),
            1,
            [5; 32],
            Some(shape),
            Vec3::ZERO,
            Vec3::new(4., 2., 4.),
            0.25,
            &[Vec3::new(2., 2., 2.)],
            2.,
        )
        .unwrap();
        let source = Arc::new(DynamicVegetationBuild {
            volume_mesh_layers: Default::default(),
            generation: 1,
            revision: 1,
            mesh_layers: Vec::new(),
            blade_layers: vec![DynamicBladeLayerBuild {
                runtime_layer_id: 7,
                resolution: [2; 2],
                density: Arc::from([255; 4]),
            }],
            ground: DynamicVegetationGroundSource::Clipmap,
            world_min: [0.; 2],
            world_max: [4.; 2],
        });
        let mut fields = VolumeFieldSnapshot::empty(0.01);
        fields.samples.insert(
            [100, 200, 100],
            VolumeFieldSample {
                position: [1., 2., 1.],
                normal: [0., 1., 0.],
                surface: Vec::new(),
                vegetation: vec![("grass".into(), 128)],
                puddles: 0,
            },
        );
        let fields = Arc::new(fields);
        let layers = build_with_native_density(
            vec![(fields.clone(), Vec3::ZERO)],
            ground,
            None,
            BTreeMap::new(),
            Some((source, BTreeMap::from([(7, "grass".into())]))),
        )
        .unwrap();
        assert_eq!(layers.len(), 1);
        assert!(
            layers[0]
                .cells
                .iter()
                .any(|cell| cell[1][3] == 0.01 && (cell[0][3] - 128. / 255.).abs() < 0.00001)
        );
        assert!(
            layers[0]
                .cells
                .iter()
                .any(|cell| cell[1][3] == 0.25 && cell[0][3] == 1.)
        );
        assert_eq!(fields.cell_size, 0.01);
        assert_eq!(fields.samples.len(), 1);
        assert_eq!(
            sample_density(Vec2::splat(0.5), [2; 2], &[0, 255, 255, 0]),
            128
        );
    }
    #[test]
    fn partial_border_cells_keep_their_interior_support_without_filling_real_holes() {
        let mut ground = crate::ClipmapGroundSnapshot::stacked_for_test();
        let shape = avian3d::prelude::Collider::trimesh(
            vec![
                Vec3::new(0., 2., 0.),
                Vec3::new(0.5, 2., 0.),
                Vec3::new(0., 2., 4.),
                Vec3::new(0.5, 2., 4.),
            ],
            vec![[0, 2, 1], [1, 2, 3]],
        );
        ground.terrain_meshes = Arc::from([
            crate::scenes::dynamic_scene::terrain_topology::TerrainMeshQuery::new(
                shape,
                Mat4::IDENTITY,
            )
            .unwrap(),
        ]);
        assert!(
            ground
                .three_dimensional_hit(Vec3::new(-0.07, 2.5, 1.), Vec3::NEG_Y, 1.)
                .is_none()
        );
        let mut fields = feathrise_terrain::volume::sparse::paged::VolumeFieldSnapshot::default();
        fields.cell_size = 0.25;
        fields.samples.insert(
            [1, 8, 4],
            feathrise_terrain::volume::sparse::paged::VolumeFieldSample {
                position: [0.25, 2., 1.],
                normal: Vec3::Y.to_array(),
                vegetation: vec![("grass".into(), 255)],
                surface: Vec::new(),
                puddles: 0,
            },
        );
        let layers = build(
            vec![(Arc::new(fields), Vec3::new(-0.07, 0., 0.))],
            ground.clone(),
            None,
            BTreeMap::from([("grass".into(), 1.)]),
        )
        .unwrap();
        assert!(
            layers[0]
                .cells
                .iter()
                .any(|c| c[2][0] == -0.07 && c[2][2] == 1. && c[0][0] > 0. && c[0][1] == 2.),
            "作者中心虽在域外，内侧半格也必须生成承托记录"
        );
        assert!(
            reproject_support(&ground, Vec3::new(2., 2., 1.), Vec3::Y, 0.25).is_none(),
            "覆盖域内部的真实缺口不能用边缘投射填平"
        );
        assert!(
            reproject_support(&ground, Vec3::new(-0.07, 8., 1.), Vec3::NEG_Y, 0.25).is_none(),
            "洞顶反面不能借室外顶面的边缘承托"
        );
    }
    #[test]
    fn sparse_density_keeps_zero_probability_support_for_parent_proxies() {
        let ground = crate::ClipmapGroundSnapshot::stacked_for_test();
        let mut fields = feathrise_terrain::volume::sparse::paged::VolumeFieldSnapshot::default();
        fields.cell_size = 0.25;
        fields.samples.insert(
            [0, 8, 0],
            feathrise_terrain::volume::sparse::paged::VolumeFieldSample {
                position: [0., 2., 0.],
                normal: [0., 1., 0.],
                surface: Vec::new(),
                vegetation: vec![("grass".into(), 96)],
                puddles: 0,
            },
        );
        let fields = Arc::new(fields);
        let layers = build(
            vec![(fields.clone(), Vec3::ZERO)],
            ground,
            None,
            BTreeMap::from([("grass".into(), 1.0)]),
        )
        .unwrap();
        let cells = &layers[0].cells;
        assert!(cells.len() > 1);
        assert_eq!(
            cells.iter().filter(|v| v[0][3] > 0.).count(),
            1,
            "晕边不能增加作者密度"
        );
        assert!(
            cells
                .iter()
                .any(|v| v[2][0] == 0.25 && v[2][2] == 0. && v[0][3] == 0.)
        );
        assert!(
            cells.iter().all(|v| v[0][1] == 2.),
            "零概率承托不能借用同XZ的上层洞顶"
        );
        assert!(support_cells(&fields, &BTreeMap::from([("grass".into(), 1000.)]), 1, 0).is_err());
        assert_eq!(fields.samples.len(), 1, "预算失败不能截断或改写作者数据");
    }

    #[test]
    fn reprojected_surface_positions_keep_the_author_cell_metadata() {
        let ground = crate::ClipmapGroundSnapshot::stacked_for_test();
        let build_at = |position| {
            let mut fields = feathrise_terrain::volume::sparse::paged::VolumeFieldSnapshot::default();
            fields.cell_size = 0.25;
            fields.samples.insert(
                [4, 8, 4],
                feathrise_terrain::volume::sparse::paged::VolumeFieldSample {
                    position,
                    normal: [0., 1., 0.],
                    surface: Vec::new(),
                    vegetation: vec![("grass".into(), 255)],
                    puddles: 0,
                },
            );
            build(
                vec![(Arc::new(fields), Vec3::ZERO)],
                ground.clone(),
                None,
                BTreeMap::new(),
            )
            .unwrap()
        };
        let a = build_at([1., 2., 1.]);
        let b = build_at([1.002, 2.002, 1.002]);
        assert_eq!(a[0].cells[0][2], b[0].cells[0][2]);
        assert_ne!(a[0].cells[0][0], b[0].cells[0][0]);
        let key = b[0].cells[0][2][3];
        assert!(key.is_finite() && key < 16777216. && key == (key as u32) as f32);
        assert_eq!(key as u32 & 3, 1, "投影主轴来自作者面，不能随显示法线切换");
    }
}
