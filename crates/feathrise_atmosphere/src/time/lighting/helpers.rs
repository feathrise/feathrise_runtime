// 天体光照纯逻辑：计算无地理地点的本地天体弧线、年度倾斜和昼夜亮度系数。

use std::f32::consts::{PI, TAU}; // NOAA 周期公式和昼夜正弦曲线使用圆周常量。

use bevy::prelude::*; // Color 与 Vec3 表达最终光色和本地东-北-天方向。

use crate::time::constants::{
    DAWN_END_HOUR,               // 清晨环境完成转亮的时刻。
    DAWN_START_HOUR,             // 清晨环境开始转亮的时刻。
    DAY_AMBIENT_BRIGHTNESS,      // 正午环境光基准。
    DUSK_END_HOUR,               // 傍晚环境完成转暗的时刻。
    DUSK_START_HOUR,             // 傍晚环境开始转暗的时刻。
    EARTH_OBLIQUITY_DEGREES,     // 季节照度归一化范围。
    EQUINOX_LIGHT_FACTOR,        // 春秋分直射光系数。
    MOONLIGHT_MAX_FACTOR,        // 午夜月光最大系数。
    NIGHT_AMBIENT_BRIGHTNESS,    // 午夜环境光基准。
    NIGHT_SKY_BRIGHTNESS_FACTOR, // 午夜动态天空回退底色与环境 IBL 亮度比例。
    SUMMER_LIGHT_FACTOR,         // 夏至直射光系数。
    SUNRISE_HOUR,                // 固定日出时刻。
    SUNSET_HOUR,                 // 固定日落时刻。
    WINTER_LIGHT_FACTOR,         // 冬至直射光系数。
};
use crate::{
    WorldCelestialLight,              // 构造正式玩法方向光的作者基准标记。
    WorldCelestialLightKind,          // 动态场景显式区分太阳灯与月亮灯，固定场景保留旧单灯。
    WorldClock,                       // 读取当前日期与时刻生成天体采样。
    WorldEnvironmentModifiers,        // 为未注册天气插件的运行环境提供中性默认修正。
    WorldSkyProfile,                  // 构造天空盒模式的可见背景亮度基准。
    time::WorldEnvironmentMapProfile, // 构造相机IBL补光与镜面反射的日间强度基准。
}; // 完成世界时间光照类型导入。

/// 夜晚清屏颜色，保留冷蓝色轮廓以保证可玩性。
pub(super) const NIGHT_CLEAR_COLOR: Color = Color::srgb(0.008, 0.012, 0.035); // 深蓝黑避免纯黑吞没剪影。
/// 夜间方向光的固定冷蓝颜色。
pub(super) const MOONLIGHT_COLOR: Color = Color::srgb(0.42, 0.52, 0.78); // 冷色月光与暖色低角度日光形成明确区分。
/// 地平线附近太阳的暖色。
pub(super) const LOW_SUN_COLOR: Color = Color::srgb(1.0, 0.36, 0.10); // 清晨和黄昏使用明显橙暖色表现贴近地平线的太阳。
/// 日出时天空使用的暖桃色。
pub(super) const DAWN_CLEAR_COLOR: Color = Color::srgb(0.76, 0.32, 0.18); // 朝霞比夕阳稍亮且更柔和，连接深蓝夜色与作者日景。
/// 日落时天空使用的暖橙红色。
pub(super) const DUSK_CLEAR_COLOR: Color = Color::srgb(0.72, 0.20, 0.08); // 夕阳强化橙红色，使傍晚与清晨形成可辨识差异。

/// 单帧天体光照采样结果，系统据此同步所有渲染组件。
#[derive(Clone, Copy, Debug)] // 纯值类型便于数学测试直接断言。
pub(super) struct LightingSample {
    pub(super) source_direction: Vec3, // 固定场景从场景中心指向当前复用太阳或月亮的单位向量。
    pub(super) sun_source_direction: Vec3, // 动态场景太阳灯始终沿真实太阳轨迹，不再向月亮方向插值。
    pub(super) moon_source_direction: Vec3, // 动态场景月亮灯始终沿真实月亮轨迹，不再借用隐藏代理。
    pub(super) direct_factor: f32,     // 固定场景白天为日照季节曲线，夜间为旧月光零至一曲线。
    pub(super) sunlight_factor: f32,   // 动态场景太阳灯在日出日落为零、正午为一的连续时段系数。
    pub(super) moonlight_factor: f32,  // 动态场景月亮灯在月升月落为零、午夜为一的连续时段系数。
    pub(super) dynamic_day_night_factor: f32, // 动态场景环境光、天空、补光与反射共用的午夜零、正午一连续系数。
    pub(super) environment_day_factor: f32,   // 五至七点转亮、十七至十九点转暗的环境日景系数。
    pub(super) sky_brightness_factor: f32,    // 固定场景环境 IBL 相对日间作者基准的昼夜比例。
    pub(super) ambient_brightness: f32,       // 当前玩法相机独立环境光亮度。
    pub(super) sun_height_factor: f32,        // 白天太阳高度相对当日正午高度的比例。
    pub(super) dawn_factor: f32,              // 五至七点以六点为峰值的朝霞系数。
    pub(super) dusk_factor: f32,              // 十七至十九点以十八点为峰值的夕阳系数。
    pub(super) is_day: bool,                  // 决定使用作者日光色还是固定冷色月光。
}

