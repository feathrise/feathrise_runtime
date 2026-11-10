//! 64点/64面的有界编辑来源；旧八点来源共用几何视图、三角化、显示与碰撞。

use crate::{EDITABLE_CUBOID_EDGES, EDITABLE_CUBOID_FACES, ScenePlacementAssetTarget};
use bevy::math::{DVec2, DVec3};
use serde::{Deserialize, Serialize};

pub const MAX_EDITABLE_MESH_VERTICES: usize = 64;
pub const MAX_EDITABLE_MESH_FACES: usize = 64;
pub const MAX_EDITABLE_MESH_FACE_VERTICES: usize = 16;
pub const MAX_EDITABLE_MESH_EDGES: usize = 126;

/// 面只存作者边界；未使用索引清零，使Copy目标可作为精确缓存与事务身份。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MeshFace {
    pub vertices: [u8; MAX_EDITABLE_MESH_FACE_VERTICES],
    pub vertex_count: u8,
}

impl MeshFace {
    pub fn new(vertices: &[u8]) -> Result<Self, String> {
        if !(3..=MAX_EDITABLE_MESH_FACE_VERTICES).contains(&vertices.len()) {
            return Err("单个网格面需要3～16个边界顶点".into());
        }
        for (index, vertex) in vertices.iter().enumerate() {
            if vertices[..index].contains(vertex) {
                return Err("网格面不能重复使用同一顶点".into());
            }
        }
        let mut face = Self {
            vertices: [0; MAX_EDITABLE_MESH_FACE_VERTICES],
            vertex_count: vertices.len() as u8,
        };
        face.vertices[..vertices.len()].copy_from_slice(vertices);
        Ok(face)
    }

    pub fn indices(&self) -> &[u8] {
        &self.vertices[..usize::from(self.vertex_count).min(MAX_EDITABLE_MESH_FACE_VERTICES)]
    }
}

/// 固定容量使目标保持Copy；数量之外必须为零，避免隐藏垃圾形成额外缓存身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EditableMeshGeometry {
    #[serde(with = "array64")]
    pub vertices_half_mm: [[i32; 3]; MAX_EDITABLE_MESH_VERTICES],
    #[serde(with = "array64")]
    pub faces: [MeshFace; MAX_EDITABLE_MESH_FACES],
    pub vertex_count: u8,
    pub face_count: u8,
}

// serde原生数组实现只到32；JSON仍写标准定长数组，不引入另一种持久化格式。
mod array64 {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer, T: Serialize>(
        values: &[T; 64],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        values.as_slice().serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        deserializer: D,
    ) -> Result<[T; 64], D::Error> {
        let values = Vec::<T>::deserialize(deserializer)?;
        values.try_into().map_err(|values: Vec<T>| {
            serde::de::Error::custom(format!("固定数组需要64项，实际{}项", values.len()))
        })
    }
}

impl EditableMeshGeometry {
    pub fn from_cuboid_vertices(vertices: [[i32; 3]; 8]) -> Self {
        let mut geometry = Self {
            vertices_half_mm: [[0; 3]; MAX_EDITABLE_MESH_VERTICES],
            faces: [MeshFace::default(); MAX_EDITABLE_MESH_FACES],
            vertex_count: 8,
            face_count: 6,
        };
        geometry.vertices_half_mm[..8].copy_from_slice(&vertices);
        for (destination, face) in geometry.faces.iter_mut().zip(EDITABLE_CUBOID_FACES) {
            *destination = MeshFace::new(&face.map(|vertex| vertex as u8)).expect("内置面固定四点");
        }
        geometry
    }

    pub fn from_parts(vertices: &[[i32; 3]], faces: &[MeshFace]) -> Result<Self, String> {
        if vertices.len() > MAX_EDITABLE_MESH_VERTICES || faces.len() > MAX_EDITABLE_MESH_FACES {
            return Err("可编辑网格最多支持64个共享顶点和64个面，操作未采用".into());
        }
        let mut geometry = Self {
            vertices_half_mm: [[0; 3]; MAX_EDITABLE_MESH_VERTICES],
            faces: [MeshFace::default(); MAX_EDITABLE_MESH_FACES],
            vertex_count: vertices.len() as u8,
            face_count: faces.len() as u8,
        };
        geometry.vertices_half_mm[..vertices.len()].copy_from_slice(vertices);
        geometry.faces[..faces.len()].copy_from_slice(faces);
        geometry.validate()?;
        Ok(geometry)
    }

