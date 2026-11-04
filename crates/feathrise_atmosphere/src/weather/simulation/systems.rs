// 天气ECS系统：同步场景区域、消费控制消息、推进暂停感知状态并解析连续输出。

use crate::{WorldClock, time::constants::GAME_TIME_SCALE};
use bevy::prelude::*; // Time、Virtual、State、MessageReader、Res和ResMut用于天气ECS系统参数。
use feathrise_scene_core::state::ScenePhase; // 公共生命周期只决定环境是否推进，应用独自拥有新局重置规则。

use crate::weather::constants::DEFAULT_TRANSITION_GAME_SECONDS; // 未指定时长的剧情命令使用二十五游戏分钟过渡。
use crate::weather::messages::WeatherCommand; // 系统消费剧情、存档工具与 F12 发送的离散天气控制请求。
use crate::weather::resources::{WeatherDebugOverride, WeatherOutput, WeatherState}; // 系统读写天气状态、覆盖和连续输出资源。
use crate::{DynamicMapWeatherClimate, UNIFORM_CLIMATE_PROFILE, WeatherClimateProfile}; // 每张玩法地图显式提供气候配置，缺失配置时不自动推进。

use super::output::resolved_weather_output; // 自然天气与唯一运行时调试覆盖由无ECS依赖纯函数完成。

/// 将当前玩法场景同步为其显式登记的共享天气区域。
pub(crate) fn sync_weather_region(
    scene: Option<Res<State<ScenePhase>>>, // 最小测试应用可能尚未安装游戏场景状态机。
    dynamic_climate: Res<DynamicMapWeatherClimate>, // 动态地图必须等清单显式发布气候后再同步区域。
    mut weather: ResMut<WeatherState>,    // 只修改区域，不重置当前天气和过渡进度。
) {
    let Some(scene) = scene else {
        return; // 没有场景状态时保留快照或默认天气区域，便于独立测试和工具使用。
    }; // 结束可选场景资源判断。
    if let Some(climate) = climate_profile_for_scene(Some(&scene), dynamic_climate.profile()) {
        weather.set_region(climate.region()); // 当前所有地图保持中性全局天气连续性。
    } // 主菜单与加载场景不覆盖存档区域，避免进入玩法前丢失区域连续性。
} // 结束天气区域同步系统。

/// 消费剧情、存档工具和 F12 世界环境控件发送的天气控制消息。
pub(crate) fn handle_weather_commands(
    mut commands: MessageReader<WeatherCommand>, // 按消息写入顺序消费本帧全部天气请求。
    clock: Res<WorldClock>,                      // 立即切换和重抽需要当前月份与时刻。
    scene: Option<Res<State<ScenePhase>>>,        // 重抽使用当前地图输入的气候配置。
    dynamic_climate: Res<DynamicMapWeatherClimate>, // 动态地图重抽读取清单已经采用的局部气候。
    mut weather: ResMut<WeatherState>,           // 过渡、立即切换、重抽和重设种子都修改自动状态。
    mut debug: ResMut<WeatherDebugOverride>,     // 清除覆盖消息只修改运行时强制模式。
) {
    let climate = climate_profile_for_scene(scene.as_deref(), dynamic_climate.profile()); // 当前地图显式提供共享区域、季节权重与局部修正。
    for command in commands.read() {
        match *command {
            WeatherCommand::TransitionTo {
                kind,                      // 读取剧情或工具请求的最终天气意图。
                duration_game_seconds,     // 读取可选自定义过渡时长。
                minimum_hold_game_seconds, // 读取到达最终目标后的可选最低维持时间。
            } => {
                weather.force_transition(
                    kind, // 状态接口会把非相邻请求路由到合法下一节点。
                    duration_game_seconds.unwrap_or(DEFAULT_TRANSITION_GAME_SECONDS), // 缺省使用二十五游戏分钟。
                    minimum_hold_game_seconds.unwrap_or(0.0), // 缺省不延长目标天气的自然稳定时长。
                ); // 显式命令不依赖任何地域身份。
            } // 结束天气过渡命令处理。
            WeatherCommand::ApplyImmediate { kind } => {
                weather.apply_immediate(kind, clock.hour_wrapped()); // 工具可直接写入自然状态并生成稳定期。
            } // 结束天气立即应用命令处理。
            WeatherCommand::Reroll => {
                if debug.forced().is_none()
                    && let Some(climate) = climate.filter(|profile| profile.automatic_weather())
                {
                    weather.reroll(
                        clock.month(),        // 当前月份决定季节权重表。
                        clock.hour_wrapped(), // 当前时刻决定薄雾是否进入候选池。
                        climate,              // 季节权重和局部倍率完全来自调用方输入。
                    ); // 自动模式按当前状态图重新抽取下一目标。
                } // 强制天气、缺失配置或关闭自动演化时忽略重抽。
            } // 结束天气重新抽取命令处理。
            WeatherCommand::ClearOverride => {
                debug.clear(); // 恢复自动输出但不重置后台天气状态和剩余时长。
            } // 结束清除天气覆盖命令处理。
            WeatherCommand::Reseed { seed } => {
                weather.reseed(seed); // 显式重设确定性种子并把随机序号归零。
            } // 结束天气随机序列重设命令处理。
        } // 完成单条天气控制消息分派。
    } // 完成本帧全部天气控制消息消费。
} // 结束天气命令处理系统。