impl Default for WorldEnvironmentModifiers {
    fn default() -> Self {
        Self {
            direct_light_factor: 1.0,        // 中性环境不改变太阳或月光照度。
            ambient_light_factor: 1.0,       // 中性环境不改变玩法相机环境亮度。
            minimum_ambient_brightness: 0.0, // 晴天不额外抬高现有昼夜环境光下限。
            sky_brightness_factor: 1.0,      // 中性环境不改变动态天空回退底色与环境 IBL 亮度。
            light_tint: Color::WHITE,        // 中性色目标使用白色，强度为零时不会参与混色。
            light_tint_strength: 0.0,        // 默认完全保留太阳、月光与晨昏环境色。
            sky_tint: Color::WHITE,          // 中性天空乘色使用白色，不改变任何昼夜通道。
            sky_tint_strength: 0.0,          // 默认完全保留场景日景、朝霞、夕阳和夜色。
        } // 返回未启用天气时保持现有画面的中性修正。
    } // 结束中性环境修正构造。
} // 结束 WorldEnvironmentModifiers 默认实现。

impl WorldCelestialLight {
    /// 返回当前实体的固定兼容、太阳或月亮稳定身份。
    pub fn kind(&self) -> WorldCelestialLightKind {
        self.kind // 光追历史按该身份写固定槽位，禁止依赖 ECS 实体遍历顺序。
    }

    /// 创建玩法天体方向光标记并保存场景作者的原始日间配置。
    pub fn new(base_illuminance: f32, base_color: Color, day_clear_color: Color) -> Self {
        Self {
            kind: WorldCelestialLightKind::LegacyCombined, // 固定场景继续由一盏灯复用太阳和月亮职责。
            base_illuminance: base_illuminance.max(0.0),   // 负照度没有物理意义，构造时归零。
            base_color,                                    // 高角度日光恢复场景作者选择的颜色。
            night_peak_illuminance: base_illuminance.max(0.0) * MOONLIGHT_MAX_FACTOR, // 固定 HN 场景保留日间照度百分之四的旧月光。
            night_color: MOONLIGHT_COLOR, // 固定场景继续使用原有冷蓝月光颜色。
            azimuth_offset_radians: 0.0,  // 固定场景不引入动态场景全局方位偏移。
            angular_diameter_radians: 0.533_f32.to_radians(), // 固定场景继续使用地球太阳平均角直径。
            day_clear_color, // 昼夜清屏过渡以当前场景日景颜色为起点。
            uses_authored_day_night_curve: false, // HN-01/HN-02 保留旧日光与午夜月光峰值算法。
        }
    }

    /// 创建动态场景完整天体配置；日夜基准、方位和角直径彼此独立。
    pub fn new_authored(
        kind: WorldCelestialLightKind, // 调用方必须明确生成太阳灯或月亮灯，禁止再次创建含糊的动态单灯。
        day_base_illuminance: f32,     // 日间太阳曲线达到一时的 lux 基准。
        day_base_color: Color,         // 高角度日光 sRGB 作者色。
        night_peak_illuminance: f32, // 动态场景月亮在午夜达到的峰值照度；参数名继续兼容既有配置字段。
        night_color: Color,          // 夜间月光 sRGB 作者色。
        azimuth_offset_radians: f32, // 太阳与月亮共同使用的世界 Y 轴偏移。
        angular_diameter_radians: f32, // 光栅和 Solari 共用的光源角直径。
        day_clear_color: Color,      // 场景原有日间清屏基准保持独立。
    ) -> Self {
        Self {
            kind, // 同一份作者配置分别绑定太阳和月亮实体，由最终系统只读取与身份相符的字段。
            base_illuminance: day_base_illuminance.max(0.0), // 上游已校验，构造边界仍拒绝负数。
            base_color: day_base_color, // 颜色范围由共享配置数据层校验。
            night_peak_illuminance: night_peak_illuminance.max(0.0), // 夜间照度不再从日间字段派生。
            night_color, // 月光颜色直接保留动态作者值。
            azimuth_offset_radians: if azimuth_offset_radians.is_finite() {
                azimuth_offset_radians.rem_euclid(TAU) // 运行时只保存零至完整圆的稳定等价值。
            } else {
                0.0 // 防御外部构造器误传非有限方位角。
            },
            angular_diameter_radians: if angular_diameter_radians.is_finite()
                && angular_diameter_radians > 0.0
            {
                angular_diameter_radians // 合法正值直接写入 SunDisk。
            } else {
                0.533_f32.to_radians() // 损坏外部输入恢复地球太阳角直径。
            },
            day_clear_color, // 天体配置不改变场景天空底色作者权威。
            uses_authored_day_night_curve: true, // 动态场景太阳/月亮直射独立，环境通道继续采用全天连续曲线。
        }
    }
}

