//! 动态云外观解析：将天气云量、最终天空色与统一风场转换为连续 GPU 参数。

use bevy::prelude::*; // Color、LinearRgba、Vec2 与 Vec3 用于纯参数计算。

use crate::{
    WeatherOutput, WorldCelestialLight, WorldCelestialLightKind,
    weather::constants::DEFAULT_WIND_DIRECTION,
}; // 天气与最终天体光共同决定云形、云色和朝阳边缘。

use super::noise::smoothstep; // 云量端点使用与程序噪声一致的平滑曲线。

/// 晴天基础薄云以下不创建动态云几何。
const CLOUD_VISIBLE_THRESHOLD: f32 = 0.08; // 默认晴天百分之五云量保持通透且没有透明批次。
/// 满云量允许达到的最大云体不透明度。
const CLOUD_MAX_OPACITY: f32 = 0.94; // 云洞仍由覆盖阈值产生，不使用全屏固定底膜。
/// 低于该不透明度时直接隐藏云层。
const CLOUD_OPACITY_EPSILON: f32 = 0.01; // 避免天气过渡极小尾量长期占用透明绘制。

/// 从已经应用昼夜、作者参数和天气透光率的方向光提取有限的日间云边光。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct CloudSunLight {
    pub(super) direction: Vec3, // 场景指向太阳的真实世界方向。
    pub(super) color: Vec3,     // 最终日光的归一化线性色相。
    pub(super) rim: f32,        // 低角度、最终照度共同控制的零至一云边强度。
}

impl CloudSunLight {
    pub(super) fn from_final_light(
        authored: &WorldCelestialLight, // 明确区分太阳与月亮、固定兼容灯。
        light: &DirectionalLight,       // 世界时间刚写入的天气修正后照度和颜色。
        transform: &Transform,          // 同一帧最终天体方向，不能读滞后一帧的GlobalTransform。
        daylight: bool,                 // 固定兼容灯夜间复用为月光，必须额外用世界时钟隔离。
    ) -> Option<Self> {
        if !daylight
            || authored.kind() == WorldCelestialLightKind::Moon
            || !light.illuminance.is_finite()
            || light.illuminance <= 0.0
            || !authored.base_illuminance.is_finite()
            || authored.base_illuminance <= 0.0
            || !transform.translation.is_finite()
        {
            return None; // 夜间、零光和坏数据一律不生成虚假的暖边。
        }
        let direction = transform.translation.normalize_or_zero(); // 最终灯从场景指向天体的方向。
        if direction.y <= 0.0 {
            return None; // 地平线以下的灯不能给云顶照明。
        }
        let final_color = light.color.to_linear(); // 只用最终日光色，不凭云本身猜夕阳色。
        let color = Vec3::new(final_color.red, final_color.green, final_color.blue); // 转为GPU所需线性色相。
        if !color.is_finite() || color.max_element() <= 0.0 {
            return None; // 损坏或黑色灯不产生白色占位边光。
        }
        let altitude_fade = 1.0 - smoothstep(((direction.y - 0.08) / 0.5).clamp(0.0, 1.0)); // 正午不额外涂暖边。
        let energy = (light.illuminance / authored.base_illuminance)
            .clamp(0.0, 1.0)
            .sqrt(); // 天气遮光和低照度自然压低云边。
        let rim = altitude_fade * energy * 0.75; // 上限克制，云色仍主要来自正式天空。
        (rim > 0.0).then_some(Self {
            direction,                                          // 方向只用于朝阳一侧的片元点积。
            color: color.max(Vec3::ZERO) / color.max_element(), // 色相归一化避免白天HDR值冲淡云体。
            rim, // 阴雨和晨昏在最终照度变化时连续过渡。
        })
    }
}

/// 与渲染组件解耦的完整动态云参数。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CloudCurtainProfile {
    pub(super) visible: bool,           // 只有玩法场景且云量越过晴天阈值时显示。
    pub(super) coverage: f32,           // 阈值重映射后的零至一有效覆盖率。
    pub(super) opacity: f32,            // 云体最大 Alpha，不含固定最低透明度。
    pub(super) top_color: Vec3,         // 受天空与天气色温影响的较亮云顶。
    pub(super) bottom_color: Vec3,      // 随云量加深的云底阴影。
    pub(super) horizon_color: Vec3,     // 与 ClearColor 回退同源的低空衔接色。
    pub(super) scroll_velocity: Vec2,   // 统一风向与风力解析出的每秒周期偏移。
    pub(super) sunlight: CloudSunLight, // 向阳薄边只使用当帧最终太阳灯，不采样月光。
}