/// 按暂停感知虚拟时间推进输入气候配置下的自动状态机。
pub(crate) fn advance_weather_state(
    time: Res<Time<Virtual>>, // ESC、TAB、F1、F12 和 P 统一暂停该虚拟时间。
    clock: Res<WorldClock>,   // 状态推进读取本帧世界时间Advance后的月份和时刻。
    scene: Option<Res<State<ScenePhase>>>, // 主菜单与加载场景禁止自动天气推进。
    dynamic_climate: Res<DynamicMapWeatherClimate>, // 动态地图只使用当前清单显式选择的气候。
    mut weather: ResMut<WeatherState>, // 写入稳定剩余、过渡进度、目标与随机序号。
    mut debug: ResMut<WeatherDebugOverride>, // 游戏恢复推进时释放自动模式的 F12 手动过渡预览。
) {
    if !time.is_paused() {
        debug.release_automatic_preview(); // F12 关闭并恢复游戏后立刻回到状态机真实进度，强制天气模式仍保持。
    } // 暂停期间保留手动预览，使滑块可在同一帧连续检查天气形成过程。
    let Some(climate) = climate_profile_for_scene(scene.as_deref(), dynamic_climate.profile())
    else {
        return; // 主菜单、加载或未登记地图没有气候配置时关闭自动天气。
    }; // 结束当前地图气候配置读取。
    if !climate.automatic_weather() {
        return; // 主菜单、加载场景和显式关闭自动天气的未来地图保留后台过程。
    } // 结束自动天气场景守卫。
    let hour = clock.hour_wrapped(); // 统一读取零至二十四小时并把24:00哨兵视为午夜。
    let game_seconds = if time.is_paused() {
        0.0 // 暂停时不消费持续时间，但仍允许时刻拖动触发晨雾十点约束。
    } else {
        time.delta_secs() * GAME_TIME_SCALE // 每现实秒与世界时钟一致推进二十四游戏秒。
    }; // 完成本帧天气游戏秒增量计算。
    weather.advance_game_seconds(
        game_seconds,  // 已感知暂停并乘世界时间倍率的游戏秒数。
        clock.month(), // 当前月份决定下一节点季节权重。
        hour,          // 当前时刻决定薄雾生成与散尽。
        climate,       // 当前地图显式气候配置只影响下一次正常抽取。
    ); // 推进自动天气状态机。
} // 结束自动天气推进系统。

/// 将自动天气与唯一运行时调试覆盖解析为连续输出资源。
pub(crate) fn resolve_weather_output(
    weather: Res<WeatherState>,        // 读取自动天气当前、目标和过渡进度。
    debug: Res<WeatherDebugOverride>,  // 读取可选强制天气和预览比例。
    mut output: ResMut<WeatherOutput>, // 写入光照、雾、雨、风和玩法系统共用参数。
) {
    *output = resolved_weather_output(&weather, &debug); // F12只移动表现锚点，不再建立第二条天气数值覆盖。
} // 结束连续天气输出解析系统。

