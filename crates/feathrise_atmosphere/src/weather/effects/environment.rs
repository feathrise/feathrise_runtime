// 天气环境桥：把WeatherOutput转换为世界时间唯一消费的通用光照与天空修正。

use crate::WorldEnvironmentModifiers;
use bevy::prelude::*; // ECS资源、颜色与场景状态查询来自Bevy预导入。
use feathrise_scene_core::state::ScenePhase; // 菜单与加载阶段需要恢复中性环境修正。 // 时间子域负责把该修正最终写入渲染组件。

use super::is_gameplay_scene; // 统一场景边界避免环境桥与雾雨出现不同启停口径。
use crate::WeatherOutput; // 连续天气输出是视觉与玩法效果的唯一输入。

/// 将当帧天气输出写入世界时间通用环境修正。
pub(crate) fn apply_weather_environment(
    output: Res<WeatherOutput>, // 读取核心状态机和F1覆盖解析后的连续天气结果。
    scene: Option<Res<State<ScenePhase>>>, // 状态机在最小测试中允许缺失，正式游戏据此隔离菜单。
    modifiers: Option<ResMut<WorldEnvironmentModifiers>>, // 未安装世界时间插件时安全跳过，保持核心插件可独立测试。
) {
    let Some(mut modifiers) = modifiers else {
        return; // 独立天气模拟测试没有世界时间资源时不应触发缺资源崩溃。
    }; // 结束世界时间环境资源存在性检查。
    *modifiers = environment_modifiers(&output, is_gameplay_scene(scene.as_deref())); // 同帧生成完整值，避免上种天气字段残留。
} // 结束天气环境桥系统。

/// 根据连续天气参数构造完整且经过防御性规范化的环境修正。
fn environment_modifiers(
    output: &WeatherOutput, // 连续天气输出提供各光照通道倍率、云量和线性色调。
    gameplay_scene: bool,   // 非玩法场景必须无条件返回完整中性修正。
) -> WorldEnvironmentModifiers {
    if !gameplay_scene {
        return WorldEnvironmentModifiers::default(); // 菜单和加载阶段明确恢复中性值，避免场景残留污染画面。
    } // 结束非玩法场景中性回退。
    let direct_light_factor = non_negative(output.direct_light_factor, 1.0); // 方向光倍率只接受有限非负值。
    let cloud_strength = ((unit(output.cloud_cover, 0.0) - 0.05) / 0.95).clamp(0.0, 1.0); // 晴天基础百分之五薄云不额外洗白天空。
    let tint = Color::linear_rgba(
        finite_channel(output.tint.red, 1.0), // 保留天气线性色调红通道并防御非有限值。
        finite_channel(output.tint.green, 1.0), // 保留天气线性色调绿通道并防御非有限值。
        finite_channel(output.tint.blue, 1.0), // 保留天气线性色调蓝通道并防御非有限值。
        1.0, // 环境目标色始终不透明，实际影响程度由独立强度字段控制。
    ); // 完成天气线性色调向Bevy Color转换。
    WorldEnvironmentModifiers {
        direct_light_factor, // 世界时间最终乘入太阳或月光照度。
        ambient_light_factor: non_negative(output.ambient_factor, 1.0), // 阴雨降低环境散射亮度但不接受负值。
        minimum_ambient_brightness: non_negative(output.min_ambient, 0.0), // 夜间可玩下限由天气目标值连续提供。
        sky_brightness_factor: non_negative(output.sky_factor, 1.0), // 正式清屏天空与天空盒统一接受厚云压暗倍率。
        light_tint: tint, // 方向光、月光和环境光共用同一冷灰天气目标色。
        light_tint_strength: ((1.0 - direct_light_factor).clamp(0.0, 1.0) * 0.58), // 云层越厚光色越冷，但保留天体原有晨昏辨识度。
        sky_tint: tint, // ClearColor以线性乘色保留昼夜亮度，同时与云幕使用相同天气色温。
        sky_tint_strength: cloud_strength * 0.80, // 天空乘色与云量连续对应，不能用绝对亮色覆盖午夜天空。
    } // 返回所有字段均在同帧覆盖的世界时间环境修正。
} // 结束环境修正构造。

