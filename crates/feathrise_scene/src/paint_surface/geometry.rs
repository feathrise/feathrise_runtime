//! 由静态源网格后台构建 BVH；查询保持真实三角身份，显示 LOD 不参与作者锚点。

use std::{collections::{BTreeMap, HashMap, BTreeSet}, sync::Arc};
use bevy::prelude::*;
use sha2::{Digest, Sha256};
use feathrise_scene_content::ModelSurfaceAnchor;

// 预算限制整个后台冻结值；超限返回错误，不截断来源或使用显示 LOD 替代作者精度。
const MAX_SURFACE_BYTES: usize = 128 * 1024 * 1024;
pub(super) const MAX_BRUSH_CANDIDATES: usize = 131_072;

#[derive(Clone)]
pub struct MeshSurfacePrimitive {
    pub primitive: u32,
    pub positions: Arc<Vec<[f32; 3]>>,
    pub indices: Arc<Vec<u32>>,
    pub primitive_to_object: Mat4,
}

#[derive(Clone, Copy, Debug)]
pub struct SurfaceHit {
    pub anchor: ModelSurfaceAnchor,
    pub local_position: Vec3,
    pub local_normal: Vec3,
    pub position: Vec3,
    pub normal: Vec3,
    pub distance: f32,
}

pub(super) struct Triangle {
    pub anchor: (u32, u32),
    pub points: [Vec3; 3],
    pub normal: Vec3,
}

struct Node {
    min: Vec3,
    max: Vec3,
    start: usize,
    len: usize,
    children: Option<[usize; 2]>,
}

pub(super) struct Geometry {
    pub triangles: Vec<Triangle>,
    lookup: BTreeMap<(u32, u32), usize>,
    order: Vec<usize>,
    nodes: Vec<Node>,
    topology: [u8; 32],
    geometry_revision: [u8; 32],
}

#[derive(Clone)]
pub struct MeshSurfaceSnapshot(pub(super) Arc<Geometry>);

impl MeshSurfaceSnapshot {
    /// 完整验证、排序和索引构建只能在计算 worker 中执行；主帧只共享完成后的 Arc。
    pub fn build(mut primitives: Vec<MeshSurfacePrimitive>) -> Result<Self, String> {
        primitives.sort_by_key(|p| p.primitive);
        let mut digest = Sha256::new();
        let mut geometry_digest = Sha256::new();
        let mut triangles = Vec::new();
        let mut lookup = BTreeMap::new();
        let mut previous = None;
        let mut bytes = 0usize;
        for primitive in primitives {
            if previous == Some(primitive.primitive) || !valid_matrix(primitive.primitive_to_object)
                || primitive.indices.len() % 3 != 0 || primitive.positions.iter().any(|p| !Vec3::from_array(*p).is_finite()) {
                return Err("模型种植面来源、变换或 primitive 身份无效".into());
            }
            previous = Some(primitive.primitive);
            bytes = bytes.saturating_add(primitive.positions.len().saturating_mul(12))
                .saturating_add(primitive.indices.len().saturating_mul(4));
            let count = primitive.indices.len() / 3;
            // 三角、排序、查找和 BVH 的保守工作集信用，不在输入恶化时继续扩容。
            bytes = bytes.saturating_add(count.saturating_mul(192));
            if bytes > MAX_SURFACE_BYTES { return Err("模型种植面超过128MiB后台几何预算".into()); }
            digest.update(primitive.primitive.to_le_bytes());
            digest.update((primitive.positions.len() as u64).to_le_bytes());
            digest.update((primitive.indices.len() as u64).to_le_bytes());
            for index in primitive.indices.iter() { digest.update(index.to_le_bytes()); }
            // 锚点可在同拓扑顶点热更新后继续解析，但旧命中不能冒充新几何；两种版本分别计算。
            geometry_digest.update(primitive.primitive.to_le_bytes());
            for point in primitive.positions.iter() {
                for component in point { geometry_digest.update(component.to_bits().to_le_bytes()); }
            }
            for component in primitive.primitive_to_object.to_cols_array() {
                geometry_digest.update(component.to_bits().to_le_bytes());
            }
            for (triangle, indices) in primitive.indices.chunks_exact(3).enumerate() {
                let mut points = [Vec3::ZERO; 3];
                for i in 0..3 {
                    let source = primitive.positions.get(indices[i] as usize).ok_or("模型三角索引越界")?;
                    points[i] = primitive.primitive_to_object.transform_point3(Vec3::from_array(*source));
                    if !points[i].is_finite() { return Err("模型变换后的种植面坐标无效".into()); }
                }
                let Some(normal) = (points[1]-points[0]).cross(points[2]-points[0]).try_normalize() else { continue; };
                let anchor = (primitive.primitive, u32::try_from(triangle).map_err(|_| "模型三角编号溢出")?);
                lookup.insert(anchor, triangles.len());
                triangles.push(Triangle { anchor, points, normal });
            }
        }
        if triangles.is_empty() { return Err("模型没有可种植的静态三角面".into()); }
        let topology: [u8; 32] = digest.finalize().into();
        geometry_digest.update(topology);
        let mut geometry = Geometry { order: (0..triangles.len()).collect(), triangles, lookup,
            nodes: Vec::new(), topology, geometry_revision: geometry_digest.finalize().into() };
        let len = geometry.order.len();
        geometry.build_node(0, len);
        Ok(Self(Arc::new(geometry)))
    }

