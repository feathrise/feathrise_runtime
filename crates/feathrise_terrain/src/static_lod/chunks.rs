//! 内置自由建模网格的后台空间分块与独立减面；完整作者源始终由调用方保留。
use super::{attributes::remap_attribute, build::build_with_locks, types::{EditableMeshChunkLods, EditableMeshLodChunk, EditableMeshLodIndices}, StaticMeshLodView};
use bevy::{mesh::{Indices, PrimitiveTopology, VertexAttributeValues}, prelude::*};
use std::collections::HashMap;

/// 覆盖一次100切的立方体；作者数据仍可更大，分块后台任务不因此扩大预算。
pub const MAX_EDITABLE_CHUNK_LOD_TRIANGLES: usize = 131_072;
/// 旧小网格允许对象查看时自动准备，密集分块由显式预览或运行时请求。
pub const SMALL_EDITABLE_CHUNK_LOD_TRIANGLES: usize = 124;
// 密集来源最多八块；小网格沿用两块，避免把内部点都锁成边界。
const DENSE_SPLIT_DEPTH: u8 = 3;

impl EditableMeshChunkLods {
    pub fn small_author_preview(&self) -> bool {
        self.chunks.iter().map(|chunk| chunk.levels[0].indices.len() / 3).sum::<usize>()
            <= SMALL_EDITABLE_CHUNK_LOD_TRIANGLES
    }

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
        // 只上传这组档位实际引用的顶点；全量共享来源留给下一次组合与原始回退。
        let mut remap = vec![u32::MAX; self.source.count_vertices()];
        let mut retained = Vec::new();
        for index in &mut indices {
            let old = *index as usize;
            if remap[old] == u32::MAX {
                remap[old] = retained.len() as u32;
                retained.push(old);
            }
            *index = remap[old];
        }
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, self.source.asset_usage);
        mesh.enable_raytracing = self.source.enable_raytracing;
        for (attribute, values) in self.source.attributes() {
            mesh.insert_attribute(*attribute, remap_attribute(values, &retained));
        }
        let indices = if retained.len() <= usize::from(u16::MAX) + 1 {
            Indices::U16(indices.into_iter().map(|index| index as u16).collect())
        } else {
            Indices::U32(indices)
        };
        mesh.insert_indices(indices);
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
    if positions.len() != normals.len() || positions.len() > MAX_EDITABLE_CHUNK_LOD_TRIANGLES * 3
        || positions.iter().chain(normals).flatten().any(|x| !x.is_finite()) {
        return Err("内置分块LOD来源超过作者容量或包含无效属性".into());
    }
    let indices: Vec<usize> = source.indices().map(|indices| indices.iter().collect())
        .unwrap_or_else(|| (0..positions.len()).collect());
    if indices.is_empty() || indices.len() % 3 != 0 || indices.len() > MAX_EDITABLE_CHUNK_LOD_TRIANGLES * 3
        || indices.iter().any(|&index| index >= positions.len()) {
        return Err("内置分块LOD三角索引无效或超限".into());
    }
    // 只焊接位置及法线完全一致的点：硬边仍分离，不平滑挤出、折面或凹壳。
    let mut vertices = Vec::new();
    let mut welded_normals = Vec::new();
    let mut lookup = HashMap::new();
    let mut remap = Vec::with_capacity(positions.len());
    for (position, normal) in positions.iter().zip(normals) {
        let next = vertices.len() as u32;
        let index = *lookup.entry((bits(*position), bits(*normal))).or_insert_with(|| {
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
    let dense = triangles.len() > SMALL_EDITABLE_CHUNK_LOD_TRIANGLES;
    let mut groups = Vec::new();
    partition(&vertices, triangles, if dense { 512 } else { 64 },
        if dense { DENSE_SPLIT_DEPTH } else { 1 }, &mut groups);
    let mut chunks = Vec::with_capacity(groups.len());
    for triangles in groups {
        let indices: Vec<u32> = triangles.iter().flatten().copied().collect();
        let mut local_ids = HashMap::new();
        let mut local_positions = Vec::new();
        let mut local_normals = Vec::new();
        let Some(VertexAttributeValues::Float32x3(normals)) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL) else {
            unreachable!("派生来源创建时已安装法线");
        };
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        // 每块只复制用得到的顶点。大网格不能为每块重复克隆和校验整个来源。
        let local_indices: Vec<u32> = indices.iter().map(|&global| {
            *local_ids.entry(global).or_insert_with(|| {
                let position = vertices[global as usize];
                let point = Vec3::from_array(position);
                min = min.min(point);
                max = max.max(point);
                let local = local_positions.len() as u32;
                local_positions.push(position);
                local_normals.push(normals[global as usize]);
                local
            })
        }).collect();
        let mut edges = HashMap::<[u32; 2], usize>::new();
        for triangle in local_indices.chunks_exact(3) { for side in 0..3 {
            let [a, b] = [triangle[side], triangle[(side + 1) % 3]];
            *edges.entry([a.min(b), a.max(b)]).or_default() += 1;
        } }
        let mut locks = vec![false; local_positions.len()];
        for ([a, b], count) in edges {
            if count == 1 { locks[a as usize] = true; locks[b as usize] = true; }
        }
        let input = Mesh::new(PrimitiveTopology::TriangleList, mesh.asset_usage)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, local_positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, local_normals)
            .with_inserted_indices(Indices::U32(local_indices));
        let mut levels = vec![EditableMeshLodIndices { indices, max_error: 0.0 }];
        // 共用成熟减面器的LockBorder；每块边界始终保留全部原始点，任何相邻档位都能闭合。
        for lod in build_with_locks(input, Some(&locks))? {
            let Some(VertexAttributeValues::Float32x3(points)) = lod.mesh.attribute(Mesh::ATTRIBUTE_POSITION) else { unreachable!() };
            let Some(VertexAttributeValues::Float32x3(normals)) = lod.mesh.attribute(Mesh::ATTRIBUTE_NORMAL) else { unreachable!() };
            let to_source: Vec<u32> = points.iter().zip(normals)
                .map(|(point, normal)| lookup[&(bits(*point), bits(*normal))]).collect();
            levels.push(EditableMeshLodIndices {
                indices: lod.mesh.indices().unwrap().iter().map(|index| to_source[index]).collect(),
                max_error: lod.max_error,
            });
        }
        chunks.push(EditableMeshLodChunk { center: (min + max) * 0.5, radius: (max - min).length() * 0.5, levels });
    }
    Ok(EditableMeshChunkLods { source: mesh, chunks })
}

fn bits(values: [f32; 3]) -> [u32; 3] {
    values.map(|value| if value == 0.0 { 0 } else { value.to_bits() })
}

fn partition(vertices: &[[f32; 3]], mut triangles: Vec<[u32; 3]>, target_size: usize,
    split_depth: u8, out: &mut Vec<Vec<[u32; 3]>>) {
    if triangles.len() <= target_size || split_depth == 0 {
        out.push(triangles);
        return;
    }
    let center = |face: &[u32; 3]| face.iter().map(|&i| Vec3::from_array(vertices[i as usize])).sum::<Vec3>() / 3.0;
    let min = triangles.iter().map(center).fold(Vec3::splat(f32::INFINITY), Vec3::min);
    let max = triangles.iter().map(center).fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
    let extent = max - min;
    let axis = if extent.x >= extent.y && extent.x >= extent.z { 0 } else if extent.y >= extent.z { 1 } else { 2 };
    triangles.sort_by(|a, b| center(a)[axis].total_cmp(&center(b)[axis]));
    let right = triangles.split_off(triangles.len() / 2);
    partition(vertices, triangles, target_size, split_depth - 1, out);
    partition(vertices, right, target_size, split_depth - 1, out);
}
