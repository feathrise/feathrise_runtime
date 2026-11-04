//! 以小片冻结真实primitive的位置、索引和源坐标变换；worker合并单一Trimesh，规划对象不参与碰撞。
use avian3d::prelude::{Collider, TrimeshFlags};
use bevy::{mesh::{Indices, VertexAttributeValues, PrimitiveTopology}, prelude::*};
use super::resources::CaptureBudget;
use super::resources::Identity;
use std::collections::BTreeSet;

struct Primitive {
    entity: Entity, mesh: Handle<Mesh>, matrix: Mat4, vertices: usize, indices: usize,
    vertex_cursor: usize, index_cursor: usize,
    positions: Vec<Vec<[f32; 3]>>, triangles: Vec<Vec<u32>>,
}
pub(super) struct Capture {
    pub identity: Identity,
    root_inverse: Mat4,
    stack: Vec<(Entity, usize)>,
    current: Option<Primitive>,
    complete: Vec<Primitive>,
    pub sources: BTreeSet<AssetId<Mesh>>,
    pub markers: Vec<Entity>, // 单帧最多64个新目标，原材质和网格不改写。
}
impl Capture {
    pub(super) fn new(identity: Identity, root_inverse: Mat4) -> Self {
        Self { identity, root_inverse, stack: vec![(identity.root, 0)], current: None, complete: Vec::new(), sources: BTreeSet::new(), markers: Vec::new() }
    }
    pub(super) fn set_root_inverse(&mut self, inverse: Mat4) { self.root_inverse = inverse; }
    pub(super) fn step(&mut self, meshes: &Assets<Mesh>, children: &Query<&Children>,
        visuals: &Query<(&Mesh3d, &GlobalTransform, Option<&crate::StaticMeshLodOriginal>)>, names: &Query<&Name>, budget: &mut CaptureBudget) -> Result<bool, String> {
        while budget.bytes >= 12 && budget.nodes > 0 {
            if let Some(current) = &mut self.current {
                let (_, global, _) = visuals.get(current.entity).map_err(|_| "GLB节点在捕获期间离场")?;
                if !(self.root_inverse * global.to_matrix()).abs_diff_eq(current.matrix, 0.001) { return Err("GLB内部节点变换在捕获期间换代".into()); }
                let mesh = meshes.get(&current.mesh).ok_or("GLB网格在捕获期间不可用")?;
                let Some(VertexAttributeValues::Float32x3(positions)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else { return Err("GLB网格POSITION必须为Float32x3".into()); };
                if positions.len() != current.vertices || mesh.indices().map_or(positions.len(), Indices::len) != current.indices { return Err("GLB拓扑在捕获期间换代".into()); }
                if current.vertex_cursor < current.vertices {
                    let end = (current.vertex_cursor + budget.bytes.min(64 * 1024) / 12).min(current.vertices);
                    current.positions.push(positions[current.vertex_cursor..end].to_vec());
                    budget.bytes -= (end - current.vertex_cursor) * 12; current.vertex_cursor = end;
                } else if current.index_cursor < current.indices {
                    let end = (current.index_cursor + budget.bytes.min(64 * 1024) / 4).min(current.indices);
                    let values = match mesh.indices() {
                        Some(Indices::U32(values)) => values[current.index_cursor..end].to_vec(),
                        Some(Indices::U16(values)) => values[current.index_cursor..end].iter().map(|v| u32::from(*v)).collect(),
                        None => (current.index_cursor..end).map(|v| v as u32).collect(),
                    };
                    current.triangles.push(values);
                    budget.bytes -= (end - current.index_cursor) * 4; current.index_cursor = end;
                } else { self.complete.push(self.current.take().unwrap()); }
                continue;
            }
            let Some(&(entity, next)) = self.stack.last() else { return Ok(true); };
            if next == 0 {
                budget.nodes -= 1;
                if names.get(entity).is_ok_and(|name| name.as_str().starts_with("规划_")) { self.stack.pop(); continue; }
                self.stack.last_mut().unwrap().1 = 1;
                if let Ok((handle, global, original)) = visuals.get(entity) {
                    let source = original.map_or(&handle.0, |original| &original.mesh); // LOD显示句柄不能成为新的完整碰撞或作者采样来源。
                    let mesh = meshes.get(source).ok_or("GLB依赖网格尚未驻留主世界")?;
                    if mesh.primitive_topology() != PrimitiveTopology::TriangleList { return Err("地形GLB碰撞只接受三角形primitive".into()); }
                    let vertices = mesh.count_vertices(); let indices = mesh.indices().map_or(vertices, Indices::len);
                    if vertices > u32::MAX as usize || indices % 3 != 0 { return Err("GLB位置或三角索引数量无效".into()); }
                    let matrix = self.root_inverse * global.to_matrix();
                    if !matrix.is_finite() || !matrix.inverse().is_finite() { return Err("GLB节点变换不可逆".into()); }
                    self.sources.insert(source.id());
                    self.markers.push(entity);
                    self.current = Some(Primitive { entity, mesh: source.clone(), matrix, vertices, indices,
                        vertex_cursor: 0, index_cursor: 0, positions: Vec::new(), triangles: Vec::new() });
                    continue;
                }
            }
            let child_index = self.stack.last().unwrap().1 - 1;
            if let Some(child) = children.get(entity).ok().and_then(|nodes| nodes.get(child_index)).copied() {
                self.stack.last_mut().unwrap().1 += 1; self.stack.push((child, 0));
            } else { self.stack.pop(); }
        }
        Ok(self.stack.is_empty() && self.current.is_none())
    }
    pub(super) fn build(self) -> Result<Collider, String> {
        let mut all_points = Vec::new(); let mut all_triangles = Vec::new();
        for primitive in self.complete {
            let points = primitive.positions.into_iter().flatten().map(|v| primitive.matrix.transform_point3(Vec3::from_array(v))).collect::<Vec<_>>();
            let flat = primitive.triangles.into_iter().flatten().collect::<Vec<_>>();
            if flat.is_empty() { continue; }
            if points.iter().any(|p| !p.is_finite()) || flat.iter().any(|i| *i as usize >= points.len()) { return Err(format!("GLB节点{:?}的碰撞索引或位置无效", primitive.entity)); }
            let offset = u32::try_from(all_points.len()).map_err(|_| "GLB碰撞顶点超过U32索引容量")?;
            if points.len() > (u32::MAX - offset) as usize { return Err("GLB碰撞顶点超过U32索引容量".into()); }
            all_triangles.extend(flat.chunks_exact(3).map(|v| [v[0] + offset, v[1] + offset, v[2] + offset]));
            all_points.extend(points);
        }
        if all_triangles.is_empty() { return Err("GLB地形没有可构建碰撞的三角形".into()); }
        // Parry禁止在Compound内嵌套TriMesh；只合并碰撞坐标，渲染网格和材质保持原样。
        let mut collider = Collider::try_trimesh_with_config(all_points, all_triangles, TrimeshFlags::FIX_INTERNAL_EDGES).map_err(|e| e.to_string())?;
        collider.set_scale(Vec3::from_array(self.identity.scale.map(f32::from_bits)), 10);
        Ok(collider)
    }
}