impl WorldSkyProfile {
    /// 创建天空盒模式的可见背景亮度基准。
    pub fn new(base_brightness: f32) -> Self {
        Self {
            base_brightness: base_brightness.max(0.0), // 负亮度没有意义，最终系统只在组件存在时消费。
        }
    }
}

impl WorldEnvironmentMapProfile {
    /// 创建相机环境贴图的两个独立日间强度基准。
    pub(crate) fn new(base_diffuse_intensity: f32, base_specular_intensity: f32) -> Self {
        Self {
            base_diffuse_intensity: base_diffuse_intensity.max(0.0), // 负漫反射辐亮度没有物理意义。
            base_specular_intensity: base_specular_intensity.max(0.0), // 负镜面辐亮度同样统一归零。
        }
    }

    /// 返回Bevy单一环境Uniform当前应承载的作者基准。
    pub(crate) fn carrier_base_intensity(self) -> f32 {
        if self.base_specular_intensity > 0.0 {
            self.base_specular_intensity // 正常路径以镜面值承载，漫反射差异已编码进小图RGB。
        } else {
            self.base_diffuse_intensity // 反射关闭时黑镜面图允许公共Uniform改为承载补光值。
        }
    }
}

/// 根据当前世界时钟计算方向、直射光、环境光和天空过渡系数。
pub(super) fn lighting_sample(clock: &WorldClock) -> LightingSample {
    let hour = clock.hour_wrapped(); // 24:00 哨兵在天文计算中等同于零点。
    let is_day = (SUNRISE_HOUR..=SUNSET_HOUR).contains(&hour); // 固定六点至十八点为太阳光时段。
    let sunlight_factor = if is_day {
        ((hour - SUNRISE_HOUR) / (SUNSET_HOUR - SUNRISE_HOUR) * PI)
            .sin()
            .max(0.0) // 日出和日落为零，正午附近平滑达到一。
    } else {
        0.0 // 夜间不保留太阳时段系数。
    };
    let moonlight_factor = if is_day {
        0.0 // 白天独立月亮灯不产生直射贡献。
    } else {
        (hour / 24.0 * TAU).cos().max(0.0) // 午夜达到一，六点和十八点平滑归零；峰值照度由独立夜间基准决定。
    };
    let dynamic_day_night_factor = dynamic_day_night_factor(clock); // 动态场景环境、天空、补光和反射只读取这一条明暗曲线；直射灯各走真实天体高度。
    let source_hour = if is_day {
        hour // 白天直接采样当前太阳轨迹。
    } else if hour < SUNRISE_HOUR {
        hour + 12.0 // 午夜至清晨映射到中午至傍晚的简化月亮轨迹。
    } else {
        hour - 12.0 // 傍晚至午夜映射到清晨至中午的简化月亮轨迹。
    };
    let source_direction = solar_direction(clock.day_of_year(), source_hour); // 固定场景仍使用在六点和十八点切换的旧太阳/月亮方向。
    let environment_day_factor = environment_day_factor(hour); // 环境使用独立两小时晨昏进度，方向不再拿可见太阳跨天空插值。
    let sun_source_direction =
        solar_direction(clock.day_of_year(), hour.clamp(SUNRISE_HOUR, SUNSET_HOUR)); // 夜间把无照度太阳停在对应地平线端点，禁止隐藏光跨天空运动。
    let moon_source_direction = lunar_direction(clock.day_of_year(), hour); // 月亮灯与可见月亮逐帧读取完全相同的自然方向。
    let noon_direction = solar_direction(clock.day_of_year(), 12.0); // 世界本地十二点始终是日间弧线最高点，用于归一化暖色过渡。
    let sun_height_factor = if is_day {
        (source_direction.y / noon_direction.y.max(f32::EPSILON)).clamp(0.0, 1.0) // 低角度为零，正午为一。
    } else {
        0.0 // 月光不参与日光暖色插值。
    };
    let dawn_factor = twilight_peak_factor(hour, DAWN_START_HOUR, DAWN_END_HOUR); // 六点达到朝霞峰值。
    let dusk_factor = twilight_peak_factor(hour, DUSK_START_HOUR, DUSK_END_HOUR); // 十八点达到夕阳峰值。
    LightingSample {
        source_direction,      // 保存固定场景兼容使用的旧天体方向。
        sun_source_direction,  // 保存动态场景独立太阳灯的真实轨迹。
        moon_source_direction, // 保存动态场景独立月亮灯的真实轨迹。
        direct_factor: if is_day {
            sunlight_factor * season_light_factor(clock.day_of_year()) // 日光仍严格使用六至十八点时段和季节系数。
        } else {
            moonlight_factor // 夜间使用零至一月光曲线，不再依赖日间照度固定百分比。
        },
        sunlight_factor,          // 动态太阳灯在最终系统中另乘季节、天气与 F1 倍率。
        moonlight_factor,         // 动态月亮灯直接乘独立夜间作者峰值、天气与 F1 倍率。
        dynamic_day_night_factor, // 动态场景所有非直射环境通道在同一时刻采用同一昼夜进度。
        environment_day_factor,   // 固定场景环境与天空继续在原晨昏两小时内完成变化。
        sky_brightness_factor: NIGHT_SKY_BRIGHTNESS_FACTOR
            + (1.0 - NIGHT_SKY_BRIGHTNESS_FACTOR) * environment_day_factor, // 五点与十九点为百分之二，七至十七点恢复作者原值。
        ambient_brightness: NIGHT_AMBIENT_BRIGHTNESS
            + (DAY_AMBIENT_BRIGHTNESS - NIGHT_AMBIENT_BRIGHTNESS) * environment_day_factor, // 环境光用同一晨昏曲线在八至八十之间平滑过渡。
        sun_height_factor, // 系统用该值混合低角度暖色和作者日光色。
        dawn_factor,       // 系统用该值叠加六点朝霞天空和轻微暖色环境光。
        dusk_factor,       // 系统用该值叠加十八点夕阳天空和轻微暖色环境光。
        is_day,            // 系统据此选择太阳或月光颜色。
    }
}

