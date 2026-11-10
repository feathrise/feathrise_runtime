//! 原命中承托面沿XZ线段的连续追踪；只跨完整XYZ共享边，坡面不漏查、断边不跳层。
use bevy::math::{Vec2, Vec3, Vec3Swizzles};
use std::collections::{HashMap, VecDeque};

const EDGE_EPSILON: f64 = 1.0e-7; // 重心坐标误差只容忍共享边舍入，不焊接不同高度的面。
const SURFACE_EPSILON: f64 = 1.0e-3; // 与实际BVH命中的毫米级表面识别口径一致。

/// BVH矩形候选必须再裁剪到实际线段，不能为对角线吸附复制整个包围矩形的三角。
pub fn terrain_surface_intersects_segment(triangle: [Vec3; 3], start: Vec2, end: Vec2) -> bool {
    segment_coordinates(triangle, start, end).is_some()
}

/// 调用方在唯一后台槽提供局部BVH候选；不存在连续承托面时返回None，禁止退到另一楼层。
pub fn follow_terrain_surface(
    start: Vec3,
    target_xz: Vec2,
    triangles: impl IntoIterator<Item = [Vec3; 3]>,
) -> Option<Vec3> {
    if !start.is_finite() || !target_xz.is_finite() {
        return None;
    }
    let faces = triangles
        .into_iter()
        .filter_map(|triangle| {
            let (weights, delta, interval) = segment_coordinates(triangle, start.xz(), target_xz)?;
            Some((triangle, weights, delta, interval))
        })
        .collect::<Vec<_>>();
    // 原三维落点而非最高Y决定种子；属性拆点只按完整XYZ焊接，洞底和洞顶保持独立。
    let seed = faces
        .iter()
        .enumerate()
        .filter_map(|(index, (triangle, weights, _, interval))| {
            if interval[0] > EDGE_EPSILON {
                return None;
            }
            let error = (height(*triangle, *weights) - f64::from(start.y)).abs();
            (error <= SURFACE_EPSILON).then_some((index, error))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))?
        .0;
    let mut edges = HashMap::new();
    for (index, (triangle, _, _, _)) in faces.iter().enumerate() {
        for opposite in 0..3 {
            edges
                .entry(edge_key(*triangle, opposite))
                .or_insert_with(Vec::new)
                .push(index);
        }
    }
    let mut arrival = vec![f64::INFINITY; faces.len()];
    arrival[seed] = 0.0;
    let mut queue = VecDeque::from([seed]);
    let mut result: Option<Vec3> = None;
    while let Some(index) = queue.pop_front() {
        let (triangle, weights, delta, interval) = faces[index];
        if interval[1] >= 1.0 - EDGE_EPSILON {
            let target_weights = std::array::from_fn(|axis| weights[axis] + delta[axis]);
            let point = Vec3::new(
                target_xz.x,
                height(triangle, target_weights) as f32,
                target_xz.y,
            );
            if !point.is_finite() {
                return None;
            }
            if result
                .is_some_and(|old| (f64::from(old.y) - f64::from(point.y)).abs() > SURFACE_EPSILON)
            {
                return None; // 非流形分叉不能凭候选顺序猜测楼层。
            }
            result = Some(point);
        }
        for opposite in 0..3 {
            let crossing = if delta[opposite] < -EDGE_EPSILON {
                -weights[opposite] / delta[opposite]
            } else if delta[opposite].abs() <= EDGE_EPSILON
                && weights[opposite].abs() <= EDGE_EPSILON
            {
                arrival[index] // 沿共享边行走，或穿过共享顶点时允许零长度过渡到相邻扇面。
            } else {
                continue;
            };
            if crossing < arrival[index] - EDGE_EPSILON
                || crossing > interval[1] + EDGE_EPSILON
                || crossing > 1.0 + EDGE_EPSILON
            {
                continue;
            }
            let adjacent = edges.get(&edge_key(triangle, opposite))?;
            if adjacent.len() != 2 {
                continue;
            } // 外边界和不确定的重叠分叉都不能变成一次跨层落点。
            let next = if adjacent[0] == index {
                adjacent[1]
            } else {
                adjacent[0]
            };
            let next_interval = faces[next].3;
            let crossing = crossing.max(0.0);
            if crossing < next_interval[0] - EDGE_EPSILON
                || crossing > next_interval[1] + EDGE_EPSILON
            {
                continue;
            }
            if crossing + EDGE_EPSILON < arrival[next] {
                arrival[next] = crossing;
                queue.push_back(next);
            }
        }
    }
    result
}

