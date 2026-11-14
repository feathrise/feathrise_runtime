//! 内置自由建模网格的后台空间分块与独立减面；完整作者源始终由调用方保留。
use super::{build::build_with_locks, types::{EditableMeshChunkLods, EditableMeshLodChunk, EditableMeshLodIndices}, StaticMeshLodView};
use bevy::{mesh::{Indices, PrimitiveTopology, VertexAttributeValues}, prelude::*};
use std::collections::HashMap;

/// 与当前作者格式的124三角上限配对；不把这个小网格路径用于无界GLB批处理。
const MAX_TRIANGLES: usize = 124;
// 作者源本来很小，过细切块会把几乎所有点都变成锁定边界，失去可简化的块内区域。
const CHUNK_TRIANGLES: usize = 64;

impl EditableMeshChunkLods {
    pub fn bytes(&self) -> usize {
        super::static_mesh_bytes(&self.source) + self.chunks.iter().map(|chunk|
            chunk.levels.iter().map(|level| level.indices.len() * 4).sum::<usize>()
        ).sum::<usize>()
    }

    /// 每个空间块用自己的保守球投影，不能以整个对象中心决定近端与远端的共同档位。
    pub fn select(&self, views: &[StaticMeshLodView], pose: &GlobalTransform,
        layers: &bevy::camera::visibility::RenderLayers, pixels: f32, hysteresis: f32,
        previous: &[usize]) -> Vec<usize> {
        self.chunks.iter().enumerate().map(|(index, chunk)| {
            let local = pose.mul_transform(Transform::from_translation(chunk.center));
            let mut selected = 0;
            for (level, candidate) in chunk.levels.iter().enumerate().skip(1) {
                let mut relevant = views.iter().filter(|view| view.layers.intersects(layers)).peekable();
                if relevant.peek().is_some() && relevant.all(|view| view.accepts(&local, chunk.radius,
                    candidate.max_error, pixels, hysteresis, previous.get(index) == Some(&level))) {
                    selected = level;
                }
            }
            selected
        }).collect()
    }

    /// 组合也在worker执行；一份Mesh原子换柄，避免子实体改变原来的拾取、材质和显隐树。
    pub fn compose(&self, selection: &[usize]) -> Result<Mesh, String> {
        if selection.len() != self.chunks.len() { return Err("分块LOD选择与来源不匹配".into()); }
        let mut indices = Vec::new();
        for (chunk, &level) in self.chunks.iter().zip(selection) {
            indices.extend_from_slice(&chunk.levels.get(level).ok_or("分块LOD档位已失效")?.indices);
        }
        let mut mesh = self.source.clone();
        mesh.insert_indices(Indices::U32(indices));
        Ok(mesh)
    }
}