/// 返回当前场景显式登记的天气气候配置。
fn climate_profile_for_scene(
    scene: Option<&State<ScenePhase>>, // 正式运行按当前场景选择，独立测试允许缺少状态机。
    dynamic_profile: Option<WeatherClimateProfile>, // 动态地图由当前受检清单提供显式区域与局部权重。
) -> Option<WeatherClimateProfile> {
    match scene.map(State::get) {
        Some(ScenePhase::Active) => dynamic_profile, // 清单未就绪时停止推进，不再隐式继承硬编码气候。
        Some(ScenePhase::Idle | ScenePhase::Loading) => None, // 非玩法场景没有天气气候消费者。
        None => Some(UNIFORM_CLIMATE_PROFILE), // 独立工具没有场景状态时使用中性退路。
    } // 返回完整显式配置；ScenePhase新增正式地图时穷尽匹配会要求同步登记。
} // 结束地图气候配置查询。

#[cfg(test)] // ECS测试覆盖二十四倍推进、暂停守卫和消息即时切换。
mod tests {
    use super::*; // 测试直接注册完整WeatherPlugin公开系统链。
    use crate::{WeatherKind, WeatherPlugin}; // 构造插件并检查公开天气种类结果。
    use std::time::Duration; // Time::advance_by使用标准持续时间。

    /// 构造包含天气插件所需最小资源的独立Bevy应用。
    fn weather_test_app() -> App {
        let mut app = App::new(); // 使用最小ECS应用避免渲染与窗口依赖。
        app.init_resource::<Time<Virtual>>(); // 手动提供暂停感知虚拟时间资源。
        app.init_resource::<WorldClock>(); // 插入六月十五日正午默认世界时钟。
        app.add_plugins(WeatherPlugin); // 注册正式天气资源、消息、顺序和系统。
        app // 返回可手动推进的天气测试应用。
    } // 结束天气测试应用构造。

    #[test] // 真实系统链必须在暂停时冻结积水历史和雨纹时间，即使虚拟时间仍保留旧delta。
    fn paused_surface_history_and_visual_clock_remain_frozen() {
        // 验证生产ECS暂停守卫。
        let mut app = weather_test_app(); // 创建包含完整天气顺序的最小应用。
        app.world_mut()
            .resource_mut::<WeatherState>()
            .apply_immediate(WeatherKind::HeavyRain, 12.0); // 启动自然大雨。
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(Duration::from_secs(1)); // 推进一视觉秒。
        app.update(); // 正式系统按二十四游戏秒累积湿润。
        let before = app.world().resource::<WeatherState>().snapshot(); // 捕获真实自然历史。
        assert!(before.surface_wetness > 0.0); // 地表已经开始湿润。
        assert_eq!(before.surface_effect_time, 1.0); // 动画时钟使用视觉秒而非二十四倍秒。
        app.world_mut().resource_mut::<Time<Virtual>>().pause(); // 保留非零旧delta并暂停。
        app.update(); // 暂停帧不得重复消费旧时间。
        assert_eq!(app.world().resource::<WeatherState>().snapshot(), before); // 全部自然状态精确冻结。
        assert_eq!(
            app.world().resource::<WeatherOutput>().surface_effect_time,
            1.0
        ); // 输出相位也被冻结。
    } // 结束地表历史ECS暂停测试。

    #[test] // 一现实秒必须与世界时钟一致消费二十四游戏秒稳定期。
    fn virtual_second_consumes_twenty_four_weather_seconds() {
        let mut app = weather_test_app(); // 构造中性气候下的默认晴天天气应用。
        let before = app
            .world()
            .resource::<WeatherState>()
            .stable_remaining_game_seconds(); // 读取推进前晴天稳定剩余时长。
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(Duration::from_secs(1)); // 模拟一秒未暂停虚拟时间。
        app.update(); // 执行天气推进与输出解析。
        let after = app
            .world()
            .resource::<WeatherState>()
            .stable_remaining_game_seconds(); // 读取推进后晴天稳定剩余时长。
        assert!((before - after - 24.0).abs() < 0.000_001); // 一现实秒必须精确消费二十四游戏秒。
    } // 结束天气时间倍率ECS测试。

