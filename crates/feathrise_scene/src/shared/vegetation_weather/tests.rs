//! 天气植被风桥的固定成本ECS测试，不依赖渲染器或场景资产。

use std::time::Duration; // 手动推进与Bevy Globals一致的回绕时间。

use bevy::prelude::*; // App、Time、Vec2及ECS资源测试。
use feathrise_atmosphere::WeatherOutput; // 五类天气使用的正式连续输出类型。
use feathrise_environment::vegetation::blade::GpuBladeInteraction; // 验证草和Mesh共用的全局资源。
use feathrise_settings::EnvironmentPreviewSettings; // 保持旧settings.json基准摆幅字段。

use super::VegetationWeatherPlugin; // 直接运行正式桥接系统。

fn test_app() -> App {
    let mut app = App::new(); // 无RenderApp仍可运行正式Update桥接。
    app.insert_resource(EnvironmentPreviewSettings::default()); // 晴天十六厘米历史基准。
    app.insert_resource(WeatherOutput::default()); // 正式天气默认晴天风力0.15。
    app.insert_resource(GpuBladeInteraction::default()); // 正式GPU草地固定uniform。
    app.insert_resource(Time::<()>::default()); // 渲染Globals提取的Time<()>由测试手动推进。
    app.add_plugins(VegetationWeatherPlugin); // 装配真实排序和相位状态。
    app // 返回可更新的最小世界。
} // 完成无需资产的测试应用。

fn grass_phase(interaction: &GpuBladeInteraction, time: f32) -> f32 {
    time * interaction.wind_speed() + interaction.wind_phase_offset() // 与草/树Shader完全相同的连续时间相位。
} // 完成跨速度变化的相位断言辅助函数。

#[test] // 晴天保持旧默认观感，同时风向统一为天气源。
fn clear_weather_preserves_baseline_and_single_direction() {
    let mut app = test_app(); // 使用生产插件而非复制转换算法。
    app.update(); // 天气解析后的固定小资源桥接一次。
    let interaction = app.world().resource::<GpuBladeInteraction>(); // 读取最终统一风场。
    let weather = app.world().resource::<WeatherOutput>(); // 比较雨云同源方向。
    assert_eq!(interaction.wind_tip_strength(), 0.16); // 原F12/旧存档默认十六厘米在晴天不变。
    assert_eq!(interaction.wind_speed(), 1.10); // 默认动画速度仍是无量纲1.10倍率。
    assert!(
        interaction
            .wind_direction()
            .distance(weather.wind_direction)
            < 0.000_001
    ); // 天气风向只做必要归一化，不再使用独立(0.8,0.6)。
} // 完成默认兼容测试。

#[test] // 不变的天气和设置不得每帧重复标记全局GPU风场已变更。
fn steady_weather_keeps_gpu_resource_unchanged() {
    let mut app = test_app(); // 保持晴天与基准设置不变。
    app.update(); // 首帧统一原先两套不同的风向。
    let changed_at = app
        .world()
        .resource_ref::<GpuBladeInteraction>()
        .last_changed(); // 记录首次写入时的变更tick。
    app.update(); // 稳定天气只更新固定小型时钟状态，不应重写草地GPU资源。
    assert_eq!(
        app.world()
            .resource_ref::<GpuBladeInteraction>()
            .last_changed(),
        changed_at
    ); // 防止风向舍入触发持续上传。
} // 完成稳定帧资源去重验证。

#[test] // 暴雨增加风摆和动画倍率，无风停摆，滑块零值仍严格静止。
fn weather_and_saved_base_control_vegetation_together() {
    let mut app = test_app(); // 输入与正式天气输出同型。
    app.world_mut().resource_mut::<WeatherOutput>().wind = 0.90; // 强风天气不用修改持久设置。
    app.update(); // 应用天气风力映射。
    let interaction = app.world().resource::<GpuBladeInteraction>(); // 读取GPU真实参数。
    assert!((interaction.wind_tip_strength() - 0.274_285_7).abs() < 0.000_01); // 暴雨约27.4厘米而非直接当m/s。
    assert!(interaction.wind_speed() > 1.10); // 风越强叶片变化越快。
    app.world_mut().resource_mut::<WeatherOutput>().wind = 0.0; // 天气回到绝对无风。
    app.update(); // 零风立刻让幅度与时间倍率归零。
    assert_eq!(
        app.world()
            .resource::<GpuBladeInteraction>()
            .wind_tip_strength(),
        0.0
    ); // 无风不保留晴天固定摆幅。
    assert_eq!(
        app.world().resource::<GpuBladeInteraction>().wind_speed(),
        0.0
    ); // 树和草同时停摆。
    app.world_mut().resource_mut::<WeatherOutput>().wind = 0.90; // 恢复强风验证调试零幅。
    app.world_mut()
        .resource_mut::<EnvironmentPreviewSettings>()
        .grass_wind_tip_strength = 0.0; // F12零厘米晴天基准。
    app.update(); // 速度由天气驱动，幅度被调试值关掉。
    assert_eq!(
        app.world()
            .resource::<GpuBladeInteraction>()
            .wind_tip_strength(),
        0.0
    ); // 不改云和雨的风力。
    assert_eq!(app.world().resource::<WeatherOutput>().wind, 0.90); // F12滑块不会回写天气。
} // 完成零风与调试摆幅隔离测试。

#[test] // 变速、暂停以及一小时回绕均保持草/树风波的时间相位连续。
fn animation_phase_does_not_jump_when_weather_speed_changes_or_time_wraps() {
    let mut app = test_app(); // 使用Bevy渲染时间同型资源。
    app.world_mut()
        .resource_mut::<Time>()
        .set_wrap_period(Duration::from_secs(20)); // 缩短回绕周期便于测试。
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs(15)); // 落在非零绝对时间。
    app.update(); // 晴天建立旧相位。
    let before = grass_phase(app.world().resource::<GpuBladeInteraction>(), 15.0); // 记录切换前风波相位。
    app.world_mut().resource_mut::<WeatherOutput>().wind = 0.90; // 同一虚拟时刻立即切至强风。
    app.update(); // 只有速度变化，没有时间推进。
    let after = grass_phase(app.world().resource::<GpuBladeInteraction>(), 15.0); // 新速度必须从旧相位续上。
    assert!((after - before).abs() < 0.000_01); // 防止运行很久后改变天气造成草树跳帧。
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs(4)); // 保持强风推进到19秒。
    app.update(); // 更新当前回绕时间。
    let before_wrap = grass_phase(app.world().resource::<GpuBladeInteraction>(), 19.0); // 记录回绕前相位。
    let speed = app.world().resource::<GpuBladeInteraction>().wind_speed(); // 下一段继续使用同一速度。
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs(2)); // 20秒周期回绕到1秒。
    app.update(); // 跨回绕由桥补偿原20秒速度累计。
    let after_wrap = grass_phase(app.world().resource::<GpuBladeInteraction>(), 1.0); // 读取回绕后同一动画时间。
    assert!((after_wrap - before_wrap - speed * 2.0).abs() < 0.000_01); // 只推进真实2秒，不突然复位风场。
} // 完成变速与回绕连续性测试。