fn segment_coordinates(
    triangle: [Vec3; 3],
    start: Vec2,
    end: Vec2,
) -> Option<([f64; 3], [f64; 3], [f64; 2])> {
    if !triangle.iter().all(|point| point.is_finite()) || !start.is_finite() || !end.is_finite() {
        return None;
    }
    let normal = (triangle[1] - triangle[0])
        .cross(triangle[2] - triangle[0])
        .try_normalize()?;
    if normal.y <= 0.05 {
        return None;
    } // 承托面必须朝上；竖壁或洞顶背面不能把两层连成高度场。
    let weights = coordinates(triangle, start)?;
    let end_weights = coordinates(triangle, end)?;
    let delta = std::array::from_fn(|axis| end_weights[axis] - weights[axis]);
    let mut interval = [0.0_f64, 1.0_f64];
    for axis in 0..3 {
        if delta[axis].abs() <= EDGE_EPSILON {
            if weights[axis] < -EDGE_EPSILON {
                return None;
            }
        } else {
            let crossing = -weights[axis] / delta[axis];
            if delta[axis] > 0.0 {
                interval[0] = interval[0].max(crossing);
            } else {
                interval[1] = interval[1].min(crossing);
            }
        }
    }
    (interval[0] <= interval[1] + EDGE_EPSILON
        && interval[0] <= 1.0 + EDGE_EPSILON
        && interval[1] >= -EDGE_EPSILON)
        .then_some((weights, delta, interval))
}

fn coordinates(triangle: [Vec3; 3], point: Vec2) -> Option<[f64; 3]> {
    let a = triangle[0].xz().as_dvec2();
    let ab = triangle[1].xz().as_dvec2() - a;
    let ac = triangle[2].xz().as_dvec2() - a;
    let relative = point.as_dvec2() - a;
    let determinant = ab.perp_dot(ac);
    if determinant == 0.0 || !determinant.is_finite() {
        return None;
    }
    let b = relative.perp_dot(ac) / determinant;
    let c = ab.perp_dot(relative) / determinant;
    Some([1.0 - b - c, b, c])
}
fn height(triangle: [Vec3; 3], weights: [f64; 3]) -> f64 {
    (0..3)
        .map(|axis| f64::from(triangle[axis].y) * weights[axis])
        .sum()
}
fn edge_key(triangle: [Vec3; 3], opposite: usize) -> ([u32; 3], [u32; 3]) {
    let key = |point: Vec3| {
        point
            .to_array()
            .map(|value| if value == 0.0 { 0 } else { value.to_bits() })
    };
    let a = key(triangle[(opposite + 1) % 3]);
    let b = key(triangle[(opposite + 2) % 3]);
    if a <= b { (a, b) } else { (b, a) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plane(min: f32, max: f32, slope: f32, offset: f32) -> Vec<[Vec3; 3]> {
        let point = |x, z| Vec3::new(x, x * slope + offset, z);
        let a = point(min, -2.0);
        let b = point(max, -2.0);
        let c = point(max, 2.0);
        let d = point(min, 2.0);
        vec![[a, c, b], [a, d, c]]
    }
    #[test]
    fn sloped_support_follows_up_and_down_without_switching_to_nearby_layers() {
        let triangles = plane(-2.0, 2.0, 2.0, 0.0)
            .into_iter()
            .chain(plane(-2.0, 2.0, 2.0, 0.2))
            .chain(plane(-2.0, 2.0, 0.0, -10.0))
            .collect::<Vec<_>>();
        assert_eq!(
            follow_terrain_surface(
                Vec3::new(0.6, 1.2, 0.0),
                Vec2::new(1.0, 0.0),
                triangles.clone()
            ),
            Some(Vec3::new(1.0, 2.0, 0.0))
        );
        assert_eq!(
            follow_terrain_surface(Vec3::new(0.4, 0.8, 0.0), Vec2::ZERO, triangles),
            Some(Vec3::ZERO)
        );
    }
    #[test]
    fn disconnected_support_below_a_cliff_is_not_a_valid_snap() {
        let triangles = plane(-2.0, 0.8, 2.0, 0.0)
            .into_iter()
            .chain(plane(-2.0, 2.0, 0.0, -10.0));
        assert_eq!(
            follow_terrain_surface(Vec3::new(0.6, 1.2, 0.0), Vec2::new(1.0, 0.0), triangles),
            None
        );
    }
    #[test]
    fn shared_edge_attribute_splits_and_corner_crossings_stay_continuous() {
        let mut triangles = plane(-2.0, 0.0, 1.0, 0.0);
        triangles.extend(plane(0.0, 2.0, 1.0, 0.0));
        assert_eq!(
            follow_terrain_surface(Vec3::new(-1.0, -1.0, 0.0), Vec2::new(1.0, 0.0), triangles),
            Some(Vec3::new(1.0, 1.0, 0.0))
        );
        let point = |x, z| Vec3::new(x, x, z);
        let mut triangles = Vec::new();
        for x in [-1.0, 0.0] {
            for z in [-1.0, 0.0] {
                let a = point(x, z);
                let b = point(x + 1.0, z);
                let c = point(x + 1.0, z + 1.0);
                let d = point(x, z + 1.0);
                triangles.extend([[a, c, b], [a, d, c]]);
            }
        }
        assert_eq!(
            follow_terrain_surface(Vec3::new(-0.5, -0.5, -0.5), Vec2::new(0.5, 0.5), triangles),
            Some(Vec3::new(0.5, 0.5, 0.5))
        );
    }
}
