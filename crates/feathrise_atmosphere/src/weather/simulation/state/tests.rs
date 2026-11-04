use super::*; // 测试直接访问状态内部纯推进接口。
use crate::weather::simulation::output::resolved_weather_output; // 入雾反向测试验证交换端点前后连续输出不跳变。
use crate::{UNIFORM_CLIMATE_PROFILE, WeatherDebugOverride}; // 调试覆盖和中性气候从crate顶层稳定路径导入。

#[test] // 新游戏必须从至少四游戏小时晴天开始。
fn default_state_starts_with_stable_clear_weather() {
    let state = WeatherState::default(); // 构造新游戏默认天气状态。
    assert_eq!(state.current(), WeatherKind::Clear); // 当前天气必须为晴。
    assert_eq!(state.target(), WeatherKind::Clear); // 默认状态不能存在未完成过渡。
    assert_eq!(
        state.stable_remaining_game_seconds(), // 读取新游戏晴天剩余时长。
        INITIAL_CLEAR_GAME_SECONDS,            // 至少四游戏小时的默认值应精确保存。
    ); // 完成初始晴天时长断言。
    assert!(!state.is_transitioning()); // 新游戏首帧不应处于天气过渡中。
} // 结束新游戏默认天气测试。

#[test] // 缺字段旧JSON必须恢复为完整默认天气快照。
fn serde_default_supports_old_saves_without_weather_fields() {
    let snapshot: WeatherSnapshot = serde_json::from_str("{}").expect("empty weather snapshot"); // 模拟旧存档新增空天气对象。
    assert_eq!(snapshot, WeatherSnapshot::default()); // 所有缺失字段必须使用统一默认值补齐。
} // 结束旧存档默认字段测试。

#[test] // 未来或损坏存档中的未知天气与区域名称必须安全回退而不能阻断整个角色读档。
fn serde_unknown_enum_names_fall_back_to_safe_defaults() {
    let snapshot: WeatherSnapshot = serde_json::from_str(
        r#"{"current":"sandstorm","target":"clear","region":"unknown_region"}"#,
    )
    .expect("unknown weather names should remain loadable"); // 模拟新版本天气名被旧客户端读取或存档被手工修改。
    assert_eq!(snapshot.current, WeatherKind::Clear); // 未知天气回退为不会产生降水副作用的晴天。
    assert_eq!(snapshot.region, WeatherRegion::Global); // 未知区域回退为中性全局区域。
} // 结束未知枚举名称兼容测试。

#[test] // 历史与未知区域标签迁移为全局身份，其他进行中的快照字段必须保留。
fn serde_legacy_regions_migrate_to_global_without_resetting_weather() {
    for label in ["kaifeng", "test", "unknown_region", "global"] {
        let snapshot: WeatherSnapshot = serde_json::from_value(serde_json::json!({
            "region": label,
            "current": "light_rain",
            "target": "heavy_rain",
            "transition_progress": 0.25,
            "transition_duration_game_seconds": 600.0,
            "seed": 42,
            "sequence": 7,
        }))
        .expect("旧区域标签必须仍可读取"); // 旧标签仅作为兼容输入，不恢复已删除的地域规则。
        let restored = WeatherState::from_snapshot(snapshot).snapshot();
        assert_eq!(restored.region, WeatherRegion::Global);
        assert_eq!(restored.current, WeatherKind::LightRain);
        assert_eq!(restored.target, WeatherKind::HeavyRain);
        assert_eq!(restored.transition_progress, 0.25);
        assert_eq!(restored.transition_duration_game_seconds, 600.0);
        assert_eq!(restored.seed, 42);
        assert_eq!(restored.sequence, 7);
        let encoded = serde_json::to_value(restored).expect("中性快照必须可写出");
        assert_eq!(encoded["region"], "global"); // 新写出的快照统一使用中性标签。
    }
}