/// 返回动态场景环境通道的统一昼夜亮度系数：午夜为零、日出日落为二分之一、正午为一。
///
/// 环境光、动态天空回退底色与环境 IBL 消费这个值；太阳和月亮直射分别消费各自的高度曲线。
pub fn dynamic_day_night_factor(clock: &WorldClock) -> f32 {
    let hour = clock.hour_wrapped(); // 24:00 哨兵与 0:00 必须得到完全相同的午夜结果。
    (0.5 - 0.5 * (hour / 24.0 * TAU).cos()).clamp(0.0, 1.0) // 单一余弦保证全天连续、早晚对称且没有分段边界闪变。
}

/// 计算近似满月的夜间自然方向；动态方向光与可见月亮必须共同调用。
pub(in crate::time) fn lunar_direction(day_of_year: u16, hour: f32) -> Vec3 {
    let wrapped_hour = if hour.is_finite() {
        hour.rem_euclid(24.0) // 二十四点与零点必须得到完全相同的午夜方向。
    } else {
        0.0 // 损坏时刻采用午夜弧线，避免非有限方向进入渲染组件。
    };
    let shifted_hour = if wrapped_hour <= SUNRISE_HOUR {
        wrapped_hour + 12.0 // 零至六点映射到太阳式正午至日落弧线，月亮向西落下。
    } else {
        wrapped_hour - 12.0 // 十八至二十四点映射到太阳式日出至正午弧线，月亮从东方升起。
    };
    let opposite_season_day = ((u32::from(day_of_year) + 181) % 365 + 1) as u16; // 沿用约半年的日期平移，让月亮采用相对季节的轨道平面倾斜。
    solar_direction(opposite_season_day, shifted_hour) // 复用同一本地弧线并返回从场景指向月亮的单位方向。
}

/// 把零至一输入转换为端点速度为零的平滑曲线。
fn smoothstep01(value: f32) -> f32 {
    let normalized = value.clamp(0.0, 1.0); // 防御时刻边界外输入并统一得到合法插值系数。
    normalized * normalized * (3.0 - 2.0 * normalized) // 三次平滑步进让明暗变化在两端自然接续。
}

/// 计算固定场景兼容使用的五至七点渐亮、十七至十九点渐暗环境日景系数。
fn environment_day_factor(hour: f32) -> f32 {
    if !(DAWN_START_HOUR..DUSK_END_HOUR).contains(&hour) {
        0.0 // 固定场景五点前和十九点后保持迁移前的完整夜景。
    } else if hour < DAWN_END_HOUR {
        smoothstep01((hour - DAWN_START_HOUR) / (DAWN_END_HOUR - DAWN_START_HOUR)) // 固定场景清晨两小时从夜景平滑升至日景。
    } else if hour <= DUSK_START_HOUR {
        1.0 // 固定场景七点至十七点保持作者完整日景环境。
    } else {
        1.0 - smoothstep01((hour - DUSK_START_HOUR) / (DUSK_END_HOUR - DUSK_START_HOUR)) // 固定场景傍晚两小时从日景平滑降至夜景。
    }
}

/// 计算指定两小时晨昏窗口内以中点为峰值的暖色景观系数。
fn twilight_peak_factor(hour: f32, start_hour: f32, end_hour: f32) -> f32 {
    if !(start_hour..=end_hour).contains(&hour) {
        return 0.0; // 窗口外不叠加朝霞或夕阳色。
    }
    let phase = ((hour - start_hour) / (end_hour - start_hour)).clamp(0.0, 1.0); // 把两小时窗口映射到零至一。
    (phase * PI).sin().max(0.0) // 正弦在窗口中点达到一并在两端无缝归零。
}

/// 把世界年内日期转换为年度轨道相位，不使用经纬度或时区。
fn fractional_year(day_of_year: u16) -> f32 {
    let day_index = day_of_year.saturating_sub(1) % 365; // 普通年份固定三百六十五日，外部日期跨年后重复同一周期。
    TAU / 365.0 * f32::from(day_index)
}

/// 使用 NOAA 近似公式计算太阳赤纬弧度。
pub(super) fn solar_declination_radians(day_of_year: u16) -> f32 {
    let gamma = fractional_year(day_of_year); // 年度赤纬只用于轨道平面倾斜和季节照度，不再定位地理太阳正午。
    0.006_918 - 0.399_912 * gamma.cos() + 0.070_257 * gamma.sin() - 0.006_758 * (2.0 * gamma).cos()
        + 0.000_907 * (2.0 * gamma).sin()
        - 0.002_697 * (3.0 * gamma).cos()
        + 0.001_48 * (3.0 * gamma).sin() // NOAA 七项谐波近似返回弧度。
}

