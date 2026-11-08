//! 体积作者材质的稳定ID解析与网格属性派生；世界XZ地表仍沿用原管线。
use bevy::render::storage::ShaderBuffer;
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};
use std::collections::{BTreeMap, HashMap};
pub const VOLUME_SOIL_KEY: u32 = 0x00fffffe;
pub const VOLUME_ROCK_KEY: u32 = 0x00ffffff;

/// 三维笔触按原始格坐标查询，不能依赖LOD网格保留下来的顶点。
/// 表项为五个uvec4：格键、真实位置、法线、稳定材质键、权重；最多八次探测。
pub fn build_volume_field_buffer<'a>(
    step: f32,
    samples: impl Iterator<Item = ([f32; 3], [f32; 3], &'a [(String, u8)])>,
) -> Result<ShaderBuffer, String> {
    build_volume_fields_with_puddles(step, samples.map(|(p, n, s)| (p, n, s, 0)))
}
/// 积水占用法线表项的空余w槽，不增加片元存储绑定或再次查询整张作者表。
pub fn build_volume_fields_with_puddles<'a>(
    step: f32,
    samples: impl Iterator<Item = ([f32; 3], [f32; 3], &'a [(String, u8)], u8)>,
) -> Result<ShaderBuffer, String> {
    if !step.is_finite() || step <= 0.0 {
        return Err("三维材质作者格距无效".into());
    }
    let values: Vec<_> = samples
        .filter(|(_, _, s, puddles)| !s.is_empty() || *puddles > 0)
        .map(|(p, n, s, puddles)| {
            let cell = (Vec3::from_array(p) / step).round().as_ivec3();
            let keys = UVec4::from_array(std::array::from_fn(|i| {
                s.get(i).map_or(0, |(id, _)| volume_surface_key(id))
            }));
            let weights = UVec4::from_array(std::array::from_fn(|i| {
                s.get(i)
                    .map_or(0, |(_, w)| (f32::from(*w) / 255.0).to_bits())
            }));
            [
                UVec4::new(cell.x as u32, cell.y as u32, cell.z as u32, 1),
                UVec4::new(p[0].to_bits(), p[1].to_bits(), p[2].to_bits(), 0),
                UVec4::new(
                    n[0].to_bits(),
                    n[1].to_bits(),
                    n[2].to_bits(),
                    (f32::from(puddles) / 255.0).to_bits(),
                ),
                keys,
                weights,
            ]
        })
        .collect();
    let mut capacity = values
        .len()
        .max(1)
        .checked_mul(2)
        .and_then(usize::checked_next_power_of_two)
        .ok_or("三维材质表容量溢出")?;
    loop {
        let rows = capacity
            .checked_mul(5)
            .and_then(|n| n.checked_add(1))
            .filter(|n| *n <= u32::MAX as usize)
            .ok_or("三维材质表超出32位Shader寻址格式")?;
        for seed in 0..16u32 {
            // 由实际CPU分配和设备storage-binding能力决定可用容量，取消旧8MiB内容总门。
            let mut table = Vec::new();
            table
                .try_reserve_exact(rows)
                .map_err(|e| format!("三维材质表内存分配失败：{e}"))?;
            table.resize(rows, UVec4::ZERO);
            table[0] = UVec4::new(capacity as u32 - 1, seed, 0, 0);
            let mut valid = true;
            for value in &values {
                let base = (value[0].x.wrapping_mul(73856093)
                    ^ value[0].y.wrapping_mul(19349663)
                    ^ value[0].z.wrapping_mul(83492791)
                    ^ seed)
                    & (capacity as u32 - 1);
                let mut placed = false;
                for probe in 0..8 {
                    let offset = 1 + (((base + probe) & (capacity as u32 - 1)) as usize) * 5;
                    if table[offset].w == 0 {
                        table[offset..offset + 5].copy_from_slice(value);
                        placed = true;
                        break;
                    }
                }
                if !placed {
                    valid = false;
                    break;
                }
            }
            if valid {
                let mut buffer = ShaderBuffer::from(table);
                buffer.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
                return Ok(buffer); // 完成字节只移交渲染世界，避免提取时克隆完整作者表。
            }
        }
        capacity = capacity.checked_mul(2).ok_or("三维材质表容量溢出")?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn volume_material_table_can_grow_beyond_legacy_eight_mebibyte_total() {
        let paint = vec![("cave-rock".into(), 255)];
        let buffer = build_volume_field_buffer(
            0.25,
            (0..70_000).map(|x| ([x as f32 * 0.25, -4., 0.], [0., 1., 0.], paint.as_slice())),
        )
        .unwrap();
        assert!(buffer.cast_slice::<u8>().unwrap().len() > 8 * 1024 * 1024);
    }
    #[test]
    fn puddles_share_the_original_grid_buffer_without_allocating_material_ids() {
        let buffer = build_volume_fields_with_puddles(
            0.25,
            std::iter::once(([1., -4., 1.], [0., 1., 0.], &[][..], 128)),
        )
        .unwrap();
        let bytes = buffer.cast_slice::<u8>().unwrap();
        let rows: Vec<[u32; 4]> = bytes
            .chunks_exact(16)
            .map(|b| {
                std::array::from_fn(|i| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap()))
            })
            .collect();
        let i = (1..rows.len())
            .step_by(5)
            .find(|i| rows[*i][3] == 1)
            .unwrap();
        assert_eq!(rows[i + 3], [0; 4]);
        assert_eq!(f32::from_bits(rows[i + 2][3]), 128. / 255.);
    }
    #[test]
    fn original_grid_paint_survives_a_mesh_without_interior_vertices() {
        let paint = vec![("cave-rock".into(), 255)];
        let buffer = build_volume_field_buffer(
            0.25,
            std::iter::once(([1., -4., 1.], [0., 1., 0.], paint.as_slice())),
        )
        .unwrap();
        let bytes = buffer.cast_slice::<u8>().unwrap();
        let rows: Vec<[u32; 4]> = bytes
            .chunks_exact(16)
            .map(|b| {
                std::array::from_fn(|i| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap()))
            })
            .collect();
        let index = (1..rows.len())
            .step_by(5)
            .find(|i| rows[*i][3] == 1)
            .unwrap();
        assert_eq!(rows[index], [4, (-16i32) as u32, 4, 1]);
        assert_eq!(rows[index + 3][0], volume_surface_key("cave-rock"));
        assert_eq!(f32::from_bits(rows[index + 4][0]), 1.0);
        let mesh = build_volume_surface_mesh(
            &[[0., -4., 0.], [8., -4., 0.], [0., -4., 8.]],
            &[0, 1, 2],
            &[[0., 1., 0.]; 3],
            |_, _| None,
            |_| Some(4.),
        )
        .unwrap();
        assert_eq!(mesh.count_vertices(), 3); // 笔触不要求为了着色重建几何细分。
        let bevy::mesh::VertexAttributeValues::Float32x4(colors) =
            mesh.attribute(Mesh::ATTRIBUTE_COLOR).unwrap()
        else {
            panic!("三维自动材质必须发布四槽权重")
        };
        assert!(colors.iter().all(|c| c.iter().sum::<f32>() > 0.99));
    }
    #[test]
    fn invalid_material_geometry_is_reported_before_indexing() {
        assert!(build_volume_surface_mesh(&[], &[0, 0, 0], &[], |_, _| None, |_| None).is_err());
    }
    #[test]
    fn stable_id_lookup_preserves_layers_after_catalog_reordering() {
        let a = build_volume_lookup(&[("rock".into(), 12), ("grass".into(), 1)], 2, 3).unwrap();
        let b = build_volume_lookup(&[("grass".into(), 1), ("rock".into(), 12)], 2, 3).unwrap();
        assert_eq!(a, b);
        assert!(
            a.iter()
                .any(|entry| entry.x == volume_surface_key("rock") && entry.y == 12)
        );
    }
}
/// 顶点UV的精确整数范围；解析表检查稳定ID碰撞，禁止静默错材质。
pub fn volume_surface_key(id: &str) -> u32 {
    let hash = id.as_bytes().iter().fold(2166136261u32, |h, b| {
        (h ^ u32::from(*b)).wrapping_mul(16777619)
    });
    1 + hash % 0x00fffffd
}
/// 最多八次探测；首项存mask/seed与自动岩土层，材质重排不改写作者块。
pub fn build_volume_lookup(
    entries: &[(String, u32)],
    soil: u32,
    rock: u32,
) -> Result<Vec<UVec4>, String> {
    let mut unique: BTreeMap<u32, (String, u32)> = BTreeMap::new();
    for (id, index) in entries {
        let key = volume_surface_key(id);
        if let Some((old, _)) = unique.get(&key) {
            if old != id {
                return Err(format!("三维材质稳定ID编码冲突：{old}与{id}"));
            }
        }
        unique.entry(key).or_insert((id.clone(), *index));
    }
    let mut capacity = (unique.len().max(1) * 2).next_power_of_two();
    loop {
        if capacity > 262144 {
            return Err("三维材质解析表超过4MiB预算".into());
        }
        for seed in 0..16u32 {
            let mut table = vec![UVec4::ZERO; capacity + 1];
            table[0] = UVec4::new(capacity as u32 - 1, seed, soil, rock);
            let mut success = true;
            for (&key, (_, index)) in &unique {
                let base = (key.wrapping_mul(2654435761) ^ seed) & (capacity as u32 - 1);
                let mut placed = false;
                for probe in 0..8 {
                    let slot = 1 + ((base + probe) & (capacity as u32 - 1)) as usize;
                    if table[slot].x == 0 {
                        table[slot] = UVec4::new(key, *index, 0, 0);
                        placed = true;
                        break;
                    }
                }
                if !placed {
                    success = false;
                    break;
                }
            }
            if success {
                return Ok(table);
            }
        }
        capacity *= 2;
    }
}
/// 在计算worker派生。每个三角共用四个ID，权重沿顶点连续插值；只在候选集合不同时分裂顶点。
pub fn build_volume_surface_mesh<'a>(
    positions: &[[f32; 3]],
    indices: &[u32],
    normals: &[[f32; 3]],
    sample: impl Fn(Vec3, Vec3) -> Option<&'a [(String, u8)]>,
    depth: impl Fn(Vec3) -> Option<f32>,
) -> Result<Mesh, String> {
    if positions.len() != normals.len()
        || indices.len() % 3 != 0
        || indices.iter().any(|i| *i as usize >= positions.len())
        || positions
            .iter()
            .chain(normals)
            .any(|p| !Vec3::from_array(*p).is_finite())
    {
        return Err("体积材质输入网格无效".into());
    }
    let mut out_positions = Vec::new();
    let mut out_normals = Vec::new();
    let mut uv0 = Vec::new();
    let mut uv1 = Vec::new();
    let mut colors = Vec::new();
    let mut out_indices = Vec::with_capacity(indices.len());
    let mut vertices: HashMap<(u32, [u32; 4]), u32> = HashMap::new();
    for triangle in indices.chunks_exact(3) {
        let values: Vec<Vec<(u32, f32)>> = triangle
            .iter()
            .map(|i| {
                let p = Vec3::from_array(positions[*i as usize]);
                let n = Vec3::from_array(normals[*i as usize]);
                let mut values: Vec<(u32, f32)> = sample(p, n).map_or_else(Vec::new, |s| {
                    s.iter()
                        .map(|(id, w)| (volume_surface_key(id), f32::from(*w) / 255.0))
                        .collect()
                });
                // 未涂满的地下部分仍混合自动岩土，不能漏出洞顶的XZ材质。
                if let Some(d) = depth(p).filter(|d| *d > 0.2) {
                    let rock = ((d - 1.0) / 2.0).clamp(0., 1.);
                    let remaining = (1. - values.iter().map(|v| v.1).sum::<f32>()).clamp(0., 1.);
                    let cover = (d / 0.5).clamp(0., 1.) * remaining;
                    if (1. - rock) * cover > 0. {
                        values.push((VOLUME_SOIL_KEY, (1. - rock) * cover));
                    }
                    if rock * cover > 0. {
                        values.push((VOLUME_ROCK_KEY, rock * cover));
                    }
                }
                values
            })
            .collect();
        let mut sums: BTreeMap<u32, f32> = BTreeMap::new();
        for value in &values {
            for &(id, w) in value {
                *sums.entry(id).or_default() += w;
            }
        }
        let mut sorted: Vec<_> = sums.into_iter().collect();
        sorted.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let keys = std::array::from_fn::<_, 4, _>(|i| sorted.get(i).map_or(0, |v| v.0));
        for (corner, &original) in triangle.iter().enumerate() {
            let index = if let Some(&index) = vertices.get(&(original, keys)) {
                index
            } else {
                let index = u32::try_from(out_positions.len())
                    .map_err(|_| "体积材质网格超出32位索引格式")?;
                vertices.insert((original, keys), index);
                out_positions.push(positions[original as usize]);
                out_normals.push(normals[original as usize]);
                uv0.push([keys[0] as f32, keys[1] as f32]);
                uv1.push([keys[2] as f32, keys[3] as f32]);
                let mut color = std::array::from_fn::<_, 4, _>(|i| {
                    values[corner]
                        .iter()
                        .find(|(id, _)| *id == keys[i])
                        .map_or(0.0, |v| v.1)
                });
                // 四槽截断只丢失细节，不能把已经覆盖的地下表面重新漏成屋顶XZ材质。
                let retained = color.iter().sum::<f32>();
                let total = values[corner].iter().map(|v| v.1).sum::<f32>().min(1.0);
                if retained > 0.0 {
                    for weight in &mut color {
                        *weight *= total / retained;
                    }
                }
                colors.push(color);
                index
            };
            out_indices.push(index);
        }
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, out_positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, out_normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv0);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, uv1);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(out_indices));
    Ok(mesh)
}