    /// 仅连接关系和 primitive 顺序参与摘要；同拓扑改顶点、改材质或变换不会丢失密度。
    pub fn topology(&self) -> [u8; 32] { self.0.topology }

    /// 内容几何版本包含顶点和primitive内部姿态，不含对象整体世界变换或显示LOD。
    pub fn geometry_revision(&self) -> [u8; 32] { self.0.geometry_revision }

    pub fn bounds(&self) -> (Vec3, Vec3) { (self.0.nodes[0].min, self.0.nodes[0].max) }

    pub fn ray_surface(&self, object_to_world: Mat4, origin: Vec3, direction: Vec3, maximum: f32) -> Option<SurfaceHit> {
        if !valid_matrix(object_to_world) || !origin.is_finite() || !maximum.is_finite() || maximum <= 0.0 { return None; }
        let direction = direction.try_normalize()?;
        let inverse = object_to_world.inverse();
        let local_origin = inverse.transform_point3(origin);
        // 不重新归一化，参数 t 仍为世界米，非均匀缩放也可以正确比较最近命中。
        let local_direction = inverse.transform_vector3(direction);
        let mut best = maximum;
        let mut result = None;
        let mut pending = vec![0usize];
        while let Some(index) = pending.pop() {
            let node = &self.0.nodes[index];
            if !ray_box(local_origin, local_direction, node.min, node.max, best) { continue; }
            if let Some(children) = node.children { pending.extend(children); continue; }
            for &index in &self.0.order[node.start..node.start + node.len] {
                let triangle = &self.0.triangles[index];
                if let Some((distance, barycentric)) = ray_triangle(local_origin, local_direction, triangle.points)
                    && distance >= 0.0 && distance <= best {
                    best = distance;
                    result = self.resolve(ModelSurfaceAnchor { primitive: triangle.anchor.0, triangle: triangle.anchor.1,
                        barycentric }, object_to_world).map(|mut hit| { hit.distance = distance; hit });
                }
            }
        }
        result
    }

    pub fn resolve(&self, anchor: ModelSurfaceAnchor, object_to_world: Mat4) -> Option<SurfaceHit> {
        anchor.validate().ok()?;
        if !valid_matrix(object_to_world) { return None; }
        let triangle = &self.0.triangles[*self.0.lookup.get(&(anchor.primitive, anchor.triangle))?];
        let [u, v] = anchor.barycentric;
        let local_position = triangle.points[0] * (1.0-u-v) + triangle.points[1]*u + triangle.points[2]*v;
        Some(SurfaceHit { anchor, local_position, local_normal: triangle.normal,
            position: object_to_world.transform_point3(local_position),
            normal: object_to_world.inverse().transpose().transform_vector3(triangle.normal).try_normalize()?, distance: 0.0 })
    }

    pub(super) fn triangle(&self, anchor: ModelSurfaceAnchor) -> Option<&Triangle> {
        self.0.lookup.get(&(anchor.primitive, anchor.triangle)).map(|&i| &self.0.triangles[i])
    }

    /// 先用 BVH 限定局部候选，再沿真实共享边连接；相邻平行楼板没有共享边，不能串层。
    pub(super) fn connected_triangles(&self, hit: &SurfaceHit, frame: Mat4, radius: f32) -> Result<Vec<usize>, String> {
        let inverse = frame.inverse();
        let halo = inverse.x_axis.truncate().abs()*radius + inverse.y_axis.truncate().abs()*radius
            + inverse.z_axis.truncate().abs()*radius;
        let min = hit.local_position-halo;
        let max = hit.local_position+halo;
        let mut pending = vec![0];
        let mut candidates = BTreeSet::new();
        let normal_matrix = inverse.transpose();
        while let Some(index) = pending.pop() {
            let node = &self.0.nodes[index];
            if node.max.cmplt(min).any() || node.min.cmpgt(max).any() { continue; }
            if let Some(children) = node.children { pending.extend(children); continue; }
            for &index in &self.0.order[node.start..node.start+node.len] {
                let triangle = &self.0.triangles[index];
                let Some(normal) = normal_matrix.transform_vector3(triangle.normal).try_normalize() else { continue; };
                if normal.dot(hit.normal) < 0.25 { continue; }
                let points = triangle.points.map(|p| frame.transform_point3(p));
                if closest_triangle(hit.position, points).distance_squared(hit.position) > radius*radius { continue; }
                candidates.insert(index);
                if candidates.len() > MAX_BRUSH_CANDIDATES { return Err("模型笔刷局部三角面超过后台工作预算".into()); }
            }
        }
        let seed = *self.0.lookup.get(&(hit.anchor.primitive, hit.anchor.triangle)).ok_or("模型命中锚点已失效")?;
        candidates.insert(seed);
        let mut edges: HashMap<([u32; 3], [u32; 3]), Vec<usize>> = HashMap::new();
        for &index in &candidates {
            let points = self.0.triangles[index].points.map(point_key);
            for edge in [[0,1], [1,2], [2,0]] {
                let a = points[edge[0]]; let b = points[edge[1]];
                let key = if a < b { (a,b) } else { (b,a) };
                edges.entry(key).or_default().push(index);
            }
        }
        let mut adjacency: HashMap<usize, Vec<usize>> = HashMap::new();
        for linked in edges.into_values().filter(|linked| linked.len() == 2) {
            adjacency.entry(linked[0]).or_default().push(linked[1]);
            adjacency.entry(linked[1]).or_default().push(linked[0]);
        }
        let mut connected = BTreeSet::from([seed]);
        let mut pending = vec![seed];
        while let Some(index) = pending.pop() {
            for &neighbor in adjacency.get(&index).into_iter().flatten() {
                if connected.insert(neighbor) { pending.push(neighbor); }
            }
        }
        Ok(connected.into_iter().collect())
    }
}

