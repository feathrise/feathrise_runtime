//! 自适应展开平滑路径；检查四分点，避免S形曲线的中点恰好落在弦上而漏掉弯曲。
use super::*;

pub(super) fn expand(
    nodes: &[[f32; 3]],
    curved: bool,
    tolerance: f32,
) -> Result<Vec<Vec3>, String> {
    let mut points = Vec::new();
    points
        .try_reserve(nodes.len())
        .map_err(|e| format!("路径节点分配失败：{e}"))?;
    for point in nodes.iter().copied().map(Vec3::from_array) {
        if !point.is_finite() || point.abs().max_element() > 100_000. {
            return Err("路径节点坐标无效".into());
        }
        if points
            .last()
            .is_none_or(|p: &Vec3| p.distance_squared(point) > 0.0000001)
        {
            points.push(point);
        }
    }
    if points.len() < 2 {
        return Err("路径需要至少两个不同的节点".into());
    }
    if !curved {
        return Ok(points);
    }
    let mut result = Vec::new();
    result
        .try_reserve(points.len())
        .map_err(|e| format!("曲线路径分配失败：{e}"))?;
    result.push(points[0]);
    for index in 0..points.len() - 1 {
        let p = [
            points[index.saturating_sub(1)],
            points[index],
            points[index + 1],
            points[(index + 2).min(points.len() - 1)],
        ];
        let sample = |t: f32| {
            let a = (p[2] - p[0]) * 0.5;
            let b = (p[3] - p[1]) * 0.5;
            p[1] * (2. * t.powi(3) - 3. * t.powi(2) + 1.)
                + a * (t.powi(3) - 2. * t.powi(2) + t)
                + p[2] * (-2. * t.powi(3) + 3. * t.powi(2))
                + b * (t.powi(3) - t.powi(2))
        };
        let mut stack = vec![(0., p[1], 1., p[2])];
        while let Some((a, from, b, to)) = stack.pop() {
            let error = [0.25, 0.5, 0.75]
                .into_iter()
                .map(|fraction| sample(a + (b - a) * fraction).distance(from.lerp(to, fraction)))
                .fold(0., f32::max);
            if !error.is_finite() {
                return Err("曲线路径出现非有限坐标".into());
            }
            if error <= tolerance {
                if result.last().unwrap().distance_squared(to) > 0.0000001 {
                    result
                        .try_reserve(1)
                        .map_err(|e| format!("曲线路径分配失败：{e}"))?;
                    result.push(to);
                }
            } else {
                let middle = (a + b) * 0.5;
                if middle == a || middle == b {
                    return Err("曲线路径超出当前坐标精度".into());
                }
                let point = sample(middle);
                stack.push((middle, point, b, to));
                stack.push((a, from, middle, point));
            }
        }
    }
    Ok(result)
}
