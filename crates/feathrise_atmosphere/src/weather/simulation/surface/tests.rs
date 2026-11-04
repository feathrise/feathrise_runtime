// 地表天气回归：验证先湿后积、雨后消退、旧档兼容与预览隔离。
use super::*; // 直接测试正式纯逻辑。
use crate::weather::simulation::output::resolved_weather_output; // 检查最终公开输出的预览隔离。
use crate::{UNIFORM_CLIMATE_PROFILE, WeatherKind, WeatherSnapshot}; // 复用公开快照和正式气候。

#[test] // 大雨先湿润，积水慢形成，雨后积水消退快于湿痕。
fn rain_then_dry_keeps_surface_history() {
    // 检查完整天气过程。
    let mut state = WeatherState::default(); // 初始干燥。
    state.apply_immediate(WeatherKind::HeavyRain, 12.0); // 切换雨势不立即填满水洼。
    state.advance_surface(120.0); // 下雨两游戏分钟。
    assert!(state.surface_wetness > 0.3); // 地面已明显变湿。
    assert_eq!(state.puddle_amount, 0.0); // 尚未达到形成水洼的湿润阈值。
    state.advance_surface(3600.0); // 持续一游戏小时。
    assert!(state.puddle_amount > 0.8); // 持续大雨形成积水。
    state.apply_immediate(WeatherKind::Clear, 12.0); // 雨停保留历史。
    let rain_end = state.snapshot(); // 保存刚停雨状态。
    state.advance_surface(2700.0); // 排水四十五游戏分钟。
    assert!(state.puddle_amount < rain_end.puddle_amount * 0.4); // 积水较快收缩。
    assert!(state.surface_wetness > 0.6); // 同时仍保留湿痕。
} // 结束雨后历史测试。

#[test] // 暂停时非零旧delta由ECS守卫；纯逻辑零时间也不改变历史。
fn zero_time_preview_and_restore_are_isolated() {
    // 检查预览不消费自然状态。
    let mut state = WeatherState::default(); // 创建自然晴天。
    let before = state.snapshot(); // 保存所有自然字段。
    state.advance_game_seconds(0.0, 6, 12.0, UNIFORM_CLIMATE_PROFILE); // 模拟暂停状态。
    let mut debug = WeatherDebugOverride::default(); // 创建临时调试模式。
    debug.set_mode_index(4); // 强制预览大雨。
    debug.set_preview(1.0); // 检查完整湿润与积水。
    let output = resolved_weather_output(&state, &debug); // 解析同帧预览。
    assert_eq!(output.surface_wetness, 1.0); // 暂停可显示完全湿润。
    assert_eq!(output.puddle_amount, 1.0); // 暂停可显示完整水洼。
    assert_eq!(state.snapshot(), before); // 预览不能污染存档。
    debug.clear(); // 关闭预览。
    assert_eq!(resolved_weather_output(&state, &debug).puddle_amount, 0.0); // 精确恢复干燥自然状态。
} // 结束预览隔离测试。

#[test] // JSON缺字段、范围外及非有限值都必须安全恢复。
fn surface_snapshot_normalizes_and_round_trips() {
    // 检查旧档和新档。
    let old: WeatherSnapshot = serde_json::from_str("{}").expect("旧档恢复"); // 模拟旧天气快照。
    assert_eq!(old.surface_wetness, 0.0); // 缺失湿润字段默认干燥。
    assert_eq!(old.puddle_amount, 0.0); // 缺失积水字段默认无水。
    let state = WeatherState::from_snapshot(WeatherSnapshot {
        // 构造损坏地表快照。
        surface_wetness: f32::NAN,          // 非有限湿润值。
        puddle_amount: 2.0,                 // 超范围积水值。
        surface_effect_time: f32::INFINITY, // 非有限动画时间。
        ..old                               // 其余字段合法。
    }); // 统一入口规范化。
    assert_eq!(state.surface_wetness, 0.0); // 非有限值安全干燥。
    assert_eq!(state.puddle_amount, 1.0); // 有限值钳制。
    assert_eq!(state.surface_effect_time, 0.0); // 坏时间回零。
    let state = WeatherState::from_snapshot(WeatherSnapshot {
        surface_wetness: 0.73,
        puddle_amount: 0.41,
        surface_effect_time: 123.5,
        ..state.snapshot()
    }); // 非默认有效值必须完整保留。
    let json = serde_json::to_string(&state.snapshot()).expect("保存地表历史"); // 走正式JSON链。
    let restored = WeatherState::from_snapshot(serde_json::from_str(&json).expect("恢复地表历史")); // 读取后走规范化。
    assert_eq!(state, restored); // 新存档完整保留地表值。
} // 结束持久化测试。