/// 计算世界本地日间弧线：六点东方地平线、十二点最高、十八点西方地平线。
pub(in crate::time) fn solar_direction(day_of_year: u16, hour: f32) -> Vec3 {
    let bounded_hour = if hour.is_finite() {
        hour.clamp(SUNRISE_HOUR, SUNSET_HOUR) // 隐藏太阳停在最近的地平线端点，不在夜间绕回天顶。
    } else {
        12.0 // 非有限外部输入采用固定正午，保证输出始终是有限单位方向。
    };
    if bounded_hour <= SUNRISE_HOUR {
        return Vec3::X; // 直接固定东方端点，避免正弦浮点误差造成日出高度偏移。
    }
    if bounded_hour >= SUNSET_HOUR {
        return Vec3::NEG_X; // 西方端点与日出始终相隔半圈，不随任何城市位置改变。
    }
    let phase = (bounded_hour - SUNRISE_HOUR) / (SUNSET_HOUR - SUNRISE_HOUR) * PI;
    let (arc_height, east) = phase.sin_cos();
    let (north_tilt, up_tilt) = solar_declination_radians(day_of_year).sin_cos();
    // 年度赤纬只绕东西轴倾斜整个弧线平面；日出、正午、日落时刻与方向契约保持固定。
    Vec3::new(east, arc_height * up_tilt, -arc_height * north_tilt).normalize_or_zero()
}

/// 按赤纬在冬至、分点和夏至之间连续插值直射光季节系数。
pub(super) fn season_light_factor(day_of_year: u16) -> f32 {
    let normalized_declination = (solar_declination_radians(day_of_year)
        / EARTH_OBLIQUITY_DEGREES.to_radians())
    .clamp(-1.0, 1.0); // 赤纬归一化后冬至为负一、分点为零、夏至为正一。
    if normalized_declination >= 0.0 {
        EQUINOX_LIGHT_FACTOR + (SUMMER_LIGHT_FACTOR - EQUINOX_LIGHT_FACTOR) * normalized_declination // 春分至夏至从零点八五升至一。
    } else {
        EQUINOX_LIGHT_FACTOR + (EQUINOX_LIGHT_FACTOR - WINTER_LIGHT_FACTOR) * normalized_declination // 秋分至冬至从零点八五降至零点七。
    }
}

#[cfg(test)] // 轨迹测试验证固定昼长、年度周期、月亮映射与有限单位方向。
mod tests {
    use super::*; // 测试复用私有天文与采样函数。
    use crate::WorldTimeSnapshot; // 构造指定月份日期的时钟。

    #[test]
    fn fixed_scene_constructor_preserves_legacy_moon_and_sun_disk_defaults() {
        let day_color = Color::srgb(1.0, 0.9, 0.8);
        let day_clear = Color::srgb(0.2, 0.4, 0.8);
        let profile = WorldCelestialLight::new(9_000.0, day_color, day_clear);
        assert_eq!(profile.base_illuminance, 9_000.0);
        assert_eq!(profile.base_color, day_color);
        assert_eq!(profile.night_peak_illuminance, 360.0); // HN-01/HN-02 仍使用日间基准的 4%。
        assert_eq!(profile.night_color, MOONLIGHT_COLOR);
        assert_eq!(profile.azimuth_offset_radians, 0.0);
        assert_eq!(profile.angular_diameter_radians, 0.533_f32.to_radians());
        assert_eq!(profile.day_clear_color, day_clear);
        assert_eq!(profile.kind, WorldCelestialLightKind::LegacyCombined); // 固定场景不得被误迁移成动态太阳或月亮灯。
        assert!(!profile.uses_authored_day_night_curve); // 固定场景必须继续走旧月光峰值曲线。
    }

    /// 用指定日期和小时构造测试时钟。
    fn clock_at(month: u8, day: u8, hour: f32) -> WorldClock {
        WorldClock::from_snapshot(WorldTimeSnapshot {
            month,                                 // 使用测试指定月份。
            day,                                   // 使用测试指定日期。
            seconds_since_midnight: hour * 3600.0, // 将测试小时换算成快照秒数。
        })
    }

    #[test]
    fn local_solar_arc_has_fixed_horizons_and_noon_peak() {
        for day_of_year in [1, 80, 172, 266, 355, 365] {
            let noon = solar_direction(day_of_year, 12.0);
            assert_eq!(solar_direction(day_of_year, 6.0), Vec3::X);
            assert_eq!(solar_direction(day_of_year, 18.0), Vec3::NEG_X);
            assert!(noon.x.abs() < 0.000_001 && noon.y > 0.9);
            for half_hour in 12..=36 {
                let hour = half_hour as f32 * 0.5;
                let direction = solar_direction(day_of_year, hour);
                assert!(direction.y >= 0.0 && direction.y <= noon.y + 0.000_001);
                let mirrored = solar_direction(day_of_year, 24.0 - hour);
                assert!((direction.y - mirrored.y).abs() < 0.000_001); // 清晨与傍晚以世界本地十二点对称，不存在地理正午偏移。
            }
            assert!(solar_direction(day_of_year, 7.0).x > 0.0);
            assert!(solar_direction(day_of_year, 17.0).x < 0.0);
        }
    }

