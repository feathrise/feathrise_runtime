//! 只在构造／解码时校验坐标、带方向的闭壳邻接、正体积及自交。

use super::{EDITABLE_CUBOID_EDGES, MeshFace, spatial::TriangleIndex, triangulation::visit_face_triangles};
use bevy::math::DVec3;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroU32;

pub(super) struct DerivedMesh {
    pub(super) edges: Vec<[usize; 2]>,
    pub(super) edge_faces: Vec<[usize; 2]>,
    pub(super) triangles: Vec<[usize; 3]>,
    pub(super) surface: Vec<(usize, [usize; 3])>,
    pub(super) face_starts: Vec<usize>,
    pub(super) points: Vec<DVec3>,
    pub(super) bounds: ([i32; 3], [i32; 3]),
    pub(super) spatial: TriangleIndex,
}

pub(super) fn validate(vertices: &[[i32; 3]], faces: &[MeshFace]) -> Result<DerivedMesh, String> {
    let (points, render_points, bounds) = points(vertices)?;
    let (edges, edge_faces) = validate_topology(vertices.len(), faces)?;
    let mut triangles = Vec::with_capacity(2 * vertices.len() - 4);
    let mut surface = Vec::with_capacity(triangles.capacity());
    let mut face_starts = Vec::with_capacity(faces.len() + 1);
    let reference = points.iter().copied().sum::<DVec3>() / vertices.len() as f64;
    let mut volume = 0.0;
    for (face_index, face) in faces.iter().enumerate() {
        face_starts.push(triangles.len());
        visit_face_triangles(face, &points, |triangle| {
            let [a, b, c] = triangle.map(|index| points[index]);
            if (b - a).cross(c - a).length_squared() < 0.5 {
                return Err("可编辑网格不能包含退化三角形".into());
            }
            let [a, b, c] = triangle.map(|index| render_points[index]);
            if (b.as_dvec3() - a.as_dvec3()).cross(c.as_dvec3() - a.as_dvec3()).length_squared() == 0.0
                || (b - a).cross(c - a).length_squared() == 0.0
            {
                return Err("可编辑网格三角形在当前坐标精度下退化".into());
            }
            let [a, b, c] = triangle.map(|index| points[index] - reference);
            volume += a.dot(b.cross(c));
            triangles.push(triangle);
            surface.push((face_index, triangle));
            Ok(())
        })?;
    }
    face_starts.push(triangles.len());
    if volume <= 1.0 {
        return Err("可编辑网格闭壳体积退化或整体翻面".into());
    }
    let spatial = TriangleIndex::build(&triangles, &points, true)?;
    Ok(DerivedMesh { edges, edge_faces, triangles, surface, face_starts, points, bounds, spatial })
}

type LocalPoints = (Vec<DVec3>, Vec<bevy::math::Vec3>, ([i32; 3], [i32; 3]));

fn points(vertices: &[[i32; 3]]) -> Result<LocalPoints, String> {
    let mut seen = HashSet::with_capacity(vertices.len());
    let mut seen_render = HashSet::with_capacity(vertices.len());
    let mut points = Vec::with_capacity(vertices.len());
    let mut render_points = Vec::with_capacity(vertices.len());
    let [mut min, mut max] = [vertices[0]; 2];
    for &point in vertices {
        if point.iter().any(|value| !(-20_000_000..=20_000_000).contains(value)) {
            return Err("可编辑网格顶点必须位于本地各轴±10000米范围内".into());
        }
        let render = point.map(|value| value as f32 * 0.0005);
        if !seen.insert(point) || !seen_render.insert(render.map(f32::to_bits)) {
            return Err("可编辑网格顶点重合或在当前坐标精度下塌缩".into());
        }
        min = std::array::from_fn(|i| min[i].min(point[i]));
        max = std::array::from_fn(|i| max[i].max(point[i]));
        points.push(DVec3::from_array(point.map(f64::from)));
        render_points.push(bevy::math::Vec3::from_array(render));
    }
    Ok((points, render_points, (min, max)))
}

#[derive(Clone, Copy)]
struct EdgeUse {
    endpoints: [usize; 2],
    sides: [Option<NonZeroU32>; 2],
}

fn ordered(a: usize, b: usize) -> [usize; 2] { [a.min(b), a.max(b)] }

type EdgesWithFaces = (Vec<[usize; 2]>, Vec<[usize; 2]>);