impl Geometry {
    fn build_node(&mut self, start: usize, len: usize) -> usize {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for &index in &self.order[start..start+len] {
            for point in self.triangles[index].points { min = min.min(point); max = max.max(point); }
        }
        let index = self.nodes.len();
        self.nodes.push(Node { min, max, start, len, children: None });
        if len > 8 {
            let extent = max-min;
            let axis = if extent.x >= extent.y && extent.x >= extent.z { 0 } else if extent.y >= extent.z { 1 } else { 2 };
            let triangles = &self.triangles;
            self.order[start..start+len].sort_unstable_by(|&a, &b| {
                let center = |index: usize| triangles[index].points.iter().map(|p| p[axis]).sum::<f32>();
                center(a).total_cmp(&center(b)).then_with(|| a.cmp(&b))
            });
            let split = len/2;
            let children = [self.build_node(start, split), self.build_node(start+split, len-split)];
            self.nodes[index].children = Some(children);
        }
        index
    }
}

fn point_key(point: Vec3) -> [u32; 3] { point.to_array().map(|v| if v == 0.0 { 0 } else { v.to_bits() }) }
pub(super) fn valid_matrix(frame: Mat4) -> bool { frame.is_finite() && frame.determinant().abs() > 1e-12 }

fn ray_box(origin: Vec3, direction: Vec3, min: Vec3, max: Vec3, maximum: f32) -> bool {
    let mut near = 0.0_f32; let mut far = maximum;
    for axis in 0..3 {
        if direction[axis].abs() < 1e-20 {
            if origin[axis] < min[axis] || origin[axis] > max[axis] { return false; }
        } else {
            let a = (min[axis]-origin[axis])/direction[axis];
            let b = (max[axis]-origin[axis])/direction[axis];
            near = near.max(a.min(b)); far = far.min(a.max(b));
            if near > far { return false; }
        }
    }
    true
}

fn ray_triangle(origin: Vec3, direction: Vec3, points: [Vec3; 3]) -> Option<(f32, [f32; 2])> {
    let e = points[1]-points[0]; let f = points[2]-points[0];
    let p = direction.cross(f); let det = e.dot(p);
    if det.abs() < 1e-12 { return None; }
    let offset = origin-points[0]; let u = offset.dot(p)/det;
    let q = offset.cross(e); let v = direction.dot(q)/det;
    if u < -1e-6 || v < -1e-6 || u+v > 1.000001 { return None; }
    Some((f.dot(q)/det, [u.max(0.0), v.max(0.0)]))
}

pub(super) fn closest_triangle(point: Vec3, [a,b,c]: [Vec3;3]) -> Vec3 {
    let ab=b-a; let ac=c-a; let ap=point-a;
    let d1=ab.dot(ap); let d2=ac.dot(ap);
    if d1<=0.0 && d2<=0.0 { return a; }
    let bp=point-b; let d3=ab.dot(bp); let d4=ac.dot(bp);
    if d3>=0.0 && d4<=d3 { return b; }
    let vc=d1*d4-d3*d2;
    if vc<=0.0 && d1>=0.0 && d3<=0.0 { return a+ab*(d1/(d1-d3)); }
    let cp=point-c; let d5=ab.dot(cp); let d6=ac.dot(cp);
    if d6>=0.0 && d5<=d6 { return c; }
    let vb=d5*d2-d1*d6;
    if vb<=0.0 && d2>=0.0 && d6<=0.0 { return a+ac*(d2/(d2-d6)); }
    let va=d3*d6-d5*d4;
    if va<=0.0 && d4-d3>=0.0 && d5-d6>=0.0 { return b+(c-b)*((d4-d3)/((d4-d3)+(d5-d6))); }
    a+ab*(vb/(va+vb+vc))+ac*(vc/(va+vb+vc))
}