#[test] // 损坏快照必须修复非有限值、非法图边和零种子。
fn restore_normalizes_corrupted_snapshot() {
    let mut state = WeatherState::default(); // 从合法默认状态开始恢复损坏快照。
    state.restore(WeatherSnapshot {
        current: WeatherKind::Clear,    // 使用晴天作为非法直跳起点。
        target: WeatherKind::HeavyRain, // 晴天不能直接跳到大雨。
        queued_target: None,            // 模拟旧快照尚未显式保存跨节点最终意图。
        queued_transition_duration_game_seconds: f32::NAN, // 模拟损坏的后续单段时长。
        pending_minimum_hold_game_seconds: f32::INFINITY, // 模拟损坏的剧情最低维持时间。
        transition_progress: f32::NAN,  // 模拟运行期构造的非有限进度。
        transition_duration_game_seconds: f32::INFINITY, // 模拟非有限过渡时长。
        stable_remaining_game_seconds: -10.0, // 模拟负稳定剩余时长。
        region: WeatherRegion::Global,  // 保留合法全局区域。
        seed: 0,                        // 模拟缺失或损坏随机种子。
        sequence: 99,                   // 随机序号本身可以合法保留。
        ..WeatherSnapshot::default()    // 新增地表字段使用安全干燥默认值。
    }); // 完成损坏天气快照恢复。
    assert_eq!(state.target(), WeatherKind::Overcast); // 晴转大雨必须路由为先转阴。
    assert_eq!(state.transition_progress(), 0.0); // 改变目标后过渡从零重新开始。
    assert_eq!(
        state.transition_duration_game_seconds(), // 读取修复后的过渡总时长。
        DEFAULT_TRANSITION_GAME_SECONDS,          // 非有限时长回退到二十五游戏分钟。
    ); // 完成过渡时长修复断言。
    assert_eq!(state.stable_remaining_game_seconds(), 0.0); // 负稳定时长钳制到零。
    assert_eq!(state.snapshot().pending_minimum_hold_game_seconds, 0.0); // 非有限最低维持时间不得锁死自动天气。
    assert_eq!(state.seed(), DEFAULT_WEATHER_SEED); // 零种子恢复为项目默认种子。
    assert_eq!(state.sequence(), 99); // 合法随机序号保持不变。
} // 结束损坏快照恢复测试。

#[test] // force_transition必须禁止晴天直接跳大雨。
fn forced_transition_respects_weather_graph() {
    let mut state = WeatherState::default(); // 从晴天稳定状态开始。
    state.force_transition(WeatherKind::HeavyRain, 600.0, 0.0); // 请求十分钟内从晴天进入大雨。
    assert_eq!(state.current(), WeatherKind::Clear); // 过渡起点仍为晴天。
    assert_eq!(state.target(), WeatherKind::Overcast); // 安全下一节点只能是阴天。
    assert_eq!(state.transition_duration_game_seconds(), 600.0); // 合法外部时长应原样保留。
    state.advance_game_seconds(600.0, 6, 12.0, UNIFORM_CLIMATE_PROFILE); // 完成晴转阴第一段。
    assert_eq!(state.current(), WeatherKind::Overcast); // 第一段完成后当前节点进入阴天。
    assert_eq!(state.target(), WeatherKind::LightRain); // 状态机应记住最终意图并继续形成小雨。
    state.advance_game_seconds(600.0, 6, 12.0, UNIFORM_CLIMATE_PROFILE); // 完成阴转小雨第二段。
    assert_eq!(state.current(), WeatherKind::LightRain); // 第二段完成后当前节点进入小雨。
    assert_eq!(state.target(), WeatherKind::HeavyRain); // 最后一段继续增强为最初请求的大雨。
    state.advance_game_seconds(600.0, 6, 12.0, UNIFORM_CLIMATE_PROFILE); // 完成小雨转大雨最后一段。
    assert_eq!(state.current(), WeatherKind::HeavyRain); // 多段路径最终必须命中剧情请求目标。
    assert_eq!(state.target(), WeatherKind::HeavyRain); // 到达最终目标后进入正常稳定期。
    assert!(!state.is_transitioning()); // 完整路径结束后不应残留排队过渡。
} // 结束强制过渡状态图测试。

#[test] // 重复发送相同剧情目标不能把已经推进的连续天气过渡重置到零。
fn repeated_forced_transition_is_idempotent() {
    let mut state = WeatherState::default(); // 从晴天开始构造多段大雨请求。
    state.force_transition(WeatherKind::HeavyRain, 600.0, 0.0); // 首次请求沿状态图先转阴。
    state.advance_game_seconds(240.0, 6, 12.0, UNIFORM_CLIMATE_PROFILE); // 把第一段推进到四成进度。
    let progress_before = state.transition_progress(); // 记录重复命令到达前的连续进度。
    state.force_transition(WeatherKind::HeavyRain, 600.0, 0.0); // 剧情系统下一帧重复发出相同目标。
    assert_eq!(state.transition_progress(), progress_before); // 重复目标不得清零或改变现有进度。
    assert_eq!(state.target(), WeatherKind::Overcast); // 当前合法中间目标保持不变。
} // 结束重复剧情天气命令幂等测试。