/// 返回有限的非负标量，否则使用安全回退值。
fn non_negative(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0) // 合法值只裁掉没有物理意义的负数。
    } else {
        fallback // NaN与无穷值不允许进入最终光照组件。
    } // 结束有限值分支。
} // 结束非负标量规范化。

/// 返回零至一有限标量，否则使用安全回退值。
fn unit(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0) // 连续覆盖率统一限制在归一化定义域。
    } else {
        fallback // 非有限值回退到调用方指定的中性强度。
    } // 结束归一化值分支。
} // 结束零至一规范化。

/// 返回有限色彩通道，否则使用中性白通道。
fn finite_channel(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0) // 线性HDR颜色允许超过一，但不允许负通道。
    } else {
        fallback // 非有限颜色回退中性白，避免污染整帧颜色缓冲。
    } // 结束颜色通道分支。
} // 结束颜色通道规范化。

#[cfg(test)] // 纯逻辑测试锁定职责边界与首版云幕映射。
mod tests {
    use super::*; // 测试直接访问环境修正构造函数。

    #[test] // 晴天必须保持世界时间原有光照与天空，不引入可见偏色。
    fn clear_weather_produces_neutral_environment() {
        let output = WeatherOutput::default(); // 核心天气默认输出为完整晴天参数。
        let modifiers = environment_modifiers(&output, true); // 模拟玩法场景中的晴天桥接。
        assert_eq!(modifiers.direct_light_factor, 1.0); // 晴天保留完整天体直射光。
        assert_eq!(modifiers.ambient_light_factor, 1.0); // 晴天保留完整环境光。
        assert_eq!(modifiers.sky_brightness_factor, 1.0); // 晴天保留作者天空亮度。
        assert_eq!(modifiers.light_tint_strength, 0.0); // 晴天不得覆盖世界时间晨昏色。
        assert_eq!(modifiers.sky_tint_strength, 0.0); // 基础少量高云不产生额外天空染色。
    } // 结束晴天中性环境测试。

    #[test] // 恶劣天气必须同时压暗光照、天空并提供连续冷灰染色。
    fn heavy_weather_drives_all_environment_channels() {
        let output = WeatherOutput {
            cloud_cover: 1.0,          // 大雨使用完整云量以验证首版低成本云幕映射。
            direct_light_factor: 0.18, // 使用设计大雨直射光透过率。
            ambient_factor: 0.70,      // 使用设计大雨环境光倍率。
            min_ambient: 5.0,          // 使用设计午夜最低可玩环境亮度。
            sky_factor: 0.25,          // 使用设计大雨天空亮度倍率。
            tint: LinearRgba::new(0.48, 0.57, 0.70, 1.0), // 使用设计大雨深冷蓝灰色调。
            ..default()                // 其余连续天气通道保持公开晴天默认值。
        }; // 构造具有大雨关键字段的连续样本。
        let modifiers = environment_modifiers(&output, true); // 将大雨连续输出转换成世界时间修正。
        assert_eq!(modifiers.direct_light_factor, output.direct_light_factor); // 直射光倍率完整传递。
        assert_eq!(modifiers.ambient_light_factor, output.ambient_factor); // 环境光倍率完整传递。
        assert_eq!(modifiers.minimum_ambient_brightness, output.min_ambient); // 午夜可玩下限完整传递。
        assert_eq!(modifiers.sky_brightness_factor, output.sky_factor); // 天空压暗倍率完整传递。
        assert!(modifiers.light_tint_strength > 0.4); // 厚云应产生明显冷灰光色。
        assert!(modifiers.sky_tint_strength > 0.7); // 大雨完整云量应形成明显低成本云幕。
    } // 结束大雨环境通道测试。

    #[test] // 主菜单和加载阶段必须无条件恢复中性环境。
    fn non_gameplay_scene_resets_environment_to_neutral() {
        let output = WeatherOutput {
            direct_light_factor: 0.18, // 模拟离开大雨玩法场景时仍保留恶劣天气输出。
            ..default()                // 其余连续天气通道保持晴天默认值。
        }; // 从晴天结构构造有效输出。
        let modifiers = environment_modifiers(&output, false); // 菜单阶段请求中性修正。
        assert_eq!(modifiers, WorldEnvironmentModifiers::default()); // 所有字段必须完整恢复，不能只清部分倍率。
    } // 结束非玩法场景中性恢复测试。
} // 结束天气环境桥测试模块。