impl CloudCurtainProfile {
    #[cfg(test)]
    pub(super) fn test_visible() -> Self {
        Self {
            visible: true,                              // 材质单元测试使用可见云。
            coverage: 0.75,                             // 明显阴天覆盖。
            opacity: 0.8,                               // 保持连续透明云边。
            top_color: Vec3::splat(0.6),                // 中性日间云顶。
            bottom_color: Vec3::splat(0.28),            // 较暗云底。
            horizon_color: Vec3::new(0.18, 0.24, 0.32), // 冷色低空衔接。
            scroll_velocity: Vec2::new(0.01, 0.005),    // 有限测试风偏移。
            sunlight: CloudSunLight::default(),         // 普通材质测试不伪造日光。
        }
    }
}

/// 根据连续天气输出构造防御性动态云参数。
pub(super) fn cloud_curtain_profile(
    output: &WeatherOutput,          // 连续云量、色调和风来自同一帧解析结果。
    final_sky_color: Option<Color>,  // 世界时间完成昼夜、晨昏与天气修正后的 ClearColor 回退。
    gameplay_scene: bool,            // 菜单与加载场景必须无条件关闭玩法云层。
    sunlight: Option<CloudSunLight>, // 已过滤夜间和非太阳灯的最终光采样。
) -> CloudCurtainProfile {
    let cloud_cover = if gameplay_scene {
        finite_unit(output.cloud_cover, 0.0) // 损坏天气云量安全回退晴空。
    } else {
        0.0 // 非玩法场景不保留视觉天气。
    };
    let coverage =
        ((cloud_cover - CLOUD_VISIBLE_THRESHOLD) / (1.0 - CLOUD_VISIBLE_THRESHOLD)).clamp(0.0, 1.0); // 把晴天基础薄云以上部分重新映射到完整覆盖域。
    let opacity = smoothstep(coverage) * CLOUD_MAX_OPACITY; // 首尾零导数避免天气过渡跳变。
    let (top_color, bottom_color, horizon_color) =
        cloud_palette(output.tint, final_sky_color, coverage); // 颜色保持昼夜相对亮度并加入云底层次。
    let wind = finite_unit(output.wind, 0.0); // 风力只接受零至一有限值。
    let direction = safe_wind_direction(output.wind_direction); // 风向损坏时回退默认+X风向。
    let scroll_speed = 0.003 + wind * 0.019; // 阴天缓慢漂移，暴雨明显移动但不快速闪烁。
    CloudCurtainProfile {
        visible: gameplay_scene && opacity > CLOUD_OPACITY_EPSILON, // 晴天不生成灰膜或透明绘制。
        coverage,      // Shader 用它改变云洞阈值而非整体铺一层灰色。
        opacity,       // 最大 Alpha 与覆盖形状职责分离。
        top_color,     // 较亮区域保持天光色温。
        bottom_color,  // 厚云暗部增强体积感。
        horizon_color, // 低空与大气背景平滑衔接。
        scroll_velocity: direction * scroll_speed, // X/Z 世界风映射到二维周期域。
        sunlight: if gameplay_scene {
            sunlight.unwrap_or_default()
        } else {
            CloudSunLight::default()
        }, // 非玩法场景不能沿用旧朝阳边光。
    }
}

