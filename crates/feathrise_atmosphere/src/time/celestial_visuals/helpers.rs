//! 可见太阳与月亮纯逻辑：计算独立轨迹、显隐时段和保持屏幕角尺寸的世界缩放。

use bevy::{camera::Projection, prelude::*}; // 透视远裁面决定天体距离，向量表达东升西落方向。

use super::super::{
    constants::*,                                 // 复用世界时间固定日出和日落边界。
    lighting::{lunar_direction, solar_direction}, // 太阳、月亮和方向光必须共同读取同一份天文方向数学。
}; // 完成可见天体所需时间常量与共享轨迹导入。
use crate::WorldClock; // 当前日期和时刻决定太阳与月亮的可见方向。

/// 天体圆盘放在透视远裁面的比例。
const CELESTIAL_DISTANCE_FAR_RATIO: f32 = 0.92; // 尽量位于全部地形之后，同时为最大五度圆盘保留裁剪余量。
/// 缺少真实投影的最小ECS环境使用的安全远景距离。
const FALLBACK_CELESTIAL_DISTANCE: f32 = 900.0; // 与Bevy玩法相机默认千米远裁面保持一致。
/// 当前时刻是否显示太阳圆盘。
pub(super) fn sun_is_visible(hour: f32) -> bool {
    (SUNRISE_HOUR..=SUNSET_HOUR).contains(&hour) // 日出和日落端点允许圆盘贴住地平线。
}

/// 当前时刻是否显示月亮圆盘。
pub(super) fn moon_is_visible(hour: f32) -> bool {
    hour <= SUNRISE_HOUR || hour >= SUNSET_HOUR // 与太阳只在两个地平线端点同时出现，区间内部互斥。
}

/// 计算可见太阳方向，并应用动态地图统一天体方位角偏移。
pub(super) fn visible_sun_direction(clock: &WorldClock, azimuth_offset_radians: f32) -> Vec3 {
    let natural = solar_direction(clock.day_of_year(), clock.hour_wrapped()); // 六至十八点直接采用世界本地日间弧线。
    (Quat::from_rotation_y(azimuth_offset_radians) * natural).normalize_or_zero() // 只绕世界Y轴旋转，不改变高度角。
}

/// 计算可见月亮的共享夜间轨迹。
///
/// 月亮在十八点位于东方地平线、午夜达到夜间弧线最高点、六点落到西方；日期平移约半年
/// 只决定轨道平面的年度倾斜。正式夜段的方向光复用同一方向，使影子始终背离月亮。
pub(super) fn visible_moon_direction(clock: &WorldClock, azimuth_offset_radians: f32) -> Vec3 {
    let natural = lunar_direction(clock.day_of_year(), clock.hour_wrapped()); // 与正式夜间方向光共享近似满月轨迹，禁止再次维护平行实现。
    (Quat::from_rotation_y(azimuth_offset_radians) * natural).normalize_or_zero() // 与太阳共同接受作者方位偏移。
}

/// 返回当前透视相机可用的天体远景距离；正交测绘视图不显示球面天空天体。
pub(super) fn celestial_distance(projection: Option<&Projection>) -> Option<f32> {
    match projection {
        Some(Projection::Perspective(perspective)) => {
            let far = if perspective.far.is_finite() && perspective.far > 0.0 {
                perspective.far // 正式玩法和F12透视相机均提供有限正远裁面。
            } else {
                FALLBACK_CELESTIAL_DISTANCE / CELESTIAL_DISTANCE_FAR_RATIO // 损坏投影回退默认千米口径。
            };
            Some((far * CELESTIAL_DISTANCE_FAR_RATIO).max(1.0)) // 保持在远裁面内并避免退化为零距离。
        }
        Some(Projection::Orthographic(_)) => None, // F12俯视测绘没有有意义的天空地平线。
        Some(Projection::Custom(_)) | None => Some(FALLBACK_CELESTIAL_DISTANCE), // 最小测试和未知自定义透视采用安全回退。
    }
}

/// 把角直径转换成给定远景距离下的圆盘世界半径。
pub(super) fn celestial_radius(distance: f32, angular_diameter_radians: f32) -> f32 {
    let diameter = if angular_diameter_radians.is_finite() && angular_diameter_radians > 0.0 {
        angular_diameter_radians // 动态地图有效角直径直接决定太阳屏幕尺寸。
    } else {
        0.533_f32.to_radians() // 损坏外部值回退地球平均太阳角直径。
    };
    distance.max(1.0) * (diameter * 0.5).tan() // Circle网格半径为一，因此Transform缩放直接使用几何半径。
}

