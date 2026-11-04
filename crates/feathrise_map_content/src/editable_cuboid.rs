//! 可编辑立方体的固定共享拓扑、半毫米坐标转换和常量规模几何合法性校验。

use bevy::math::{DVec2, DVec3};

use crate::MapPlacementAssetTarget;

/// 顶点索引按XYZ正向位排列：0---、1+--、2-+-、3++-、4--+、5+-+、6-++、7+++。
pub const EDITABLE_CUBOID_EDGES: [[usize; 2]; 12] = [
    [0, 1], [2, 3], [4, 5], [6, 7],
    [0, 2], [1, 3], [4, 6], [5, 7],
    [0, 4], [1, 5], [2, 6], [3, 7],
];

/// 六个外向四边面，依次为-X、+X、-Y、+Y、-Z、+Z；相邻面复用真实共享顶点。
pub const EDITABLE_CUBOID_FACES: [[usize; 4]; 6] = [
    [0, 4, 6, 2], [1, 3, 7, 5], [0, 1, 5, 4],
    [2, 6, 7, 3], [0, 2, 3, 1], [4, 5, 7, 6],
];

/// 每面固定沿0→2三角化，显示、碰撞和面选择必须消费完全相同的十二个三角形。
pub const EDITABLE_CUBOID_TRIANGLES: [[usize; 3]; 12] = [
    [0, 4, 6], [0, 6, 2], [1, 3, 7], [1, 7, 5],
    [0, 1, 5], [0, 5, 4], [2, 6, 7], [2, 7, 3],
    [0, 2, 3], [0, 3, 1], [4, 5, 7], [4, 7, 6],
];

impl MapPlacementAssetTarget {
    /// 返回固定八顶点的本地半毫米坐标；参数立方体直接±全毫米尺寸，无中心漂移或奇数丢精度。
    pub fn editable_vertices_half_mm(self) -> Option<[[i32; 3]; 8]> {
        match self {
            Self::Cuboid { size_mm, .. } => {
                let size = size_mm.map(i32::try_from);
                let [Ok(x), Ok(y), Ok(z)] = size else { return None; };
                Some(std::array::from_fn(|index| [
                    if index & 1 == 0 { -x } else { x },
                    if index & 2 == 0 { -y } else { y },
                    if index & 4 == 0 { -z } else { z },
                ]))
            }
            Self::EditableCuboid { vertices_half_mm, .. } => Some(vertices_half_mm),
            _ => None,
        }
    }

    /// 首次真实几何修改才转换正式记录；只进入编辑模式不制造历史和保存差异。
    pub fn into_editable(self) -> Option<Self> {
        match self {
            Self::Cuboid { color_srgb, roughness_milli, collision, .. } => Some(Self::EditableCuboid {
                vertices_half_mm: self.editable_vertices_half_mm()?, color_srgb, roughness_milli, collision,
            }),
            Self::EditableCuboid { .. } | Self::EditableMesh { .. } => Some(self),
            _ => None,
        }
    }
}

/// 只访问八点十二三角，不扫描地图；双精度以半毫米整数为输入，避免10km范围的f32抵消。
/// 允许凹形和非平面四边面，但要求八点平均值仍位于有向壳内，并拒绝退化、翻转及自相交。
pub(super) fn validate_vertices(vertices: [[i32; 3]; 8]) -> Result<(), String> {
    if vertices.iter().flatten().any(|coordinate| !(-20_000_000..=20_000_000).contains(coordinate)) {
        return Err("可编辑网格顶点必须位于本地各轴±10000米范围内".into());
    }
    for (index, vertex) in vertices.iter().enumerate() {
        if vertices[..index].contains(vertex) {
            return Err("可编辑网格顶点不能重合".into());
        }
    }
    let points = vertices.map(|point| DVec3::from_array(point.map(f64::from)));
    let render_points = vertices.map(|point| DVec3::from_array(point.map(|value| (value as f32 * 0.0005) as f64)));
    for (index, point) in render_points.iter().enumerate() {
        if render_points[..index].contains(point) {
            return Err("可编辑网格顶点在当前坐标精度下重合，请将对象原点移近几何".into());
        }
    }
    let center = points.iter().copied().sum::<DVec3>() / 8.0;
    for triangle in EDITABLE_CUBOID_TRIANGLES {
        let [a, b, c] = triangle.map(|index| points[index]);
        let normal = (b - a).cross(c - a);
        if normal.length_squared() < 0.5 {
            return Err("可编辑网格不能包含退化三角形".into());
        }
        let [ra, rb, rc] = triangle.map(|index| render_points[index]);
        if (rb - ra).cross(rc - ra).length_squared() == 0.0 {
            return Err("可编辑网格三角形在当前坐标精度下退化".into());
        }
        if normal.dot(a - center) <= 0.0 {
            return Err("可编辑网格表面发生翻转或穿过内部中心".into());
        }
    }
    for (index, first) in EDITABLE_CUBOID_TRIANGLES.iter().enumerate() {
        for second in &EDITABLE_CUBOID_TRIANGLES[..index] {
            if triangles_intersect_illegally(*first, *second, &points) {
                return Err("可编辑网格表面不能相互穿透".into());
            }
        }
    }
    Ok(())
}