/// 构造保持昼夜亮度的云顶、云底与地平线线性色。
fn cloud_palette(
    weather_tint: LinearRgba,       // 没有 ClearColor 时提供天气色温回退。
    final_sky_color: Option<Color>, // 动态天空不可用时也与正式清屏色保持一致。
    coverage: f32,                  // 厚云提高去饱和与云底压暗程度。
) -> (Vec3, Vec3, Vec3) {
    let fallback = Vec3::new(
        finite_non_negative(weather_tint.red, 1.0), // 防御天气红通道。
        finite_non_negative(weather_tint.green, 1.0), // 防御天气绿通道。
        finite_non_negative(weather_tint.blue, 1.0), // 防御天气蓝通道。
    );
    let sky = final_sky_color
        .map(|color| color.to_linear()) // 所有云色计算在线性空间完成，并避免方法指针所有权签名随 Bevy API 漂移。
        .map(|color| {
            Vec3::new(
                finite_non_negative(color.red, 0.0),   // 损坏天空红通道归零。
                finite_non_negative(color.green, 0.0), // 损坏天空绿通道归零。
                finite_non_negative(color.blue, 0.0),  // 损坏天空蓝通道归零。
            )
        })
        .unwrap_or(fallback); // 最小测试或独立天气宿主使用天气色温。
    let sky_peak = sky.max_element(); // 最亮通道保存当前昼夜相对能量。
    if sky_peak <= f32::EPSILON {
        let night = Vec3::new(0.0025, 0.0035, 0.0060); // 纯黑异常值只保留极暗冷色轮廓。
        return (night, night * 0.42, Vec3::ZERO); // 禁止午夜被绝对天气色抬成灰顶棚。
    }
    let tint_peak = fallback.max_element().max(f32::EPSILON); // 色温归一化不改变亮度职责。
    let sky_hue = sky / sky_peak; // 提取当前晨昏或夜景色相。
    let tint_hue = fallback / tint_peak; // 提取阴雨冷灰色相。
    let hue = sky_hue.lerp(tint_hue, coverage * 0.28); // 厚云只适度去饱和，仍保留晨昏方向感。
    let top_peak = (sky_peak * 1.34 + 0.004).clamp(0.004, 0.82); // 云顶略亮于背景但不成为 HDR 白幕。
    let top = hue * top_peak; // 云顶沿当前天空与天气混合色相着色。
    let bottom_factor = 0.58 - coverage * 0.22; // 云越厚，云底与云顶明暗差越明显。
    let bottom = top * bottom_factor.max(0.28); // 保持有限正亮度并避免纯黑斑块。
    let horizon = sky * (0.74 + coverage * 0.08); // 低空略压暗，与动态大气或 ClearColor 无硬边。
    (top, bottom, horizon)
}

/// 返回合法单位风向，损坏输出回退默认+X风向。
fn safe_wind_direction(direction: Vec2) -> Vec2 {
    if direction.is_finite() && direction.length_squared() > f32::EPSILON {
        direction.normalize() // 云速只由独立风力标量决定。
    } else {
        DEFAULT_WIND_DIRECTION // 零向量与非有限值不能污染材质偏移。
    }
}

/// 按虚拟时间推进并循环云层偏移。
pub(super) fn advance_cloud_scroll(
    current: Vec2,      // 上一帧零至一周期偏移。
    velocity: Vec2,     // 当前统一风场每秒偏移。
    delta_seconds: f32, // 暂停感知虚拟帧增量。
) -> Vec2 {
    let delta = if delta_seconds.is_finite() {
        delta_seconds.max(0.0) // 不允许负时间倒放云层。
    } else {
        0.0 // 非有限帧增量按暂停处理。
    };
    let next = current + velocity * delta; // 在周期域中推进统一风偏移。
    Vec2::new(next.x.rem_euclid(1.0), next.y.rem_euclid(1.0)) // 长时间运行不会丢失浮点精度。
}

/// 返回仅在虚拟时间实际运行时有效的动画增量。
pub(super) fn active_virtual_delta_seconds(time: Option<&Time<Virtual>>) -> f32 {
    let Some(time) = time else {
        return 0.0; // 最小测试没有虚拟时钟时保持静止。
    };
    if time.is_paused() {
        0.0 // 本帧刚暂停时即使旧 delta 尚在也不得再漂移。
    } else {
        time.delta_secs() // 正常玩法跟随统一虚拟时间。
    }
}

/// 返回有限零至一值，否则使用安全回退。
fn finite_unit(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0) // 云量与风力限制到定义域。
    } else {
        fallback // NaN 和无穷不得传播到 GPU。
    }
}

/// 返回有限非负颜色通道，否则使用安全回退。
fn finite_non_negative(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0) // 线性 HDR 允许超过一但不允许负通道。
    } else {
        fallback // 损坏色值回退中性或黑色。
    }
}