    #[test]
    fn celestial_directions_are_finite_unit_vectors() {
        for day_of_year in [0, 1, 80, 172, 355, 365, 366, u16::MAX] {
            for hour in [
                -100.0,
                -1.0,
                0.0,
                6.0,
                7.25,
                12.0,
                18.0,
                23.9,
                24.0,
                100.0,
                f32::NAN,
                f32::INFINITY,
                f32::NEG_INFINITY,
            ] {
                for direction in [
                    solar_direction(day_of_year, hour),
                    lunar_direction(day_of_year, hour),
                ] {
                    assert!(direction.is_finite());
                    assert!((direction.length() - 1.0).abs() < 0.000_001);
                }
            }
        }
    }

    #[test]
    fn seasonal_orbit_tilt_and_light_repeat_each_year() {
        let summer = solar_direction(clock_at(6, 21, 12.0).day_of_year(), 12.0);
        let winter = solar_direction(clock_at(12, 21, 12.0).day_of_year(), 12.0);
        assert!(summer.z < 0.0 && winter.z > 0.0); // 年度周期只改变弧线平面向南或向北的倾斜，不绑定任何城市纬度。
        for day_of_year in [1, 80, 172, 266, 355] {
            let next_year = day_of_year + 365;
            assert!(
                (season_light_factor(day_of_year) - season_light_factor(next_year)).abs()
                    < 0.000_001
            );
            for hour in [0.0, 6.0, 9.0, 12.0, 15.0, 18.0, 21.0] {
                assert!(
                    solar_direction(day_of_year, hour).distance(solar_direction(next_year, hour))
                        < 0.000_001
                );
                assert!(
                    lunar_direction(day_of_year, hour).distance(lunar_direction(next_year, hour))
                        < 0.000_001
                );
            }
        }
    }

    #[test]
    fn moon_reuses_shifted_local_arc_and_wraps_midnight() {
        let day_of_year = clock_at(6, 21, 12.0).day_of_year();
        let opposite_season_day = clock_at(12, 20, 12.0).day_of_year(); // 六月二十一日向后平移一百八十二日。
        for (moon_hour, solar_hour) in [
            (18.0, 6.0),
            (21.0, 9.0),
            (0.0, 12.0),
            (3.0, 15.0),
            (6.0, 18.0),
        ] {
            let moon = lunar_direction(day_of_year, moon_hour);
            assert!(moon.distance(solar_direction(opposite_season_day, solar_hour)) < 0.000_001);
        }
        assert_eq!(lunar_direction(day_of_year, 18.0), Vec3::X);
        assert_eq!(lunar_direction(day_of_year, 6.0), Vec3::NEG_X);
        assert_eq!(
            lunar_direction(day_of_year, 0.0),
            lunar_direction(day_of_year, 24.0)
        );
        assert_eq!(
            lunar_direction(day_of_year, -3.0),
            lunar_direction(day_of_year, 21.0)
        );
    }

    #[test] // 季节照度应在冬、分点、夏三个设计锚点之间连续变化。
    fn seasonal_light_factor_reaches_design_anchors() {
        let summer = season_light_factor(clock_at(6, 21, 12.0).day_of_year()); // 夏至照度采样。
        let equinox = season_light_factor(clock_at(3, 20, 12.0).day_of_year()); // 春分照度采样。
        let winter = season_light_factor(clock_at(12, 21, 12.0).day_of_year()); // 冬至照度采样。
        assert!((summer - 1.0).abs() < 0.01); // NOAA 近似夏至应接近完整照度。
        assert!((equinox - 0.85).abs() < 0.01); // 春分应接近零点八五。
        assert!((winter - 0.70).abs() < 0.01); // 冬至应接近零点七。
    }

    #[test] // 午夜应启用冷月光强度并把天空盒压到百分之二。
    fn midnight_uses_moonlight_and_dark_sky() {
        let sample = lighting_sample(&clock_at(6, 15, 0.0)); // 采样六月午夜。
        assert!(!sample.is_day); // 午夜必须属于月光阶段。
        assert!((sample.direct_factor - 1.0).abs() < 0.000_001); // 午夜月光曲线达到独立夜间峰值的一倍。
        assert!((sample.sky_brightness_factor - NIGHT_SKY_BRIGHTNESS_FACTOR).abs() < 0.000_001); // 午夜天空盒降到百分之二。
        assert_eq!(sample.ambient_brightness, NIGHT_AMBIENT_BRIGHTNESS); // 午夜环境光固定为八。
        assert!(sample.dynamic_day_night_factor.abs() < 0.000_001); // 动态场景把午夜作为统一环境昼夜系数的最低点。
    }