pub(super) fn triangles_intersect_illegally(first: [usize; 3], second: [usize; 3], points: &[DVec3]) -> bool {
    let mut shared = [0; 3];
    let mut shared_count = 0;
    for vertex in first {
        if second.contains(&vertex) { shared[shared_count] = vertex; shared_count += 1; }
    }
    if shared_count == 3 { return true; } // 新多边形拓扑可能形成反向重叠三角，完整重合不能当作合法共享边界。
    let shared = &shared[..shared_count]; // 固定拓扑逐对校验不进行堆分配。
    let allowed = |point: DVec3| {
        if shared.iter().any(|vertex| point.distance_squared(points[*vertex]) < 1e-10) { return true; }
        if shared.len() == 2 {
            let a = points[shared[0]];
            let edge = points[shared[1]] - a;
            let t = (point - a).dot(edge) / edge.length_squared();
            return (-1e-10..=1.0 + 1e-10).contains(&t) && point.distance_squared(a + edge * t) < 1e-10;
        }
        false
    };
    for (source, destination) in [(first, second), (second, first)] {
        let triangle = destination.map(|vertex| points[vertex]);
        for edge in 0..3 {
            let a = points[source[edge]];
            let b = points[source[(edge + 1) % 3]];
            if segment_hits_triangle(a, b, triangle, &allowed) { return true; }
        }
    }
    false
}