fn validate_topology(count: usize, faces: &[MeshFace]) -> Result<EdgesWithFaces, String> {
    let mut canonical_faces = HashSet::with_capacity(faces.len());
    let expected_edges = count + faces.len() - 2;
    let mut edge_lookup = HashMap::with_capacity(expected_edges);
    let mut edges = Vec::<EdgeUse>::with_capacity(expected_edges);
    let mut incident = vec![Vec::new(); count];

    for (face_index, face) in faces.iter().enumerate() {
        face.validate(count)?;
        let mut canonical = face.vertices;
        canonical[..face.indices().len()].sort_unstable();
        if !canonical_faces.insert((face.vertex_count, canonical)) {
            return Err("可编辑网格不能包含重复面".into());
        }
        let side_index = NonZeroU32::new(face_index as u32 + 1).expect("作者面数低于u32上界");
        let boundary = face.indices();
        for (&from, &to) in boundary.iter().zip(boundary.iter().cycle().skip(1)) {
            let [from, to] = [from as usize, to as usize];
            let index = match edge_lookup.entry(ordered(from, to)) {
                std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    // 多于 V+F−2 的边不可能通过闭壳条件，及早退出也使坏输入遵守邻接预算。
                    if edges.len() == expected_edges {
                        return Err("当前编辑网格必须保持单一无孔闭壳".into());
                    }
                    let index = edges.len();
                    edges.push(EdgeUse { endpoints: [from, to], sides: [None; 2] });
                    incident[from].push(index);
                    incident[to].push(index);
                    *entry.insert(index)
                }
            };
            let edge = &mut edges[index];
            let direction = usize::from(edge.endpoints[0] != from);
            if edge.sides[direction].replace(side_index).is_some() {
                return Err("可编辑网格必须是每边双面且绕序一致的闭壳".into());
            }
        }
    }
    drop(canonical_faces);
    if incident.iter().any(Vec::is_empty) {
        return Err("可编辑网格不能保留无关联顶点".into());
    }
    if edges.iter().any(|edge| edge.sides.contains(&None)) {
        return Err("可编辑网格必须是每边双面且绕序一致的闭壳".into());
    }
    if expected_edges != edges.len() {
        return Err("当前编辑网格必须保持单一无孔闭壳".into());
    }
    // 耳切边界最多16点，理论边数也已由Euler约束；临时邻接与校验哈希可先释放。
    validate_connected(&incident, &edges)?;
    validate_one_rings(&incident, &edges, faces.len())?;
    drop(incident);

    let mut ordered_edges = Vec::with_capacity(edges.len());
    let mut ordered_faces = Vec::with_capacity(edges.len());
    let mut old_indices = [usize::MAX; EDITABLE_CUBOID_EDGES.len()];
    let mut push = |edge: EdgeUse, endpoints| {
        ordered_edges.push(endpoints);
        let mut sides = edge.sides.map(|face| face_index(face));
        if endpoints != edge.endpoints {
            sides.swap(0, 1); // 旧八点保留历史边方向，关联面仍与发布边的正反一致。
        }
        ordered_faces.push(sides);
    };
    // 旧八基础边保留原来在面遍历之前的特定顺序；其余边按照首次遇到的方向和面顺序。
    for (slot, endpoints) in EDITABLE_CUBOID_EDGES.into_iter().enumerate() {
        if let Some(&index) = edge_lookup.get(&endpoints) {
            old_indices[slot] = index;
            push(edges[index], endpoints);
        }
    }
    for (index, edge) in edges.into_iter().enumerate() {
        if !old_indices.contains(&index) {
            push(edge, edge.endpoints);
        }
    }
    Ok((ordered_edges, ordered_faces))
}

fn face_index(face: Option<NonZeroU32>) -> usize {
    (face.expect("已核对完整双面边").get() - 1) as usize
}

fn validate_connected(incident: &[Vec<usize>], edges: &[EdgeUse]) -> Result<(), String> {
    let mut reached = vec![false; incident.len()];
    reached[0] = true;
    let mut pending = vec![0];
    while let Some(vertex) = pending.pop() {
        for &edge in &incident[vertex] {
            let [a, b] = edges[edge].endpoints;
            let neighbor = if a == vertex { b } else { a };
            if !std::mem::replace(&mut reached[neighbor], true) {
                pending.push(neighbor);
            }
        }
    }
    if reached.contains(&false) {
        return Err("可编辑网格包含分离的闭壳".into());
    }
    Ok(())
}

fn validate_one_rings(incident: &[Vec<usize>], edges: &[EdgeUse], faces: usize) -> Result<(), String> {
    // 不为每个点扫描全部面。在该点的双面边上连通并查集；时间只随已登记的边/面邻接增长。
    let mut parent = vec![0; faces];
    let mut size = vec![0; faces];
    for adjoining in incident {
        for &edge in adjoining {
            for face in edges[edge].sides.map(face_index) {
                parent[face] = face;
                size[face] = 1;
            }
        }
        let mut joins = 0;
        for &edge in adjoining {
            let [a, b] = edges[edge].sides.map(face_index);
            let [mut a, mut b] = [root(&mut parent, a), root(&mut parent, b)];
            if a != b {
                if size[a] < size[b] {
                    std::mem::swap(&mut a, &mut b);
                }
                parent[b] = a;
                size[a] += size[b];
                joins += 1;
            }
        }
        // 闭合双面壳上，一个点的关联面个数等于关联边个数；一环必须只含一个连通分量。
        if joins + 1 != adjoining.len() {
            return Err("可编辑网格顶点的一环面必须连通，不能把多个壳粘在一点".into());
        }
    }
    Ok(())
}

fn root(parent: &mut [usize], mut face: usize) -> usize {
    while parent[face] != face {
        parent[face] = parent[parent[face]];
        face = parent[face];
    }
    face
}