    /// 原始JSON数量可能非法；只读视图不会越界，正式使用仍须先validate。
    pub fn vertices(&self) -> &[[i32; 3]] {
        &self.vertices_half_mm[..usize::from(self.vertex_count).min(MAX_EDITABLE_MESH_VERTICES)]
    }

    pub fn faces(&self) -> &[MeshFace] {
        &self.faces[..usize::from(self.face_count).min(MAX_EDITABLE_MESH_FACES)]
    }

    /// 真实作者边只来自面边界；三角化对角线不进入选择，旧八点保持原12边编号。
    pub fn edges(&self) -> Vec<[usize; 2]> {
        let contains = |a, b| {
            self.faces().iter().any(|face| {
                let face = face.indices();
                (0..face.len()).any(|side| {
                    let x = usize::from(face[side]);
                    let y = usize::from(face[(side + 1) % face.len()]);
                    (x == a && y == b) || (x == b && y == a)
                })
            })
        };
        let mut edges = Vec::with_capacity(MAX_EDITABLE_MESH_EDGES);
        for edge in EDITABLE_CUBOID_EDGES {
            if contains(edge[0], edge[1]) {
                edges.push(edge);
            }
        }
        for face in self.faces() {
            let face = face.indices();
            for side in 0..face.len() {
                let edge = [
                    usize::from(face[side]),
                    usize::from(face[(side + 1) % face.len()]),
                ];
                if !edges
                    .iter()
                    .any(|existing| *existing == edge || *existing == [edge[1], edge[0]])
                {
                    edges.push(edge);
                }
            }
        }
        edges
    }

    /// 调用方先validate；坏输入整体返回空结果，不把失败多边形截断成部分可见表面。
    pub fn triangles(&self) -> Vec<[usize; 3]> {
        self.triangles_with_faces()
            .into_iter()
            .map(|(_, triangle)| triangle)
            .collect()
    }

    pub fn triangles_with_faces(&self) -> Vec<(usize, [usize; 3])> {
        self.checked_triangles().unwrap_or_default()
    }

    fn checked_triangles(&self) -> Result<Vec<(usize, [usize; 3])>, String> {
        let points = self
            .vertices()
            .iter()
            .map(|point| DVec3::from_array(point.map(f64::from)))
            .collect::<Vec<_>>();
        let mut triangles = Vec::with_capacity(self.vertices().len().saturating_mul(2));
        for (index, face) in self.faces().iter().enumerate() {
            triangles.extend(
                triangulate_face(face, &points)?
                    .into_iter()
                    .map(|triangle| (index, triangle)),
            );
        }
        Ok(triangles)
    }