#[test] // 过渡中改为更远目标必须保留当前连续段进度，只替换后续最终意图。
fn retarget_during_transition_keeps_current_segment_progress() {
    let mut state = WeatherState::default(); // 从晴天开始构造接近完成的转阴过程。
    state.force_transition(WeatherKind::Overcast, 600.0, 0.0); // 首先请求十分钟转阴。
    state.advance_game_seconds(480.0, 6, 12.0, UNIFORM_CLIMATE_PROFILE); // 推进到八成，视觉已经接近阴天。
    let progress_before = state.transition_progress(); // 保存中途命令前的连续进度。
    state.force_transition(WeatherKind::HeavyRain, 720.0, 0.0); // 八成时改为最终进入大雨。
    assert_eq!(state.current(), WeatherKind::Clear); // 当前离散起点在本段完成前保持晴天。
    assert_eq!(state.target(), WeatherKind::Overcast); // 当前合法相邻段必须继续完成而不是重新开始。
    assert_eq!(state.transition_progress(), progress_before); // 新目标不得把八成画面跳回完整晴天。
    assert_eq!(state.snapshot().queued_target, Some(WeatherKind::HeavyRain)); // 新最终意图只写入后续路径。
    assert_eq!(
        state.snapshot().queued_transition_duration_game_seconds,
        720.0,
    ); // 后续相邻段使用新命令时长，不篡改正在进行的本段时长。
    state.advance_game_seconds(120.0, 6, 12.0, UNIFORM_CLIMATE_PROFILE); // 完成原晴转阴剩余两成。
    assert_eq!(state.current(), WeatherKind::Overcast); // 原段应平滑到达阴天。
    assert_eq!(state.target(), WeatherKind::LightRain); // 随后才按新目标继续向小雨过渡。
    assert!(state.transition_progress().abs() < 0.000_001); // 浮点剩余秒可能带来极小误差，新相邻段仍应从完整阴天近零起步。
} // 结束过渡中向前改目标测试。

#[test] // 过渡中反向请求也不得立即交换离散端点造成分阶段雨效回跳。
fn reverse_request_finishes_current_segment_before_returning() {
    let mut state = WeatherState::default(); // 从晴天开始向阴天过渡。
    state.force_transition(WeatherKind::Overcast, 600.0, 0.0); // 启动单段晴转阴。
    state.advance_game_seconds(480.0, 6, 12.0, UNIFORM_CLIMATE_PROFILE); // 推进到八成连续画面。
    let progress_before = state.transition_progress(); // 记录反向请求前的视觉进度。
    state.force_transition(WeatherKind::Clear, 300.0, 0.0); // 请求返回晴天并使用五分钟后续时长。
    assert_eq!(state.target(), WeatherKind::Overcast); // 当前段仍先完成到阴天，避免交换端点时输出曲线不对称。
    assert_eq!(state.transition_progress(), progress_before); // 反向请求同样不能瞬间改变当前画面。
    assert_eq!(state.snapshot().queued_target, Some(WeatherKind::Clear)); // 晴天作为抵达阴天后的反向目标排队。
    state.advance_game_seconds(120.0, 6, 12.0, UNIFORM_CLIMATE_PROFILE); // 完成当前段剩余时间。
    assert_eq!(state.current(), WeatherKind::Overcast); // 当前节点平滑落到阴天。
    assert_eq!(state.target(), WeatherKind::Clear); // 随即从完整阴天开始反向转晴。
    assert_eq!(state.transition_duration_game_seconds(), 300.0); // 反向段使用新命令时长。
} // 结束过渡中反向目标平滑处理测试。