pub fn build_editable_mesh_chunk_lods(source: Mesh) -> Result<EditableMeshChunkLods, String> {
    if source.primitive_topology() != PrimitiveTopology::TriangleList {
        return Err("内置分块LOD要求静态三角网格".into());
    }
    let Some(VertexAttributeValues::Float32x3(positions)) = source.attribute(Mesh::ATTRIBUTE_POSITION) else {
        return Err("内置分块LOD缺少位置".into());
    };
    let Some(VertexAttributeValues::Float32x3(normals)) = source.attribute(Mesh::ATTRIBUTE_NORMAL) else {
        return Err("内置分块LOD缺少法线".into());
    };
    if positions.len() != normals.len() || positions.len() > MAX_TRIANGLES * 3
        || positions.iter().chain(normals).flatten().any(|x| !x.is_finite()) {
        return Err("内置分块LOD来源超过作者容量或包含无效属性".into());
    }
    let indices: Vec<usize> = source.indices().map(|indices| indices.iter().collect())
        .unwrap_or_else(|| (0..positions.len()).collect());
    if indices.is_empty() || indices.len() % 3 != 0 || indices.len() > MAX_TRIANGLES * 3
        || indices.iter().any(|&index| index >= positions.len()) {
        return Err("内置分块LOD三角索引无效或超限".into());
    }
    // 只焊接位置及法线完全一致的点：硬边仍分离，不平滑挤出、折面或凹壳。
    let mut vertices = Vec::new();
    let mut welded_normals = Vec::new();
    let mut lookup = HashMap::new();
    let mut remap = Vec::with_capacity(positions.len());
    for (position, normal) in positions.iter().zip(normals) {
        let bits = |values: &[f32; 3]| values.map(|value| if value == 0.0 { 0 } else { value.to_bits() });
        let next = vertices.len() as u32;
        let index = *lookup.entry((bits(position), bits(normal))).or_insert_with(|| {
            vertices.push(*position);
            welded_normals.push(*normal);
            next
        });
        remap.push(index);
    }
    let triangles: Vec<[u32; 3]> = indices.chunks_exact(3)
        .map(|face| [remap[face[0]], remap[face[1]], remap[face[2]]]).collect();
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, source.asset_usage)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vertices.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, welded_normals);
    mesh.enable_raytracing = source.enable_raytracing;
    let mut groups = Vec::new();
    partition(&vertices, triangles, &mut groups);
    let mut chunks = Vec::with_capacity(groups.len());
    for triangles in groups {
        let indices: Vec<u32> = triangles.iter().flatten().copied().collect();
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for &index in &indices {
            let point = Vec3::from_array(vertices[index as usize]);
            min = min.min(point); max = max.max(point);
        }
        let mut input = mesh.clone();
        input.insert_indices(Indices::U32(indices.clone()));
        let mut edges = HashMap::<[u32; 2], usize>::new();
        for triangle in &triangles { for side in 0..3 {
            let [a, b] = [triangle[side], triangle[(side + 1) % 3]];
            *edges.entry([a.min(b), a.max(b)]).or_default() += 1;
        } }
        let mut locks = vec![false; vertices.len()];
        for ([a, b], count) in edges {
            if count == 1 { locks[a as usize] = true; locks[b as usize] = true; }
        }
        let mut levels = vec![EditableMeshLodIndices { indices, max_error: 0.0 }];
        // 共用成熟减面器的LockBorder；每块边界始终保留全部原始点，任何相邻档位都能闭合。
        for lod in build_with_locks(input, Some(&locks))? {
            let Some(VertexAttributeValues::Float32x3(points)) = lod.mesh.attribute(Mesh::ATTRIBUTE_POSITION) else { unreachable!() };
            let Some(VertexAttributeValues::Float32x3(normals)) = lod.mesh.attribute(Mesh::ATTRIBUTE_NORMAL) else { unreachable!() };
            let to_source: Vec<u32> = points.iter().zip(normals)
                .map(|(point, normal)| lookup[&(point.map(|v| if v == 0.0 { 0 } else { v.to_bits() }),
                    normal.map(|v| if v == 0.0 { 0 } else { v.to_bits() }))]).collect();
            levels.push(EditableMeshLodIndices {
                indices: lod.mesh.indices().unwrap().iter().map(|index| to_source[index]).collect(),
                max_error: lod.max_error,
            });
        }
        chunks.push(EditableMeshLodChunk { center: (min + max) * 0.5, radius: (max - min).length() * 0.5, levels });
    }
    Ok(EditableMeshChunkLods { source: mesh, chunks })
}

fn partition(vertices: &[[f32; 3]], mut triangles: Vec<[u32; 3]>, out: &mut Vec<Vec<[u32; 3]>>) {
    if triangles.len() <= CHUNK_TRIANGLES { out.push(triangles); return; }
    let center = |face: &[u32; 3]| face.iter().map(|&i| Vec3::from_array(vertices[i as usize])).sum::<Vec3>() / 3.0;
    let min = triangles.iter().map(center).fold(Vec3::splat(f32::INFINITY), Vec3::min);
    let max = triangles.iter().map(center).fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
    let extent = max - min;
    let axis = if extent.x >= extent.y && extent.x >= extent.z { 0 } else if extent.y >= extent.z { 1 } else { 2 };
    triangles.sort_by(|a, b| center(a)[axis].total_cmp(&center(b)[axis]));
    let right = triangles.split_off(triangles.len() / 2);
    partition(vertices, triangles, out);
    partition(vertices, right, out);
}
