//! 作者多边面与历史四边面的同源三角化。

use super::MeshFace;
use bevy::math::{DVec2, DVec3};

/// 三四边面保持历史固定对角线与非平面语义；更多边面用投影耳切保留共线边点。
/// 边界不能自交，超过半毫米量化容差的非共面多边形明确拒绝。
pub(super) fn triangulate_face(face: &MeshFace, points: &[DVec3]) -> Result<Vec<[usize; 3]>, String> {
    let indices = face.vertices.map(|vertex| vertex as usize);
    let indices = &indices[..face.indices().len()];
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

/// 密集三四边格的常见路径复用历史固定对角线，逐面追加时不分配临时耳列表。
pub(super) fn visit_face_triangles(
    face: &MeshFace,
    points: &[DVec3],
    mut visit: impl FnMut([usize; 3]) -> Result<(), String>,
) -> Result<(), String> {
    match face.indices() {
        &[a, b, c] => visit([a as usize, b as usize, c as usize]),
        &[a, b, c, d] => {
            visit([a as usize, b as usize, c as usize])?;
            visit([a as usize, c as usize, d as usize])
        }
        _ => {
            for triangle in triangulate_face(face, points)? {
                visit(triangle)?;
            }
            Ok(())
        }
    }
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