#[test] // 剧情指定的最低维持时间必须在最终天气到达后覆盖较短的自然持续时间。
fn scripted_minimum_hold_applies_after_final_target() {
    let mut state = WeatherState::default(); // 从晴天稳定状态开始相邻剧情过渡。
    let minimum_hold = 8.0 * 3_600.0; // 阴天自然最长七小时，用八小时保证测试能识别覆盖。
    state.force_transition(WeatherKind::Overcast, 600.0, minimum_hold); // 十分钟后进入至少维持八小时的阴天。
    state.advance_game_seconds(600.0, 6, 12.0, UNIFORM_CLIMATE_PROFILE); // 精确完成相邻过渡但不消费新稳定期。
    assert_eq!(state.current(), WeatherKind::Overcast); // 剧情目标必须已经生效。
    assert_eq!(state.target(), WeatherKind::Overcast); // 最终天气进入稳定状态。
    assert_eq!(state.stable_remaining_game_seconds(), minimum_hold); // 最低维持时间必须覆盖随机自然时长。
    assert_eq!(state.snapshot().pending_minimum_hold_game_seconds, 0.0); // 已兑现时长不能在下次天气重复应用。
} // 结束剧情最低维持时间测试。

#[test] // 多段剧情天气的最终目标和后续单段时长必须随快照完整恢复。
fn queued_transition_roundtrips_through_snapshot() {
    let mut state = WeatherState::default(); // 从默认晴天构造跨节点剧情过渡。
    state.force_transition(WeatherKind::HeavyRain, 720.0, 0.0); // 请求每段十二游戏分钟最终进入大雨。
    state.advance_game_seconds(240.0, 6, 12.0, UNIFORM_CLIMATE_PROFILE); // 推进第一段三分之一后保存。
    let mut restored = WeatherState::default(); // 构造独立目标状态模拟读档。
    restored.restore(state.snapshot()); // 恢复进行中的多段天气快照。
    assert_eq!(restored.current(), WeatherKind::Clear); // 读档后仍从晴天向中间节点过渡。
    assert_eq!(restored.target(), WeatherKind::Overcast); // 当前直接目标仍是合法阴天节点。
    restored.advance_game_seconds(1_920.0, 6, 12.0, UNIFORM_CLIMATE_PROFILE); // 消费剩余三段共一千九百二十游戏秒。
    assert_eq!(restored.current(), WeatherKind::HeavyRain); // 恢复后必须继续命中原最终目标。
    assert!(!restored.is_transitioning()); // 完成后进入大雨稳定期。
} // 结束多段过渡快照往返测试。

#[test] // 九点半薄雾应开始转晴，十点后必须完全清除。
fn morning_mist_disperses_before_ten() {
    let mut state = WeatherState::from_snapshot(WeatherSnapshot {
        current: WeatherKind::Mist,                   // 从稳定薄雾开始。
        target: WeatherKind::Mist,                    // 当前没有天气过渡。
        stable_remaining_game_seconds: 3.0 * 3_600.0, // 模拟仍有三小时稳定期。
        ..WeatherSnapshot::default()                  // 其余快照字段使用合法默认值。
    }); // 完成薄雾状态构造。
    state.advance_game_seconds(0.0, 9, 9.5, UNIFORM_CLIMATE_PROFILE); // 模拟F1暂停时把时刻拖到九点半。
    assert_eq!(state.target(), WeatherKind::Clear); // 九点半应开始向晴天消散。
    assert!(state.is_transitioning()); // 消散过程应保持半小时平滑过渡。
    state.advance_game_seconds(0.0, 9, 10.0, UNIFORM_CLIMATE_PROFILE); // 模拟继续把时刻拖到十点整。
    assert_eq!(state.current(), WeatherKind::Clear); // 十点时薄雾必须完全清除。
    assert!(!state.is_transitioning()); // 硬性清除后不应残留过渡状态。
} // 结束晨雾消散测试。

#[test] // 九点半后才触发散雾时必须使用距十点的实际剩余时长。
fn late_morning_mist_uses_remaining_time_before_ten() {
    let mut state = WeatherState::from_snapshot(WeatherSnapshot {
        current: WeatherKind::Mist,                   // 从稳定薄雾开始。
        target: WeatherKind::Mist,                    // 当前尚未开始散雾。
        stable_remaining_game_seconds: 3.0 * 3_600.0, // 保证触发原因只来自时刻约束。
        ..WeatherSnapshot::default()                  // 其余字段保持合法默认值。
    }); // 完成稳定薄雾状态构造。
    state.advance_game_seconds(0.0, 9, 9.75, UNIFORM_CLIMATE_PROFILE); // 模拟暂停时由F12把时间直接调到09:45。
    assert_eq!(state.target(), WeatherKind::Clear); // 晚触发仍必须开始向晴天消散。
    assert_eq!(state.transition_duration_game_seconds(), 15.0 * 60.0); // 09:45只剩十五游戏分钟，不能重新使用完整半小时。
} // 结束晚触发晨雾剩余时长测试。