    #[test] // 暂停虚拟时间时即使保留旧delta也不得消费天气持续期。
    fn paused_virtual_time_does_not_advance_weather() {
        let mut app = weather_test_app(); // 构造中性气候下的默认晴天天气应用。
        {
            let mut time = app.world_mut().resource_mut::<Time<Virtual>>(); // 取得测试虚拟时间资源。
            time.advance_by(Duration::from_secs(1)); // 先制造可能保留到本帧的非零delta。
            time.pause(); // 模拟统一暂停系统在天气推进前暂停虚拟时间。
        } // 释放虚拟时间可变借用。
        let before = app
            .world()
            .resource::<WeatherState>()
            .stable_remaining_game_seconds(); // 读取暂停前晴天稳定剩余时长。
        app.update(); // 执行带暂停状态和旧delta的天气系统。
        let after = app
            .world()
            .resource::<WeatherState>()
            .stable_remaining_game_seconds(); // 读取暂停后晴天稳定剩余时长。
        assert_eq!(before, after); // 暂停状态不得消费任何稳定期游戏秒。
    } // 结束天气暂停ECS测试。

    #[test] // 自动模式手动预览只在暂停调试期间接管，恢复游戏首帧必须回到自然进度。
    fn automatic_preview_releases_when_virtual_time_resumes() {
        let mut app = weather_test_app(); // 构造默认自动晴天天气应用。
        app.world_mut()
            .resource_mut::<WeatherDebugOverride>()
            .set_preview(0.25); // 模拟 F12 把自动过渡预览拖到四分之一。
        app.world_mut().resource_mut::<Time<Virtual>>().pause(); // F12 打开期间统一暂停虚拟时间。
        app.update(); // 暂停帧应保留手动预览供同帧光照和雨雾解析。
        assert!(
            app.world()
                .resource::<WeatherDebugOverride>()
                .preview_override_active()
        ); // 暂停期间自动预览仍由用户比例接管。
        app.world_mut().resource_mut::<Time<Virtual>>().unpause(); // 模拟关闭 F12 并恢复游戏。
        app.update(); // 即使首帧delta为零也应释放临时接管。
        assert!(
            !app.world()
                .resource::<WeatherDebugOverride>()
                .preview_override_active()
        ); // 恢复后画面重新使用状态机真实过渡进度。
    } // 结束自动预览恢复测试。

    #[test] // 动态地图必须使用当前清单输入的气候，不能偷偷采用另一张地图配置。
    fn dynamic_scene_requires_and_uses_manifest_climate() {
        let scene = State::new(ScenePhase::Active); // 构造统一动态地图场景身份。
        assert_eq!(climate_profile_for_scene(Some(&scene), None), None,); // 清单尚未就绪时关闭自动推进，避免沿用上一张图局部倍率。
        let custom_climate = WeatherClimateProfile::new(
            crate::WeatherRegion::Global,
            false,
            [[2.0; 5]; 4],
            [3.0; 5],
        ); // 输入关闭自动演化的自定义配置，验证没有被默认值替换。
        assert_eq!(
            climate_profile_for_scene(Some(&scene), Some(custom_climate)),
            Some(custom_climate),
        ); // 清单采用后保留完整输入配置。
    } // 结束动态地图显式气候测试。

    #[test] // ApplyImmediate消息必须同帧完成状态切换并更新连续输出。
    fn immediate_command_updates_state_and_output_in_same_frame() {
        let mut app = weather_test_app(); // 构造默认晴天天气应用。
        app.world_mut()
            .write_message(WeatherCommand::ApplyImmediate {
                kind: WeatherKind::HeavyRain, // 请求立即应用完整大雨状态。
            }); // 将 F12 立即切换请求写入天气消息队列。
        app.update(); // 执行命令处理、状态推进和连续输出解析。
        let weather = app.world().resource::<WeatherState>(); // 读取立即切换后的自动天气状态。
        assert_eq!(weather.current(), WeatherKind::HeavyRain); // 当前天气必须立即成为大雨。
        assert_eq!(weather.target(), WeatherKind::HeavyRain); // 目标必须与当前一致并结束过渡。
        let output = app.world().resource::<WeatherOutput>(); // 读取同帧解析后的连续天气输出。
        assert_eq!(output.precipitation, 1.0); // 同帧输出必须已经达到完整大雨雨量。
    } // 结束天气立即切换消息ECS测试。

} // 结束天气ECS系统测试模块。
