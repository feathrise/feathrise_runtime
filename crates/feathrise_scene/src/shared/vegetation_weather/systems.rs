//! 将天气零至一风力转换成植被的米制摆幅与无量纲动画速度。

use bevy::prelude::*; // Resource、Res和Time用于固定成本全局参数更新。
use feathrise_atmosphere::WeatherOutput; // 云雨使用的同帧连续天气是唯一输入。
use feathrise_environment::vegetation::blade::{
    GPU_GRASS_MAX_WIND_TIP_STRENGTH,
    GpuBladeInteraction, // 草和Mesh共享一个风参数资源。
}; // 通用植被后端不反向依赖天气系统。
use feathrise_settings::EnvironmentPreviewSettings; // 保留settings.json中原草尖摆幅字段与默认十六厘米。

const CLEAR_WIND: f32 = 0.15; // 晴天基准风力，保持旧十六厘米和1.10倍率观感。
const CLEAR_ANIMATION_SPEED: f32 = 1.10; // 动画时间倍率，不是米每秒物理风速。

#[derive(Resource, Default)] // 不存地图或逐株状态，仅记当前全局风的时间相位。
pub(super) struct VegetationWindPhase {
    last_wrapped_time: Option<f32>, // 检测Bevy渲染全局时间的一小时回绕。
    offset: f32,                    // 变速时补偿绝对时间相乘带来的瞬时相位跳变。
} // 完成固定大小相位状态。

/// 晴天比例为一、零风归零，暴雨逐渐增强但不让草尖瞬间顶到安全上限。
fn wind_response(wind: f32) -> f32 {
    let wind = if wind.is_finite() {
        wind.clamp(0.0, 1.0)
    } else {
        0.0
    }; // 损坏天气输出安全变为无风。
    2.0 * wind / (wind + CLEAR_WIND) // 晴天0.15为1，暴雨0.90约1.71；不是把归一化风当m/s。
} // 完成有界且连续的天气响应映射。

/// 天气变化只写小型Uniform，草株和实体树由既有GPU顶点风路径消费。
pub(super) fn sync_vegetation_weather_wind(
    weather: Option<Res<WeatherOutput>>, // 独立场景测试未装天气插件时回退正式晴天。
    settings: Res<EnvironmentPreviewSettings>,         // F12拖动仅改变晴天基准摆幅而不直接争写GPU。
    time: Option<Res<Time>>,             // 渲染Globals使用的同一虚拟时间及回绕周期。
    mut phase: ResMut<VegetationWindPhase>, // 雨天变速时保持草树波形相位连续。
    interaction: Option<ResMut<GpuBladeInteraction>>, // 无Blade资源的地图插件测试安全跳过。
) {
    let Some(mut interaction) = interaction else {
        return;
    }; // 先以只读方式比较，稳定帧不触发Bevy资源变更检测。
    let weather = weather.as_deref().copied().unwrap_or_default(); // 同帧天气输出或晴天安全默认值。
    let response = wind_response(weather.wind); // 草、树、灌木和目录Mesh共用同一风强曲线。
    let base = if settings.grass_wind_tip_strength.is_finite() {
        settings
            .grass_wind_tip_strength
            .clamp(0.0, GPU_GRASS_MAX_WIND_TIP_STRENGTH) // 旧设置解释为晴天草尖基准值。
    } else {
        0.0 // 非有限旧存档不得污染顶点或GPU uniform。
    }; // 完成持久调试摆幅安全化。
    let tip = (base * response).min(GPU_GRASS_MAX_WIND_TIP_STRENGTH); // 实际摆幅严格不越过35厘米安全预算。
    let speed = CLEAR_ANIMATION_SPEED * response.sqrt(); // 风越强变化越快，晴天精确保留旧1.10倍率。
    let direction = if weather.wind_direction.is_finite() {
        weather.wind_direction.try_normalize().unwrap_or(Vec2::X) // 零风向使用安全世界轴，不重复触发GPU写入。
    } else {
        Vec2::X // 非有限天气风向不得污染草、树和阴影顶点。
    }; // 风向以天气输出为权威，不沿用草地旧硬编码(0.8,0.6)。
    let previous_speed = interaction.wind_speed(); // 旧相位的时间系数必须在覆盖前读取。
    let wrapped_time = time.as_deref().map_or(0.0, Time::elapsed_secs_wrapped); // 与Bevy Globals.time一致。
    if phase
        .last_wrapped_time
        .is_some_and(|last| wrapped_time < last)
    {
        phase.offset += previous_speed
            * time
                .as_deref()
                .map_or(3600.0, |clock| clock.wrap_period().as_secs_f32()); // 跨整小时回绕仍保持旧相位连续。
    } // 完成渲染时间回绕补偿。
    phase.offset += (previous_speed - speed) * wrapped_time; // 新速度从本帧同一相位起步，而不是重算绝对时间。
    phase.last_wrapped_time = Some(wrapped_time); // 下帧比较只需保存一个时间标量。
    if interaction.wind_direction() != direction
        || interaction.wind_tip_strength() != tip
        || previous_speed != speed
        || interaction.wind_phase_offset() != phase.offset
    {
        interaction.set_wind(direction, tip, speed); // 原方法只写风参数，保留至多八个足迹源。
        interaction.set_wind_phase_offset(phase.offset); // 草Shader与Mesh风材质读取同一时间偏移。
    } // 稳定晴天不重复标记GPU资源为已变更。
} // 完成天气到植被的单一同帧桥接。