#[test] // 已经散到一半后拖动时刻必须保持当前雾量并重新对齐十点截止点。
fn retiming_mist_dispersal_preserves_progress() {
    let mut state = WeatherState::from_snapshot(WeatherSnapshot {
        current: WeatherKind::Mist,                    // 当前仍处于薄雾离散端点。
        target: WeatherKind::Clear,                    // 已经在向晴天散雾。
        transition_progress: 0.5,                      // 画面已经完成一半混合。
        transition_duration_game_seconds: 30.0 * 60.0, // 原始过渡按九点半开始计算。
        stable_remaining_game_seconds: 0.0,            // 过渡期间不消费稳定期。
        ..WeatherSnapshot::default()                   // 其余字段保持合法默认值。
    }); // 完成进行中散雾状态构造。
    state.advance_game_seconds(0.0, 9, 9.9, UNIFORM_CLIMATE_PROFILE); // 模拟暂停时把时间拖到09:54。
    assert_eq!(state.transition_progress(), 0.5); // 调整截止时间不能改变同帧画面雾量。
    let remaining_seconds =
        (1.0 - state.transition_progress()) * state.transition_duration_game_seconds(); // 按状态机公式计算当前过渡实际剩余游戏秒。
    assert!((remaining_seconds - 6.0 * 60.0).abs() < 0.01); // 剩余一半进度必须恰好在六分钟后于10:00完成。
} // 结束进行中晨雾重定时测试。

#[test] // 接近散尽后把时间向回拖不能产生无法稳定存档的超大总过渡时长。
fn retimed_mist_duration_remains_snapshot_safe() {
    let mut state = WeatherState::from_snapshot(WeatherSnapshot {
        current: WeatherKind::Mist,     // 当前仍以薄雾作为离散起点。
        target: WeatherKind::Clear,     // 正在直接向晴天散雾。
        transition_progress: 0.999_999, // 画面已经几乎完全晴朗。
        transition_duration_game_seconds: 30.0 * 60.0, // 原始自然散雾使用半小时。
        stable_remaining_game_seconds: 0.0, // 过渡期间没有稳定等待。
        ..WeatherSnapshot::default()    // 其余字段保持合法默认值。
    }); // 完成接近散尽的状态构造。
    state.advance_game_seconds(0.0, 9, 9.5, UNIFORM_CLIMATE_PROFILE); // 模拟暂停时把时间从十点前向回拖到09:30。
    assert_eq!(state.transition_progress(), 0.999_999); // 不能为了拉长时长重新增加画面雾量。
    assert_eq!(
        state.transition_duration_game_seconds(),
        MAX_SNAPSHOT_TRANSITION_GAME_SECONDS,
    ); // 数学总时长钳制到快照允许上限，剩余极小尾量可早于十点散尽。
    let restored = WeatherState::from_snapshot(state.snapshot()); // 模拟在该极端调试状态下保存并立即读取。
    assert_eq!(
        restored.transition_duration_game_seconds(),
        state.transition_duration_game_seconds(),
    ); // 存读档不得再次改变散雾速度。
    assert_eq!(restored.transition_progress(), state.transition_progress()); // 当前连续画面也必须完整往返。
} // 结束晨雾重定时快照安全测试。

