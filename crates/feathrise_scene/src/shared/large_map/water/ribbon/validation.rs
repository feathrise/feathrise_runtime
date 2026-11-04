//! 带状水面公开构建入口的纯参数校验。

use bevy::prelude::Vec3; // 相邻控制点退化检查沿用正式Bevy数学类型。
use feathrise_map_content::{
    MAX_MAP_WATER_FLOW_SPEED, MAX_MAP_WATER_SPLINE_DEPTH_METERS, MAX_MAP_WATER_SPLINE_WIDTH_METERS,
    MIN_MAP_WATER_SPLINE_DEPTH_METERS, MIN_MAP_WATER_SPLINE_WIDTH_METERS, MapWaterSplinePoint,
}; // 与作者资产校验共享同一公开数值范围。

use super::MIN_SEGMENT_LENGTH_METERS; // 独立构建入口与后续帧构造使用同一退化段阈值。

/// 验证公共纯构建入口所需的最小几何和浮点前置条件。
pub(super) fn validate_points(points: &[MapWaterSplinePoint], owner: &str) -> Result<(), String> {
    if points.len() < 2 {
        return Err(format!("{owner}至少需要两个控制点")); // 单点无法建立带状面积与流向。
    }
    for point in points {
        if !point.position.iter().all(|value| value.is_finite())
            || !point.width.is_finite()
            || !(MIN_MAP_WATER_SPLINE_WIDTH_METERS..=MAX_MAP_WATER_SPLINE_WIDTH_METERS)
                .contains(&point.width)
            || !point.depth.is_finite()
            || !(MIN_MAP_WATER_SPLINE_DEPTH_METERS..=MAX_MAP_WATER_SPLINE_DEPTH_METERS)
                .contains(&point.depth)
            || !point.flow_speed.is_finite()
            || !(0.0..=MAX_MAP_WATER_FLOW_SPEED).contains(&point.flow_speed)
        {
            return Err(format!(
                "{owner}控制点{}包含无效位置、宽度、深度或流速",
                point.id
            )); // 坏浮点不得进入后台数组容量和GPU属性。
        }
    }
    if points.windows(2).any(|pair| {
        Vec3::from_array(pair[0].position).distance_squared(Vec3::from_array(pair[1].position))
            <= MIN_SEGMENT_LENGTH_METERS * MIN_SEGMENT_LENGTH_METERS
    }) {
        return Err(format!("{owner}包含退化相邻控制点")); // 零长作者段无法建立确定切线。
    }
    Ok(()) // 发布可以进入固定预算采样的最小合法路径。
}

/// 验证公开纯构建入口收到的标准水材质参数不会把NaN或越界值送入GPU。
pub(super) fn validate_material(
    owner: &str,
    color: [f32; 4],
    roughness: f32,
) -> Result<(), String> {
    if !color
        .iter()
        .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
        || !roughness.is_finite()
        || !(0.0..=1.0).contains(&roughness)
    {
        return Err(format!("{owner}包含无效水色或粗糙度")); // 公开构建入口保持与MapWaterAsset校验相同的材质域。
    }
    Ok(()) // 发布可安全复制进StandardMaterial的有限PBR参数。
}