#[cfg(test)]
mod tests {
    use super::*; // 纯函数测试覆盖太阳/月亮轨迹、边界显隐与角尺寸换算。
    use crate::WorldTimeSnapshot; // 构造指定日期时刻的正式世界时钟。

    fn clock_at(month: u8, day: u8, hour: f32) -> WorldClock {
        WorldClock::from_snapshot(WorldTimeSnapshot {
            month,                                 // 使用调用方指定月份。
            day,                                   // 使用调用方指定日期。
            seconds_since_midnight: hour * 3600.0, // 小时换算为正式快照秒数。
        })
    }

    #[test]
    fn sun_and_moon_use_opposite_day_night_visibility_windows() {
        assert!(sun_is_visible(6.0)); // 日出端点显示东方太阳。
        assert!(sun_is_visible(12.0)); // 正午持续显示太阳。
        assert!(sun_is_visible(18.0)); // 日落端点显示西方太阳。
        assert!(!sun_is_visible(18.01)); // 日落后立即隐藏太阳圆盘。
        assert!(moon_is_visible(18.0)); // 月亮从东方地平线开始显示。
        assert!(moon_is_visible(0.0)); // 午夜显示月亮。
        assert!(moon_is_visible(6.0)); // 月亮在西方地平线结束显示。
        assert!(!moon_is_visible(12.0)); // 白天不显示固定圆月外观。
    }

    #[test]
    fn visible_bodies_follow_independent_east_to_west_paths() {
        let sunrise = visible_sun_direction(&clock_at(3, 20, 6.0), 0.0); // 春分日出应位于东方。
        let sunset = visible_sun_direction(&clock_at(3, 20, 18.0), 0.0); // 春分日落应位于西方。
        let moonrise = visible_moon_direction(&clock_at(6, 21, 18.0), 0.0); // 夏至夜月亮从东方升起。
        let midnight = visible_moon_direction(&clock_at(6, 21, 0.0), 0.0); // 午夜月亮应在夜间弧线最高点。
        let moonset = visible_moon_direction(&clock_at(6, 21, 6.0), 0.0); // 清晨月亮落到西方。
        assert!(sunrise.x > 0.0 && sunrise.y.abs() < 0.000_1); // +X东且贴地平线。
        assert!(sunset.x < 0.0 && sunset.y.abs() < 0.000_1); // -X西且贴地平线。
        assert!(moonrise.x > 0.0 && moonrise.y.abs() < 0.000_1); // 月亮不复用日落时西侧主光方向。
        assert!(midnight.y > 0.9); // 只保证本地夜间弧线的高点，不绑定城市纬度或固定南侧高度。
        assert!(moonset.x < 0.0 && moonset.y.abs() < 0.000_1); // 六点从西侧落下。
    }

    #[test]
    fn map_azimuth_rotates_both_bodies_without_changing_height() {
        let offset = std::f32::consts::FRAC_PI_2;
        let rotation = Quat::from_rotation_y(offset);
        for (month, day, hour) in [(3, 20, 6.0), (6, 21, 12.0), (12, 21, 21.0)] {
            let clock = clock_at(month, day, hour);
            for (natural, rotated) in [
                (
                    visible_sun_direction(&clock, 0.0),
                    visible_sun_direction(&clock, offset),
                ),
                (
                    visible_moon_direction(&clock, 0.0),
                    visible_moon_direction(&clock, offset),
                ),
            ] {
                assert!(rotated.distance(rotation * natural) < 0.000_001);
                assert!((rotated.y - natural.y).abs() < 0.000_001);
                assert!(rotated.is_finite() && (rotated.length() - 1.0).abs() < 0.000_001);
            }
        }
    }

    #[test]
    fn angular_size_stays_constant_when_camera_far_plane_changes() {
        let near_distance = 920.0; // 默认千米视锥内的圆盘距离。
        let far_distance = 9_200.0; // F12大地图透视视锥可能扩大十倍。
        let near_radius = celestial_radius(near_distance, MOON_ANGULAR_DIAMETER_RADIANS); // 计算近相机世界半径。
        let far_radius = celestial_radius(far_distance, MOON_ANGULAR_DIAMETER_RADIANS); // 计算远相机世界半径。
        assert!((far_radius / near_radius - 10.0).abs() < 0.000_1); // 距离和半径同比放大后屏幕角直径不变。
    }
}