    /// 新来源支持凹壳，采用流形边、连通性、正有向体积及自交检查。
    /// 上限先于二次自交检查：无孔闭壳最多124个三角，预算与场景规模无关。
    pub fn validate(&self) -> Result<(), String> {
        let vertices = usize::from(self.vertex_count);
        let faces = usize::from(self.face_count);
        if !(4..=MAX_EDITABLE_MESH_VERTICES).contains(&vertices)
            || !(4..=MAX_EDITABLE_MESH_FACES).contains(&faces)
        {
            return Err("可编辑网格需要4～64个共享顶点与4～64个面".into());
        }
        if self.vertices_half_mm[vertices..]
            .iter()
            .any(|value| *value != [0; 3])
            || self.faces[faces..]
                .iter()
                .any(|value| *value != MeshFace::default())
        {
            return Err("可编辑网格未使用的固定容量必须清零".into());
        }
        let points = self
            .vertices()
            .iter()
            .map(|point| DVec3::from_array(point.map(f64::from)))
            .collect::<Vec<_>>();
        let render_points = self
            .vertices()
            .iter()
            .map(|point| DVec3::from_array(point.map(|value| (value as f32 * 0.0005) as f64)))
            .collect::<Vec<_>>();
        for (index, point) in self.vertices().iter().enumerate() {
            if point
                .iter()
                .any(|value| !(-20_000_000..=20_000_000).contains(value))
            {
                return Err("可编辑网格顶点必须位于本地各轴±10000米范围内".into());
            }
            if self.vertices()[..index].contains(point)
                || render_points[..index].contains(&render_points[index])
            {
                return Err("可编辑网格顶点重合或在当前坐标精度下塌缩".into());
            }
        }
        let mut used = [false; MAX_EDITABLE_MESH_VERTICES];
        for (index, face) in self.faces().iter().enumerate() {
            let count = usize::from(face.vertex_count);
            if !(3..=MAX_EDITABLE_MESH_FACE_VERTICES).contains(&count)
                || face.vertices[count..].iter().any(|value| *value != 0)
            {
                return Err("网格面需要3～16个索引，未使用索引必须清零".into());
            }
            for (corner, vertex) in face.indices().iter().enumerate() {
                if usize::from(*vertex) >= vertices || face.indices()[..corner].contains(vertex) {
                    return Err("网格面索引无效或重复".into());
                }
                used[usize::from(*vertex)] = true;
            }
            let mut canonical = face.indices().to_vec();
            canonical.sort();
            if self.faces()[..index].iter().any(|previous| {
                let mut previous = previous.indices().to_vec();
                previous.sort();
                previous == canonical
            }) {
                return Err("可编辑网格不能包含重复面".into());
            }
        }
        if used[..vertices].contains(&false) {
            return Err("可编辑网格不能保留无关联顶点".into());
        }
        let edges = self.edges();
        if edges.len() > MAX_EDITABLE_MESH_EDGES {
            return Err("可编辑网格最多支持126条真实边".into());
        }
        for &[a, b] in &edges {
            let mut forward = 0;
            let mut backward = 0;
            for face in self.faces() {
                let face = face.indices();
                for side in 0..face.len() {
                    let edge = [
                        usize::from(face[side]),
                        usize::from(face[(side + 1) % face.len()]),
                    ];
                    if edge == [a, b] {
                        forward += 1;
                    }
                    if edge == [b, a] {
                        backward += 1;
                    }
                }
            }
            if forward != 1 || backward != 1 {
                return Err("可编辑网格必须是每边双面且绕序一致的闭壳".into());
            }
        }
        if vertices as isize - edges.len() as isize + faces as isize != 2 {
            return Err("当前编辑网格必须保持单一无孔闭壳".into());
        }
        let mut reached = [false; MAX_EDITABLE_MESH_VERTICES];
        reached[0] = true;
        for _ in 0..vertices {
            for &[a, b] in &edges {
                if reached[a] || reached[b] {
                    reached[a] = true;
                    reached[b] = true;
                }
            }
        }
        if reached[..vertices].contains(&false) {
            return Err("可编辑网格包含分离的闭壳".into());
        }
        // 仅检查边与Euler仍会漏掉在两个点上粘合的双壳；每个顶点的一环面也必须连成单环。
        for vertex in 0..vertices {
            let incident = self
                .faces()
                .iter()
                .filter_map(|face| {
                    let face = face.indices();
                    let corner = face
                        .iter()
                        .position(|index| usize::from(*index) == vertex)?;
                    Some([
                        face[(corner + face.len() - 1) % face.len()],
                        face[(corner + 1) % face.len()],
                    ])
                })
                .collect::<Vec<_>>();
            let mut reached = [false; MAX_EDITABLE_MESH_FACES];
            reached[0] = true;
            let mut pending = vec![0];
            while let Some(current) = pending.pop() {
                for next in 0..incident.len() {
                    if !reached[next]
                        && incident[current]
                            .iter()
                            .any(|neighbor| incident[next].contains(neighbor))
                    {
                        reached[next] = true;
                        pending.push(next);
                    }
                }
            }
            if reached[..incident.len()].contains(&false) {
                return Err("可编辑网格顶点的一环面必须连通，不能把多个壳粘在一点".into());
            }
        }
        let triangles = self.checked_triangles()?;
        let reference = points.iter().copied().sum::<DVec3>() / vertices as f64;
        let mut volume = 0.0;
        for (_, triangle) in &triangles {
            let [a, b, c] = triangle.map(|index| points[index]);
            if (b - a).cross(c - a).length_squared() < 0.5 {
                return Err("可编辑网格不能包含退化三角形".into());
            }
            let [a, b, c] = triangle.map(|index| render_points[index]);
            if (b - a).cross(c - a).length_squared() == 0.0 {
                return Err("可编辑网格三角形在当前坐标精度下退化".into());
            }
            let [a, b, c] = triangle.map(|index| render_points[index].as_vec3());
            if (b - a).cross(c - a).length_squared() == 0.0 {
                return Err("可编辑网格三角形在实际渲染运算精度下退化".into());
            }
            let [a, b, c] = triangle.map(|index| points[index] - reference);
            volume += a.dot(b.cross(c));
        }
        if volume <= 1.0 {
            return Err("可编辑网格闭壳体积退化或整体翻面".into());
        }
        let bounds = triangles
            .iter()
            .map(|(_, triangle)| {
                let [a, b, c] = triangle.map(|index| points[index]);
                (a.min(b).min(c), a.max(b).max(c))
            })
            .collect::<Vec<_>>();
        for (index, (_, first)) in triangles.iter().enumerate() {
            for (second_index, (_, second)) in triangles[..index].iter().enumerate() {
                let (a_min, a_max) = bounds[index];
                let (b_min, b_max) = bounds[second_index];
                if a_min.cmpgt(b_max).any() || b_min.cmpgt(a_max).any() {
                    continue;
                } // 局部包围盒先排除远离的面片，固定上限也避免无谓精确相交。
                if crate::editable_cuboid::triangles_intersect_illegally(*first, *second, &points) {
                    return Err("可编辑网格表面不能相互穿透或重叠".into());
                }
            }
        }
        Ok(())
    }
}