#[test] // 薄雾正在转阴时不得回跳端点，应完成当前段后在截止前继续转晴。
fn outgoing_mist_segment_keeps_progress_and_queues_clear() {
    let mut state = WeatherState::from_snapshot(WeatherSnapshot {
        current: WeatherKind::Mist,                    // 从薄雾端开始自然转阴。
        target: WeatherKind::Overcast,                 // 当前相邻目标不是晴天。
        transition_progress: 0.5,                      // 画面已经混合一半阴天效果。
        transition_duration_game_seconds: 30.0 * 60.0, // 原始过渡总时长不应决定十点截止点。
        stable_remaining_game_seconds: 0.0,            // 过渡期间没有稳定等待。
        ..WeatherSnapshot::default()                   // 其余字段保持合法默认值。
    }); // 完成薄雾转阴状态构造。
    state.advance_game_seconds(0.0, 9, 9.75, UNIFORM_CLIMATE_PROFILE); // 模拟F1暂停时把时刻拖到09:45。
    assert_eq!(state.current(), WeatherKind::Mist); // 当前插值起点必须保持薄雾，不能离散重写画面。
    assert_eq!(state.target(), WeatherKind::Overcast); // 当前段仍先完成转阴，不突然改成薄雾转晴。
    assert_eq!(state.transition_progress(), 0.5); // 已完成的一半混合比例必须原样保留。
    assert_eq!(state.snapshot().queued_target, Some(WeatherKind::Clear)); // 阴天抵达后立即排队转晴。
    let snapshot = state.snapshot(); // 读取两段重新分配后的时长。
    let total_remaining = (1.0 - snapshot.transition_progress)
        * snapshot.transition_duration_game_seconds
        + snapshot.queued_transition_duration_game_seconds; // 当前半段与下一整段共同占用截止前时间。
    assert!((total_remaining - 15.0 * 60.0).abs() < 0.01); // 09:45起两段总计必须恰好十五游戏分钟。
    state.advance_game_seconds(15.0 * 60.0, 9, 9.75, UNIFORM_CLIMATE_PROFILE); // 消费到十点前的全部剩余时间。
    assert_eq!(state.current(), WeatherKind::Clear); // 当前段和排队段完成后必须稳定到晴天。
    assert!(!state.is_transitioning()); // 截止点不能残留未完成天气过渡。
} // 结束薄雾转阴连续路径测试。

#[test] // 九点半后正在形成的薄雾必须从当前画面反向，不能继续加重或产生视觉跳变。
fn incoming_mist_reverses_without_output_jump() {
    let mut state = WeatherState::from_snapshot(WeatherSnapshot {
        current: WeatherKind::Overcast, // 从阴天端开始形成薄雾。
        target: WeatherKind::Mist,      // 当前相邻目标是薄雾。
        transition_progress: 0.25,      // 已经出现四分之一原始过渡的雾感。
        transition_duration_game_seconds: 30.0 * 60.0, // 使用合法普通过渡时长。
        stable_remaining_game_seconds: 0.0, // 过渡期间没有稳定等待。
        ..WeatherSnapshot::default()    // 其余字段保持合法默认值。
    }); // 完成阴天转薄雾状态构造。
    let debug = WeatherDebugOverride::default(); // 不使用F1强制覆盖，比较真实自动输出。
    let before = resolved_weather_output(&state, &debug); // 保存反向前当前混合画面。
    state.advance_game_seconds(0.0, 9, 9.75, UNIFORM_CLIMATE_PROFILE); // 模拟暂停时跨入强制散雾窗口。
    let after = resolved_weather_output(&state, &debug); // 读取端点交换后的同帧输出。
    assert_eq!(state.current(), WeatherKind::Mist); // 端点交换后从薄雾侧反向离开。
    assert_eq!(state.target(), WeatherKind::Overcast); // 先无跳变地退回原阴天端点。
    assert_eq!(state.transition_progress(), 0.75); // 互补进度与交换端点共同保持同一插值位置。
    assert_eq!(state.snapshot().queued_target, Some(WeatherKind::Clear)); // 返回阴天后继续在十点前转晴。
    assert!((before.cloud_cover - after.cloud_cover).abs() < 0.000_001); // 云量不能因反向处理跳变。
    assert!((before.visibility - after.visibility).abs() < 0.000_001); // 能见度不能因反向处理跳变。
    assert!((before.direct_light_factor - after.direct_light_factor).abs() < 0.000_001); // 直射光压制必须连续。
    assert!(
        (before.gameplay.ai_visibility_factor - after.gameplay.ai_visibility_factor).abs()
            < 0.000_001
    ); // AI视认倍率与视觉雾量使用同一连续位置。
} // 结束入雾过渡连续反向测试。

#[test] // 调试天气索引和预览值必须安全覆盖全部端点。
fn debug_override_maps_modes_and_clamps_preview() {
    let mut debug = WeatherDebugOverride::default(); // 默认从自动模式开始。
    assert_eq!(debug.mode_index(), 0); // 自动模式索引固定为零。
    debug.set_mode_index(5); // 选择第五个天气模式。
    assert_eq!(debug.forced(), Some(WeatherKind::Mist)); // 索引五必须映射薄雾。
    debug.set_preview(2.0); // 模拟UI传入超上限预览值。
    assert_eq!(debug.preview(), 1.0); // 预览值必须钳制到完整效果。
    debug.set_mode_index(99); // 模拟越界离散索引。
    assert_eq!(debug.forced(), None); // 越界索引安全回到自动模式。
} // 结束调试覆盖接口测试。
