// 天气气候配置：接受地图输入的自动演化开关、四季权重与局部倍率，不内置地域规则。

use crate::{WeatherKind, WeatherRegion}; // 气候表以公开天气种类和共享区域作为稳定键。

/// 地图或天气区域使用的静态气候配置。
#[derive(Clone, Copy, Debug, PartialEq)] // 小型只读值可按帧复制给纯抽取逻辑并在测试中直接比较。
pub struct WeatherClimateProfile {
    region: WeatherRegion,              // 地图进入后应同步到的共享天气区域。
    automatic_weather: bool,            // 是否允许使用虚拟时间推进并自动抽取天气。
    seasonal_weights: [[f32; 5]; 4],    // 春夏秋冬五类天气相对基础权重。
    local_weight_multipliers: [f32; 5], // 当前地图对下一次正常抽取施加的局部倍率。
} // 结束天气气候配置声明。

impl WeatherClimateProfile {
    /// 构造可供后续正式地图显式登记的气候配置。
    pub const fn new(
        region: WeatherRegion,              // 同一region的地图共享一份天气演进过程。
        automatic_weather: bool,            // 特殊地图可以显式关闭自动演化。
        seasonal_weights: [[f32; 5]; 4],    // 行为春夏秋冬，列顺序必须与WeatherKind::ALL一致。
        local_weight_multipliers: [f32; 5], // 地图局部修正使用同一列顺序。
    ) -> Self {
        Self {
            region,
            automatic_weather,
            seasonal_weights,
            local_weight_multipliers,
        } // 保存完整静态配置。
    } // 结束气候配置构造。

    /// 返回地图所属共享天气区域。
    pub const fn region(self) -> WeatherRegion {
        self.region // 当前仅提供中性全局连续性，不根据地点选择概率表。
    } // 结束天气区域读取。

    /// 返回当前地图是否允许自动天气推进。
    pub const fn automatic_weather(self) -> bool {
        self.automatic_weather // 调用方可以显式关闭自动演化。
    } // 结束自动演化开关读取。

    /// 返回指定月份和天气的基础权重叠加地图局部倍率后的结果。
    pub fn weight(self, kind: WeatherKind, month: u8) -> f32 {
        if !self.automatic_weather {
            return 0.0; // 关闭自动天气的配置不应向抽取池贡献任何候选。
        } // 结束自动演化守卫。
        let season = match month.clamp(1, 12) {
            3..=5 => 0,  // 三至五月使用春季行。
            6..=8 => 1,  // 六至八月使用夏季行。
            9..=11 => 2, // 九至十一月使用秋季行。
            _ => 3,      // 十二月至二月使用冬季行。
        }; // 完成月份到季节表行映射。
        let index = weather_kind_index(kind); // 把公开枚举稳定映射到权重表列。
        let base = self.seasonal_weights[season][index]; // 读取当前季节基础权重。
        let multiplier = self.local_weight_multipliers[index]; // 读取当前地图局部修正。
        if base.is_finite() && multiplier.is_finite() {
            (base.max(0.0) * multiplier.max(0.0)).max(0.0) // 损坏负数只会关闭候选，不允许反转累计权重。
        } else {
            0.0 // 非有限自定义配置安全视为该候选不可抽取。
        } // 返回规范化最终权重。
    } // 结束气候权重读取。
} // 结束天气气候配置接口。

/// 未提供专用规则时使用的中性气候；各季节和天气权重均为一，不附加局部偏置。
pub const UNIFORM_CLIMATE_PROFILE: WeatherClimateProfile = WeatherClimateProfile::new(
    WeatherRegion::Global, // 地域旧字段迁移后仍保持全局天气连续性。
    true,                  // 保留通用自动演化，关闭时由调用方构造配置。
    [[1.0; 5]; 4],         // 所有季节、天气基础权重一致，冬季不单独禁用大雨。
    [1.0; 5],              // 不携带任何地点专属倍率。
); // 中性退路不替调用方定义项目气候。

/// 把天气枚举映射到气候表稳定列序号。
const fn weather_kind_index(kind: WeatherKind) -> usize {
    match kind {
        WeatherKind::Clear => 0,     // 晴天列。
        WeatherKind::Overcast => 1,  // 阴天列。
        WeatherKind::LightRain => 2, // 小雨列。
        WeatherKind::HeavyRain => 3, // 大雨列。
        WeatherKind::Mist => 4,      // 薄雾列。
    } // 返回稳定权重列序号。
} // 结束天气列映射。

#[cfg(test)] // 气候配置测试锁定四季权重与地图修正行为。
mod tests {
    use super::*; // 直接验证公开气候常量和权重读取接口。

    #[test] // 默认配置不得暗中保留任何季节或天气偏置。
    fn uniform_profile_has_unit_weights_in_every_month() {
        assert_eq!(UNIFORM_CLIMATE_PROFILE.region(), WeatherRegion::Global);
        assert!(UNIFORM_CLIMATE_PROFILE.automatic_weather());
        for month in 1..=12 {
            for kind in WeatherKind::ALL {
                assert_eq!(UNIFORM_CLIMATE_PROFILE.weight(kind, month), 1.0);
            }
        }
    }

    #[test] // 项目输入的四季行和局部倍率共同决定候选，而非由库内地点选择。
    fn custom_profile_applies_season_rows_and_local_multipliers() {
        let climate = WeatherClimateProfile::new(
            WeatherRegion::Global,
            true,
            [[2.0; 5], [4.0; 5], [6.0; 5], [8.0; 5]],
            [1.0, 0.5, 2.0, 0.0, 3.0],
        ); // 使用独立测试输入，包含可关闭的候选。
        assert_eq!(climate.weight(WeatherKind::Clear, 3), 2.0);
        assert_eq!(climate.weight(WeatherKind::Overcast, 6), 2.0);
        assert_eq!(climate.weight(WeatherKind::LightRain, 9), 12.0);
        assert_eq!(climate.weight(WeatherKind::HeavyRain, 12), 0.0);
        assert_eq!(climate.weight(WeatherKind::Mist, 1), 24.0);
    }

    #[test] // 禁用自动天气时，即使输入有正权重也不得贡献候选。
    fn disabled_profile_has_no_candidates() {
        let climate =
            WeatherClimateProfile::new(WeatherRegion::Global, false, [[2.0; 5]; 4], [3.0; 5]);
        for kind in WeatherKind::ALL {
            assert_eq!(climate.weight(kind, 6), 0.0);
        }
    }

    #[test] // 非有限和负数自定义权重只关闭候选，避免污染累计概率。
    fn invalid_custom_weights_are_disabled() {
        let climate = WeatherClimateProfile::new(
            WeatherRegion::Global,
            true,
            [[f32::NAN, -2.0, 3.0, 4.0, 5.0]; 4],
            [1.0, 1.0, f32::INFINITY, -1.0, 2.0],
        );
        for kind in [
            WeatherKind::Clear,
            WeatherKind::Overcast,
            WeatherKind::LightRain,
            WeatherKind::HeavyRain,
        ] {
            assert_eq!(climate.weight(kind, 6), 0.0);
        }
        assert_eq!(climate.weight(WeatherKind::Mist, 6), 10.0);
    }
} // 结束天气气候配置测试模块。