    #[test] // 动态场景的非直射环境参数必须共用一条早晚对称、全天连续的昼夜曲线。
    fn dynamic_environment_curve_has_one_set_of_time_anchors() {
        let midnight_clock = clock_at(6, 15, 0.0); // 午夜是全部动态环境通道的共同最低点。
        let dawn_clock = clock_at(6, 15, 6.0); // 六点采用日夜区间的中间值，不引入额外分段曲线。
        let noon_clock = clock_at(6, 15, 12.0); // 正午是全部动态环境通道的共同日间峰值。
        let dusk_clock = clock_at(6, 15, 18.0); // 十八点与六点关于正午对称。
        let evening_clock = clock_at(6, 15, 19.0); // 十九点必须仍明显高于午夜最低值。
        let morning_clock = clock_at(6, 15, 5.0); // 五点与十九点关于午夜对称。
        let midnight = dynamic_day_night_factor(&midnight_clock); // 直接验证公开给运行时和F12共同消费的纯函数。
        let dawn = dynamic_day_night_factor(&dawn_clock); // 读取日出锚点。
        let noon = dynamic_day_night_factor(&noon_clock); // 读取正午锚点。
        let dusk = dynamic_day_night_factor(&dusk_clock); // 读取日落锚点。
        let evening = dynamic_day_night_factor(&evening_clock); // 读取十九点暮光进度。
        let morning = dynamic_day_night_factor(&morning_clock); // 读取对称清晨进度。
        assert!(midnight.abs() < 0.000_001); // 午夜必须精确采用夜间最低值。
        assert!((dawn - 0.5).abs() < 0.000_001); // 六点必须位于日夜基础值的正中间。
        assert!((noon - 1.0).abs() < 0.000_001); // 正午必须精确采用完整日间基础值。
        assert!((dusk - 0.5).abs() < 0.000_001); // 十八点必须与六点使用同一半值。
        assert!((evening - 0.370_590_45).abs() < 0.000_001); // 十九点保留约百分之三十七日夜区间，不能突然进入完整夜景。
        assert!((morning - evening).abs() < 0.000_001); // 同太阳高度的清晨和傍晚必须具有相同基础亮度。
        assert!(
            (lighting_sample(&evening_clock).dynamic_day_night_factor - evening).abs() < 0.000_001
        ); // ECS采样不得另算一条内部曲线。
    }

    #[test] // 固定场景兼容曲线必须在五、六、七点从完整夜景递增到完整日景，并保留六点朝霞峰值。
    fn fixed_scene_dawn_environment_preserves_legacy_curve() {
        let five = lighting_sample(&clock_at(6, 15, 5.0)); // 五点采样固定场景完整夜景端点。
        let six = lighting_sample(&clock_at(6, 15, 6.0)); // 六点采样固定场景日出和朝霞峰值。
        let seven = lighting_sample(&clock_at(6, 15, 7.0)); // 七点采样固定场景完整日景端点。
        assert_eq!(five.environment_day_factor, 0.0); // 固定场景五点环境仍是完整夜景。
        assert!((six.environment_day_factor - 0.5).abs() < 0.000_001); // 平滑步进在窗口中点精确达到一半。
        assert_eq!(seven.environment_day_factor, 1.0); // 七点环境恢复完整日景。
        assert!(five.ambient_brightness < six.ambient_brightness); // 环境光从五点到六点持续增亮。
        assert!(six.ambient_brightness < seven.ambient_brightness); // 环境光从六点到七点继续增亮。
        assert!(five.sky_brightness_factor < six.sky_brightness_factor); // 玩法天空盒在清晨前半段同步增亮。
        assert!(six.sky_brightness_factor < seven.sky_brightness_factor); // 玩法天空盒在清晨后半段同步增亮。
        assert!(five.dawn_factor.abs() < 0.000_001); // 五点不叠加朝霞色。
        assert!((six.dawn_factor - 1.0).abs() < 0.000_001); // 六点朝霞色达到峰值。
        assert!(seven.dawn_factor.abs() < 0.000_001); // 七点朝霞退回作者日景色。
        assert!(six.direct_factor.abs() < 0.000_001); // 六点太阳照度仍严格保持原设计的零边界。
    }