/// 三四边面保持历史固定对角线与非平面语义；更多边面用投影耳切保留共线边点。
/// 边界不能自交，超过半毫米量化容差的非共面多边形明确拒绝。
fn triangulate_face(face: &MeshFace, points: &[DVec3]) -> Result<Vec<[usize; 3]>, String> {
    let indices = face
        .indices()
        .iter()
        .map(|vertex| usize::from(*vertex))
        .collect::<Vec<_>>();
    if indices.len() < 3 || indices.iter().any(|index| *index >= points.len()) {
        return Err("网格面索引无效".into());
    }
    if indices.len() == 3 {
        return Ok(vec![[indices[0], indices[1], indices[2]]]);
    }
    if indices.len() == 4 {
        return Ok(vec![
            [indices[0], indices[1], indices[2]],
            [indices[0], indices[2], indices[3]],
        ]);
    }
    let origin = points[indices[0]];
    let local = indices
        .iter()
        .map(|index| points[*index] - origin)
        .collect::<Vec<_>>();
    let normal = (0..local.len())
        .map(|index| local[index].cross(local[(index + 1) % local.len()]))
        .sum::<DVec3>()
        .try_normalize()
        .ok_or("网格多边面退化")?;
    if local.iter().any(|point| point.dot(normal).abs() > 0.75) {
        return Err("五边及以上网格面必须共面，请先三角化或拆分该面".into());
    }
    let axis = normal.abs().max_element();
    let projected = local
        .iter()
        .map(|point| {
            if normal.x.abs() == axis {
                DVec2::new(point.y, point.z)
            } else if normal.y.abs() == axis {
                DVec2::new(point.x, point.z)
            } else {
                DVec2::new(point.x, point.y)
            }
        })
        .collect::<Vec<_>>();
    let cross = |a: DVec2, b: DVec2| a.x * b.y - a.y * b.x;
    let area = (0..projected.len())
        .map(|index| cross(projected[index], projected[(index + 1) % projected.len()]))
        .sum::<f64>();
    if area.abs() <= 1e-7 {
        return Err("网格多边面投影退化".into());
    }
    let orientation = area.signum();
    for first in 0..projected.len() {
        let next = (first + 1) % projected.len();
        for second in 0..first {
            let after = (second + 1) % projected.len();
            if next == second || after == first {
                continue;
            }
            if segments_intersect(
                projected[first],
                projected[next],
                projected[second],
                projected[after],
            ) {
                return Err("网格多边面边界不能自交或重叠".into());
            }
        }
    }
    let mut remaining = (0..indices.len()).collect::<Vec<_>>();
    let mut output = Vec::with_capacity(indices.len() - 2);
    while remaining.len() > 3 {
        let mut ear = None;
        for corner in 0..remaining.len() {
            let a = remaining[(corner + remaining.len() - 1) % remaining.len()];
            let b = remaining[corner];
            let c = remaining[(corner + 1) % remaining.len()];
            if cross(projected[b] - projected[a], projected[c] - projected[b]) * orientation <= 1e-7
            {
                continue;
            }
            let contains = remaining
                .iter()
                .copied()
                .filter(|index| *index != a && *index != b && *index != c)
                .any(|index| {
                    let p = projected[index];
                    cross(projected[b] - projected[a], p - projected[a]) * orientation >= -1e-7
                        && cross(projected[c] - projected[b], p - projected[b]) * orientation
                            >= -1e-7
                        && cross(projected[a] - projected[c], p - projected[c]) * orientation
                            >= -1e-7
                });
            if !contains {
                ear = Some((corner, [indices[a], indices[b], indices[c]]));
                break;
            }
        }
        let (corner, triangle) = ear.ok_or("网格多边面无法形成无退化三角，请拆分该面")?;
        output.push(triangle);
        remaining.remove(corner);
    }
    output.push([
        indices[remaining[0]],
        indices[remaining[1]],
        indices[remaining[2]],
    ]);
    Ok(output)
}

