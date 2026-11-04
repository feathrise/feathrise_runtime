//! 固定配方的静态网格减面：保边界、不剪枝、不跨材质primitive合并，失败保留原资产。
use bevy::{mesh::{Indices, PrimitiveTopology, VertexAttributeValues}, prelude::*};
use meshopt::{SimplifyOptions, VertexDataAdapter};
use super::{attributes::remap_attribute, types::StaticMeshLod};

/// 返回最多四层完整结果，不包含原始L0；仅供后台任务调用，不能在帧关键路径执行。
pub fn build_static_mesh_lods(source: Mesh) -> Result<Vec<StaticMeshLod>, String> {
    if source.primitive_topology() != PrimitiveTopology::TriangleList
        || source.try_contains_attribute(Mesh::ATTRIBUTE_JOINT_INDEX).unwrap_or(true)
        || source.try_contains_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT).unwrap_or(true)
        || source.try_has_morph_targets().unwrap_or(true)
    {
        return Err("普通网格LOD仅支持无骨骼、无形态目标的静态三角网格".to_owned());
    }
    let Some(VertexAttributeValues::Float32x3(positions)) = source.try_attribute_option(Mesh::ATTRIBUTE_POSITION)
        .map_err(|_| "静态网格CPU顶点数据已提取")? else {
        return Err("静态网格缺少三维位置".to_owned());
    };
    if positions.is_empty() || positions.len() > u32::MAX as usize
        || positions.iter().flatten().any(|x| !x.is_finite())
    {
        return Err("静态网格位置为空、超限或包含非有限值".to_owned());
    }
    for (_, values) in source.try_attributes().map_err(|_| "静态网格CPU属性已提取")? {
        if values.len() != positions.len() {
            return Err("静态网格各顶点属性数量不一致".to_owned());
        }
    }
    let indices: Vec<u32> = match source.try_indices_option().map_err(|_| "静态网格CPU索引已提取")? {
        Some(Indices::U32(values)) => values.clone(),
        Some(Indices::U16(values)) => values.iter().map(|&index| u32::from(index)).collect(),
        None => (0..positions.len() as u32).collect(),
    };
    if indices.is_empty() || indices.len() % 3 != 0 || indices.iter().any(|&i| i as usize >= positions.len()) {
        return Err("静态网格三角索引无效".to_owned());
    }
    let vertices = VertexDataAdapter::new(bytemuck::cast_slice(positions), 12, 0)
        .map_err(|_| "静态网格位置布局无效")?;
    let scale = meshopt::simplify_scale(&vertices);
    if !scale.is_finite() || scale <= 0.0 { return Ok(Vec::new()); }
    let (attributes, weights) = appearance_attributes(&source, positions.len())?;
    let locks = vec![false; positions.len()];
    let mut lods = Vec::with_capacity(4);
    let mut last_count = indices.len();
    // 每层从L0派生，误差不会由级联减面累积；目标比例是愿望，接缝和误差约束优先。
    for (ratio, relative_error) in [(0.5, 0.001), (0.25, 0.004), (0.125, 0.016), (0.0625, 0.064)] {
        let target = (((indices.len() as f32 * ratio) as usize) / 3 * 3).max(3);
        let mut error = 0.0;
        let simplified = meshopt::simplify_with_attributes_and_locks(
            &indices, &vertices, &attributes, &weights, weights.len() * 4, &locks,
            target, scale * relative_error,
            SimplifyOptions::LockBorder | SimplifyOptions::ErrorAbsolute, Some(&mut error),
        );
        if simplified.len() >= last_count { continue; }
        if simplified.is_empty() || simplified.len() % 3 != 0 || !error.is_finite() {
            return Err("静态网格简化未产生完整有效结果".to_owned());
        }
        let mut used = vec![false; positions.len()];
        for &index in &simplified { used[index as usize] = true; }
        let retained: Vec<usize> = used.iter().enumerate().filter_map(|(i, &used)| used.then_some(i)).collect();
        let mut remap = vec![0; positions.len()];
        for (new, &old) in retained.iter().enumerate() { remap[old] = new as u32; }
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, source.asset_usage);
        mesh.enable_raytracing = source.enable_raytracing;
        for (attribute, values) in source.attributes() {
            mesh.insert_attribute(*attribute, remap_attribute(values, &retained));
        }
        mesh.insert_indices(Indices::U32(simplified.iter().map(|&old| remap[old as usize]).collect()));
        last_count = simplified.len();
        // 数值误差也计入保守门槛；完全平面可零几何误差减面，但仍须满足纹理/法线约束。
        let previous_error = lods.last().map_or(0.0, |lod: &StaticMeshLod| lod.max_error);
        lods.push(StaticMeshLod { mesh, max_error: error.max(previous_error).max(0.0) });
    }
    Ok(lods)
}

fn appearance_attributes(source: &Mesh, count: usize) -> Result<(Vec<f32>, Vec<f32>), String> {
    let mut channels: Vec<(&VertexAttributeValues, usize, f32)> = Vec::new();
    for (attribute, dimensions, weight) in [
        (Mesh::ATTRIBUTE_NORMAL, 3, 0.25), (Mesh::ATTRIBUTE_TANGENT, 4, 0.25),
        (Mesh::ATTRIBUTE_UV_0, 2, 1.0), (Mesh::ATTRIBUTE_UV_1, 2, 0.5), (Mesh::ATTRIBUTE_COLOR, 4, 0.1),
    ] {
        if let Some(values) = source.attribute(attribute) {
            // 标准GLB通道使用Float32格式，异常格式不猜读；自定义通道仍通过同一索引完整保留。
            let valid = matches!((values, dimensions),
                (VertexAttributeValues::Float32x2(_), 2) | (VertexAttributeValues::Float32x3(_), 3)
                    | (VertexAttributeValues::Float32x4(_), 4));
            if !valid { return Err("静态网格标准外观属性格式无效".to_owned()); }
            channels.push((values, dimensions, weight));
        }
    }
    let weights: Vec<f32> = channels.iter().flat_map(|(_, dimensions, weight)| std::iter::repeat_n(*weight, *dimensions)).collect();
    let mut attributes = Vec::with_capacity(count * weights.len());
    for i in 0..count {
        for &(values, _, _) in &channels {
            let row: &[f32] = match values {
                VertexAttributeValues::Float32x2(values) => &values[i],
                VertexAttributeValues::Float32x3(values) => &values[i],
                VertexAttributeValues::Float32x4(values) => &values[i],
                _ => unreachable!(),
            };
            if row.iter().any(|x| !x.is_finite()) { return Err("静态网格外观属性包含非有限值".to_owned()); }
            attributes.extend_from_slice(row);
        }
    }
    Ok((attributes, weights))
}