    #[test] // 动态场景太阳灯与月亮灯必须各守真实轨迹，晨昏只让照度归零而不跨天空转向。
    fn dynamic_sun_and_moon_keep_independent_directions_and_continuous_factors() {
        let day_of_year = clock_at(6, 15, 12.0).day_of_year(); // 使用六月日期覆盖太阳与月亮各自年度弧线平面的倾斜。
        let seventeen = lighting_sample(&clock_at(6, 15, 17.0)); // 日落前太阳仍在西侧天空并具有直射照度。
        let eighteen = lighting_sample(&clock_at(6, 15, 18.0)); // 日落端点太阳和月亮直射都应平滑归零。
        let eighteen_thirty = lighting_sample(&clock_at(6, 15, 18.5)); // 月升半小时后只允许东方月亮灯产生低照度。
        let midnight = lighting_sample(&clock_at(6, 15, 0.0)); // 午夜月亮达到夜间弧线最高点和照度峰值。
        let five_thirty = lighting_sample(&clock_at(6, 15, 5.5)); // 月落前半小时只允许西侧月亮灯产生低照度。
        let six = lighting_sample(&clock_at(6, 15, 6.0)); // 日出端点两盏直射灯再次共同归零。
        let west_horizon = solar_direction(day_of_year, SUNSET_HOUR); // 计算当日西侧日落端点。
        let east_horizon = solar_direction(day_of_year, SUNRISE_HOUR); // 计算当日东方日出端点。
        assert!(seventeen.sun_source_direction.x < 0.0); // 十七点真实太阳必须位于西侧。
        assert!(seventeen.sunlight_factor > 0.0); // 日落前太阳灯仍有连续贡献。
        assert!(eighteen.sun_source_direction.distance(west_horizon) < 0.000_001); // 十八点太阳灯停在西侧地平线。
        assert!(eighteen.sunlight_factor < 0.000_001); // 日落端点太阳直射必须归零。
        assert!(eighteen.moonlight_factor < 0.000_001); // 月升端点月亮直射也从零开始。
        assert!(eighteen_thirty.sun_source_direction.distance(west_horizon) < 0.000_001); // 十八点半隐藏太阳仍停在西方，绝不飞到天顶。
        assert!(eighteen_thirty.moon_source_direction.x > 0.0); // 十八点半月亮灯位于东方低空。
        assert!(eighteen_thirty.moon_source_direction.y < 0.5); // 刚升起的月亮不能被错误放到头顶。
        assert!(eighteen_thirty.sunlight_factor < 0.000_001); // 日落后太阳灯保持零贡献。
        assert!(eighteen_thirty.moonlight_factor > 0.0); // 日落后月亮灯从零连续增强。
        assert!(midnight.moon_source_direction.y > 0.9); // 午夜月亮位于高空，不再断言任何城市的正午或午夜高度角。
        assert!((midnight.moonlight_factor - 1.0).abs() < 0.000_001); // 午夜月亮灯达到独立夜间作者峰值。
        assert!(five_thirty.moon_source_direction.x < 0.0); // 清晨月亮灯位于西侧低空。
        assert!(five_thirty.sun_source_direction.distance(east_horizon) < 0.000_001); // 隐藏太阳只停在东方日出端点，不经天顶交接。
        assert!(six.sun_source_direction.distance(east_horizon) < 0.000_001); // 六点太阳从东方地平线开始真实轨迹。
        assert!(six.sunlight_factor < 0.000_001); // 日出端点太阳照度从零连续增加。
        assert!(six.moonlight_factor < 0.000_001); // 月落端点月亮照度连续归零。
        for boundary_hour in [6.0, 18.0] {
            let before = lighting_sample(&clock_at(6, 15, boundary_hour - 1.0 / 60.0)); // 读取边界前一分钟的两盏灯系数。
            let after = lighting_sample(&clock_at(6, 15, boundary_hour + 1.0 / 60.0)); // 读取边界后一分钟的两盏灯系数。
            assert!(before.sunlight_factor.max(before.moonlight_factor) < 0.01); // 边界前直射已经平滑接近零，不会产生突然灭灯。
            assert!(after.sunlight_factor.max(after.moonlight_factor) < 0.01); // 边界后另一盏灯只从极低照度起步，不会产生突然亮灯。
        }
    }

    #[test] // 固定场景兼容曲线必须在十七、十八、十九点从完整日景递减到完整夜景，并保留十八点夕阳峰值。
    fn fixed_scene_dusk_environment_preserves_legacy_curve() {
        let seventeen = lighting_sample(&clock_at(6, 15, 17.0)); // 十七点采样固定场景完整日景端点。
        let eighteen = lighting_sample(&clock_at(6, 15, 18.0)); // 十八点采样固定场景日落和夕阳峰值。
        let nineteen = lighting_sample(&clock_at(6, 15, 19.0)); // 十九点采样固定场景完整夜景端点。
        assert_eq!(seventeen.environment_day_factor, 1.0); // 固定场景十七点环境仍是完整日景。
        assert!((eighteen.environment_day_factor - 0.5).abs() < 0.000_001); // 平滑步进在窗口中点精确达到一半。
        assert_eq!(nineteen.environment_day_factor, 0.0); // 固定场景十九点环境进入完整夜景。
        assert!(seventeen.ambient_brightness > eighteen.ambient_brightness); // 环境光从十七点到十八点持续变暗。
        assert!(eighteen.ambient_brightness > nineteen.ambient_brightness); // 环境光从十八点到十九点继续变暗。
        assert!(seventeen.sky_brightness_factor > eighteen.sky_brightness_factor); // 玩法天空盒在傍晚前半段同步变暗。
        assert!(eighteen.sky_brightness_factor > nineteen.sky_brightness_factor); // 玩法天空盒在傍晚后半段同步变暗。
        assert!(seventeen.dusk_factor.abs() < 0.000_001); // 十七点不叠加夕阳色。
        assert!((eighteen.dusk_factor - 1.0).abs() < 0.000_001); // 十八点夕阳色达到峰值。
        assert!(nineteen.dusk_factor.abs() < 0.000_001); // 十九点夕阳退入深蓝夜景。
        assert!(eighteen.direct_factor.abs() < 0.000_001); // 十八点太阳照度仍严格保持原设计的零边界。
    }
}