fn segments_intersect(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> bool {
    let cross = |a: DVec2, b: DVec2| a.x * b.y - a.y * b.x;
    let signs = [
        cross(b - a, c - a),
        cross(b - a, d - a),
        cross(d - c, a - c),
        cross(d - c, b - c),
    ];
    if signs[0] * signs[1] < -1e-7 && signs[2] * signs[3] < -1e-7 {
        return true;
    }
    [
        (signs[0], c, a, b),
        (signs[1], d, a, b),
        (signs[2], a, c, d),
        (signs[3], b, c, d),
    ]
    .iter()
    .any(|(sign, p, start, end)| sign.abs() <= 1e-7 && (*p - *start).dot(*p - *end) <= 1e-7)
}

impl ScenePlacementAssetTarget {
    /// 元素变换只改变坐标时保留八点来源；拓扑变化或已升级的对象继续使用完整网格来源。
    pub fn with_editable_geometry(self, geometry: EditableMeshGeometry) -> Result<Self, String> {
        geometry.validate()?;
        let (color_srgb, roughness_milli, collision) = match self {
            Self::Cuboid {
                color_srgb,
                roughness_milli,
                collision,
                ..
            }
            | Self::EditableCuboid {
                color_srgb,
                roughness_milli,
                collision,
                ..
            }
            | Self::EditableMesh {
                color_srgb,
                roughness_milli,
                collision,
                ..
            } => (color_srgb, roughness_milli, collision),
            _ => return Err("当前对象不支持网格编辑".into()),
        };
        let old_faces = Self::DEFAULT_CUBOID.editable_geometry().expect("内置来源");
        let target = if !matches!(self, Self::EditableMesh { .. })
            && geometry.vertex_count == 8
            && geometry.faces() == old_faces.faces()
        {
            let vertices_half_mm = geometry.vertices()[..8].try_into().expect("已检查八点数量");
            Self::EditableCuboid {
                vertices_half_mm,
                color_srgb,
                roughness_milli,
                collision,
            }
        } else {
            Self::EditableMesh {
                geometry,
                color_srgb,
                roughness_milli,
                collision,
            }
        };
        target.validate()?;
        Ok(target)
    }

    pub fn editable_geometry(self) -> Option<EditableMeshGeometry> {
        match self {
            Self::EditableMesh { geometry, .. } => Some(geometry),
            _ => self
                .editable_vertices_half_mm()
                .map(EditableMeshGeometry::from_cuboid_vertices),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prism(boundary: &[[i32; 2]]) -> EditableMeshGeometry {
        let count = boundary.len();
        let mut vertices = Vec::new();
        for y in [-2_000, 2_000] {
            vertices.extend(boundary.iter().map(|[x, z]| [*x, y, *z]));
        }
        let mut faces = vec![
            MeshFace::new(&(0..count as u8).rev().collect::<Vec<_>>()).unwrap(),
            MeshFace::new(&(count as u8..(count * 2) as u8).collect::<Vec<_>>()).unwrap(),
        ];
        for side in 0..count {
            let next = (side + 1) % count;
            faces.push(
                MeshFace::new(&[
                    side as u8,
                    next as u8,
                    (next + count) as u8,
                    (side + count) as u8,
                ])
                .unwrap(),
            );
        }
        EditableMeshGeometry::from_parts(&vertices, &faces).unwrap()
    }

    #[test]
    fn mesh_polygon_ear_clipping_preserves_concavity_and_collinear_author_edges() {
        let concave = prism(&[
            [-2_000, -2_000],
            [-2_000, 2_000],
            [2_000, 2_000],
            [2_000, 0],
            [0, 0],
            [0, -2_000],
        ]);
        assert_eq!(concave.triangles().len(), 20);
        assert_eq!(concave.edges().len(), 18);
        let collinear = prism(&[
            [-2_000, -2_000],
            [-2_000, 0],
            [-2_000, 2_000],
            [2_000, 2_000],
            [2_000, -2_000],
        ]);
        assert_eq!(collinear.triangles().len(), 16);
        assert_eq!(collinear.edges().len(), 15);
        let from_pairs = concave
            .triangles_with_faces()
            .into_iter()
            .map(|(_, triangle)| triangle)
            .collect::<Vec<_>>();
        assert_eq!(concave.triangles(), from_pairs);
        let old = ScenePlacementAssetTarget::DEFAULT_CUBOID
            .editable_geometry()
            .unwrap();
        assert_eq!(old.edges(), EDITABLE_CUBOID_EDGES);
        assert_eq!(old.triangles(), crate::EDITABLE_CUBOID_TRIANGLES);
    }

    #[test]
    fn mesh_roundtrip_preserves_fixed_capacity_and_rejects_truncation_or_hidden_data() {
        let geometry = ScenePlacementAssetTarget::DEFAULT_CUBOID
            .editable_geometry()
            .unwrap();
        let target = ScenePlacementAssetTarget::EditableMesh {
            geometry,
            color_srgb: [8, 45, 129],
            roughness_milli: 321,
            collision: false,
        };
        target.validate().unwrap();
        let mut json = serde_json::to_value(target).unwrap();
        assert_eq!(
            serde_json::from_value::<ScenePlacementAssetTarget>(json.clone()).unwrap(),
            target
        );
        json["geometry"]["vertices_half_mm"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(serde_json::from_value::<ScenePlacementAssetTarget>(json).is_err());
        let mut dirty = geometry;
        dirty.faces[6].vertices[5] = 1;
        assert!(dirty.validate().unwrap_err().contains("清零"));
        let mut duplicate = geometry;
        duplicate.faces[1] = duplicate.faces[0];
        assert!(duplicate.validate().unwrap_err().contains("重复面"));
        let mut broken = geometry;
        broken.faces[0].vertices[0] = 63;
        assert!(broken.validate().unwrap_err().contains("索引"));
        assert!(MeshFace::new(&[0, 1, 2, 0]).is_err());
        assert!(MeshFace::new(&(0..17).collect::<Vec<_>>()).is_err());
        assert!(
            EditableMeshGeometry::from_parts(&[[0; 3]; 65], &[])
                .unwrap_err()
                .contains("64")
        );
    }

    #[test]
    fn mesh_polygon_rejects_crossed_and_nonplanar_boundaries_without_partial_triangles() {
        let points = [[0, 0, 0], [4, 0, 0], [4, 0, 4], [2, 0, 2], [0, 0, 4]]
            .map(|point| DVec3::from_array(point.map(f64::from)));
        assert!(triangulate_face(&MeshFace::new(&[0, 2, 1, 4, 3]).unwrap(), &points).is_err());
        let mut bent = points;
        bent[3].y = 10.0;
        assert!(
            triangulate_face(&MeshFace::new(&[0, 1, 2, 3, 4]).unwrap(), &bent)
                .unwrap_err()
                .contains("共面")
        );
        // 历史四边面的折面仍使用0→2对角线，不能受新多边面平面规则影响。
        assert_eq!(
            triangulate_face(&MeshFace::new(&[0, 1, 3, 4]).unwrap(), &bent).unwrap(),
            vec![[0, 1, 3], [0, 3, 4]]
        );
    }

    #[test]
    fn mesh_rejects_two_closed_shells_glued_at_two_vertices_even_when_euler_is_two() {
        let vertices = [
            [0, 0, 2_000],
            [0, 0, -2_000],
            [2_000, -1_000, 0],
            [4_000, 0, 0],
            [2_000, 1_000, 0],
            [1_000, 0, 0],
            [-2_000, -1_000, 0],
            [-1_000, 0, 0],
            [-2_000, 1_000, 0],
            [-4_000, 0, 0],
        ];
        let mut faces = Vec::new();
        for base in [2u8, 6u8] {
            for side in 0..4 {
                let a = base + side;
                let b = base + (side + 1) % 4;
                faces.push(MeshFace::new(&[0, a, b]).unwrap());
                faces.push(MeshFace::new(&[1, b, a]).unwrap());
            }
        }
        let error = EditableMeshGeometry::from_parts(&vertices, &faces).unwrap_err();
        assert!(
            error.contains("一环面"),
            "应明确识别共享顶点处的非流形，而不是接受Euler碰巧为2的双壳：{error}"
        );
    }

    #[test]
    fn mesh_maximum_valid_topology_has_124_triangles_and_126_edges() {
        let mut vertices = Vec::new();
        for y in [-3_000, -1_000, 1_000, 3_000] {
            for side in 0..16 {
                let angle = side as f64 * std::f64::consts::TAU / 16.0;
                vertices.push([
                    (angle.cos() * 4_000.0).round() as i32,
                    y,
                    (-angle.sin() * 4_000.0).round() as i32,
                ]);
            }
        }
        let mut faces = vec![
            MeshFace::new(&(0..16).rev().collect::<Vec<_>>()).unwrap(),
            MeshFace::new(&(48..64).collect::<Vec<_>>()).unwrap(),
        ];
        for ring in 0..3 {
            for side in 0..16 {
                let next = (side + 1) % 16;
                let [a, b, c, d] = [
                    (ring * 16 + side) as u8,
                    (ring * 16 + next) as u8,
                    ((ring + 1) * 16 + next) as u8,
                    ((ring + 1) * 16 + side) as u8,
                ];
                if ring == 0 && side < 14 {
                    faces.push(MeshFace::new(&[a, b, c]).unwrap());
                    faces.push(MeshFace::new(&[a, c, d]).unwrap());
                } else {
                    faces.push(MeshFace::new(&[a, b, c, d]).unwrap());
                }
            }
        }
        let geometry = EditableMeshGeometry::from_parts(&vertices, &faces).unwrap();
        assert_eq!((geometry.vertex_count, geometry.face_count), (64, 64));
        assert_eq!(geometry.triangles().len(), 124);
        assert_eq!(geometry.edges().len(), 126);
        let start = std::time::Instant::now();
        for _ in 0..20 {
            geometry.validate().unwrap();
        }
        eprintln!(
            "最大64点/64面网格validate平均耗时: {:?}",
            start.elapsed() / 20
        );
        let mut oversized = geometry;
        oversized.vertex_count = 65;
        assert!(oversized.validate().unwrap_err().contains("64"));
    }

    #[test]
    fn mesh_coordinate_edits_keep_legacy_storage_and_existing_mesh_identity() {
        let source = ScenePlacementAssetTarget::DEFAULT_CUBOID;
        let mut geometry = source.editable_geometry().unwrap();
        geometry.vertices_half_mm[7][1] += 100;
        let editable = source.with_editable_geometry(geometry).unwrap();
        assert!(matches!(
            editable,
            ScenePlacementAssetTarget::EditableCuboid { .. }
        ));
        let target = ScenePlacementAssetTarget::EditableMesh {
            geometry,
            color_srgb: [1, 2, 3],
            roughness_milli: 221,
            collision: false,
        };
        assert_eq!(target.with_editable_geometry(geometry).unwrap(), target);
    }
}