fn segment_hits_triangle(a: DVec3, b: DVec3, triangle: [DVec3; 3], allowed: &impl Fn(DVec3) -> bool) -> bool {
    let [p, q, r] = triangle;
    let normal = (q - p).cross(r - p);
    let da = normal.dot(a - p);
    let db = normal.dot(b - p);
    let epsilon = normal.length() * 1e-7;
    if da.abs() <= epsilon && db.abs() <= epsilon {
        // 共面面片也必须只在拓扑声明的共享点/边相接，防止面片折叠后互相覆盖。
        let axis = normal.abs().max_element();
        let project = |point: DVec3| if normal.x.abs() == axis { DVec2::new(point.y, point.z) }
            else if normal.y.abs() == axis { DVec2::new(point.x, point.z) } else { DVec2::new(point.x, point.y) };
        let orient = |x: DVec2, y: DVec2, z: DVec2| (y - x).perp_dot(z - x);
        let inside = |point| {
            let values = [orient(project(p), project(q), point), orient(project(q), project(r), point), orient(project(r), project(p), point)];
            values.iter().all(|value| *value >= -1e-7) || values.iter().all(|value| *value <= 1e-7)
        };
        if (inside(project(a)) && !allowed(a)) || (inside(project(b)) && !allowed(b)) { return true; }
        let direction = project(b) - project(a);
        for edge in 0..3 {
            let x = triangle[edge];
            let y = triangle[(edge + 1) % 3];
            let other = project(y) - project(x);
            let denominator = direction.perp_dot(other);
            if denominator.abs() < 1e-12 { continue; }
            let delta = project(x) - project(a);
            let t = delta.perp_dot(other) / denominator;
            let u = delta.perp_dot(direction) / denominator;
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) && !allowed(a.lerp(b, t)) { return true; }
        }
        return false;
    }
    if (da > epsilon && db > epsilon) || (da < -epsilon && db < -epsilon) { return false; }
    let denominator = da - db;
    if denominator.abs() <= epsilon { return false; }
    let t = da / denominator;
    if !(-1e-10..=1.0 + 1e-10).contains(&t) { return false; }
    let hit = a.lerp(b, t.clamp(0.0, 1.0));
    let area = normal.length_squared();
    let u = normal.dot((q - hit).cross(r - hit)) / area;
    let v = normal.dot((r - hit).cross(p - hit)) / area;
    let w = 1.0 - u - v;
    u >= -1e-10 && v >= -1e-10 && w >= -1e-10 && !allowed(hit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editable_conversion_preserves_odd_millimeter_dimensions_center_and_material() {
        let target = MapPlacementAssetTarget::Cuboid {
            size_mm: [1, 3, 9_999_999], color_srgb: [1, 70, 200], roughness_milli: 321, collision: false,
        };
        let editable = target.into_editable().unwrap();
        let vertices = editable.editable_vertices_half_mm().unwrap();
        assert_eq!(vertices[0], [-1, -3, -9_999_999]);
        assert_eq!(vertices[7], [1, 3, 9_999_999]);
        assert!(editable.validate().is_ok());
        assert_eq!(editable.into_editable(), Some(editable));
        let json = serde_json::to_string(&editable).unwrap();
        assert_eq!(serde_json::from_str::<MapPlacementAssetTarget>(&json).unwrap(), editable);
        assert!(editable.is_builtin());
        assert!(editable.mesh_reference("").is_none());
        assert!(MapPlacementAssetTarget::Scene0.into_editable().is_none());
    }

    #[test]
    fn editable_fixed_topology_is_closed_outward_and_allows_warped_faces() {
        let mut target = MapPlacementAssetTarget::DEFAULT_CUBOID.into_editable().unwrap();
        let MapPlacementAssetTarget::EditableCuboid { ref mut vertices_half_mm, .. } = target else { unreachable!() };
        vertices_half_mm[7] = [2_500, 2_800, 1_700];
        assert!(target.validate().is_ok());
        // 内凹角仍是合法闭壳；不能把点编辑偷换成只允许凸包的尺寸调整。
        let MapPlacementAssetTarget::EditableCuboid { ref mut vertices_half_mm, .. } = target else { unreachable!() };
        vertices_half_mm[7] = [500, 500, 500];
        assert!(target.validate().is_ok());
        for edge in EDITABLE_CUBOID_EDGES {
            assert_eq!(EDITABLE_CUBOID_FACES.iter().filter(|face| (0..4).any(|i| [face[i], face[(i+1)%4]] == edge || [face[(i+1)%4], face[i]] == edge)).count(), 2);
        }
    }

    #[test]
    fn editable_validation_rejects_degenerate_flipped_out_of_range_and_bad_material() {
        let target = MapPlacementAssetTarget::DEFAULT_CUBOID.into_editable().unwrap();
        let original = target.editable_vertices_half_mm().unwrap();
        let invalid = |vertices_half_mm, roughness_milli| MapPlacementAssetTarget::EditableCuboid {
            vertices_half_mm, color_srgb: [160; 3], roughness_milli, collision: true,
        };
        let mut duplicate = original;
        duplicate[7] = duplicate[6];
        assert!(invalid(duplicate, 700).validate().is_err());
        let mut flipped = original;
        flipped.swap(0, 7);
        assert!(invalid(flipped, 700).validate().is_err());
        let mut far = original;
        far[7][0] = 20_000_001;
        assert!(invalid(far, 700).validate().is_err());
        let mut flat = original;
        for point in &mut flat { point[1] = 0; }
        assert!(invalid(flat, 700).validate().is_err());
        let mut collinear = original;
        collinear[6] = [-2_000, -2_000, 0];
        assert!(invalid(collinear, 700).validate().unwrap_err().contains("退化"));
        let mut precision_loss = original;
        for point in &mut precision_loss { point[0] = if point[0] < 0 { 19_999_999 } else { 20_000_000 }; }
        assert!(invalid(precision_loss, 700).validate().unwrap_err().contains("精度"));
        assert!(invalid(original, 1_001).validate().is_err());
    }

    #[test]
    fn editable_triangle_intersection_allows_only_declared_shared_boundary() {
        let points = [DVec3::new(-2.0, -2.0, 0.0), DVec3::new(2.0, -2.0, 0.0), DVec3::new(0.0, 2.0, 0.0),
            DVec3::new(0.0, 0.0, -2.0), DVec3::new(0.0, 0.0, 2.0), DVec3::new(1.0, 1.0, 1.0), DVec3::ZERO, DVec3::ONE];
        assert!(triangles_intersect_illegally([0, 1, 2], [3, 4, 5], &points));
        assert!(triangles_intersect_illegally([0, 1, 2], [2, 1, 0], &points));
        let mut boundary = points;
        boundary[3] = DVec3::new(-4.0, -4.0, -2.0);
        boundary[4] = DVec3::new(-4.0, -4.0, 2.0);
        assert!(!triangles_intersect_illegally([0, 1, 2], [0, 3, 4], &boundary));
        let coplanar = [DVec3::ZERO, DVec3::X * 2.0, DVec3::Y * 2.0, DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(3.0, 0.0, 0.0), DVec3::new(1.0, 2.0, 0.0), DVec3::ZERO, DVec3::ZERO];
        assert!(triangles_intersect_illegally([0, 1, 2], [3, 4, 5], &coplanar));
    }
}
