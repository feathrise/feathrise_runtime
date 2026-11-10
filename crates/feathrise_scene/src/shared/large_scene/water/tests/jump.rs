//! 玩家水中起跳、落水、连续跳和低帧率采样边界回归。
//! 测试复用父模块的正式水面构造与纯逻辑推进入口，不维护第二套状态机。

use super::*; // 复用父测试模块的生产常量、状态构造和序号推进helper。

#[test] // 深水跳不穿出静止水位时，实际起跳立即发强波，真实着底再追加同规格强波。
fn deep_water_jump_emits_once_when_fall_stops() {
    let mut water_surface = test_water_surface(); // 构造空波源池和未接触状态。
    let frame_seconds = 1.0 / 60.0; // 使用六十帧帧长把五厘米位移转换为每秒三米。
    let deep_water_foot_y = water_surface.rest_water_level - 2.4; // 脚底位于支持范围内的真实河床深度。
    let _entry = update_player_ripples(
        &mut water_surface,
        Some(Vec3::new(0.0, deep_water_foot_y, 0.0)),
        frame_seconds,
    ); // 首帧标准入水圈同时建立高度基准。
    let rising = update_player_ripples_with_takeoff(
        &mut water_surface,
        Some(Vec3::new(0.02, deep_water_foot_y + 0.05, 0.0)),
        1, // Tnua已经实际接受本次短按跳跃并发布第一个起跳序号。
        frame_seconds,
    ); // 实际起跳序号立即追加与落水相同的大波并武装当前周期。
    assert_eq!(
        rising.map(|ripple| ripple.amplitude),
        Some(PLAYER_RIPPLE_JUMP_AMPLITUDE)
    ); // 短按起跳必须立即返回与落水完全相同的九厘米强波。
    assert_eq!(water_surface.player_ripples.len(), 2); // 初次入水与起跳大波必须同时保留在统一八槽。
    assert_eq!(
        water_surface.player_ripples.last().unwrap().center_local_xz,
        Vec2::new(0.02, 0.0)
    ); // 脚底仍在合法深水时起跳大波必须采用本帧真实起跳点，不能落后到上一帧位置。
    let falling = update_player_ripples(
        &mut water_surface,
        Some(Vec3::new(0.02, deep_water_foot_y, 0.0)),
        frame_seconds,
    ); // 五厘米单帧下降确认已经进入回落段。
    assert!(falling.is_none()); // 尚未着底停止时不应提前生成冲击波。
    assert!(water_surface.player_ripples[1].age > 0.0); // 下落期间起跳大波必须继续老化而不是被周期状态重置。
    let landed = update_player_ripples(
        &mut water_surface,
        Some(Vec3::new(0.02, deep_water_foot_y, 0.0)),
        frame_seconds,
    ); // 同高度下一帧代表下降速度归零并真实着底。
    assert_eq!(
        landed.map(|ripple| ripple.amplitude),
        Some(PLAYER_RIPPLE_JUMP_AMPLITUDE)
    ); // 深水着底必须获得本周期唯一跳跃强波。
    assert_eq!(water_surface.player_ripples.len(), 3); // 初次入水、起跳和本次着底三圈应同时保留，旧波没有被重置。
} // 结束深水着底跳跃回归测试。

#[test] // 深水连续按跳在着底帧再次开始时，必须保留合并强波并为下一周期重新武装。
fn deep_water_continuous_jump_keeps_landing_ripple_and_rearms_takeoff() {
    let mut water_surface = test_water_surface(); // 构造无待处理周期的正式八槽统一水面状态。
    let frame_seconds = 1.0 / 60.0; // 使用六十帧帧长产生明确升降速度。
    let foot_y = water_surface.rest_water_level - 2.4; // 使用不会穿出水面的深水脚底高度。
    let _entry = update_player_ripples(
        &mut water_surface,
        Some(Vec3::new(0.0, foot_y, 0.0)),
        frame_seconds,
    ); // 首帧入水并建立高度历史。
    let first_takeoff = update_player_ripples_with_takeoff(
        &mut water_surface,
        Some(Vec3::new(0.0, foot_y + 0.05, 0.0)),
        1, // 发布第一轮Tnua实际起跳身份。
        frame_seconds,
    ); // 第一次实际起跳追加统一大波并武装周期。
    assert_eq!(
        first_takeoff.map(|ripple| ripple.amplitude),
        Some(PLAYER_RIPPLE_JUMP_AMPLITUDE)
    ); // 第一轮起跳必须立即获得长期强波。
    let _falling = update_player_ripples(
        &mut water_surface,
        Some(Vec3::new(0.0, foot_y, 0.0)),
        frame_seconds,
    ); // 明确下降段让周期等待下一次速度反转结算。
    let landing_and_next_takeoff = update_player_ripples_with_takeoff(
        &mut water_surface,
        Some(Vec3::new(0.0, foot_y + 0.05, 0.0)),
        2, // 同一渲染帧收到第二轮Tnua ActionStarted，必须与刚着底强波合并。
        frame_seconds,
    ); // 着底后的立即上升在同一帧结算旧周期并武装新周期。
    assert_eq!(
        landing_and_next_takeoff.map(|ripple| ripple.amplitude),
        Some(PLAYER_RIPPLE_JUMP_AMPLITUDE)
    ); // 上一周期的深水着底仍必须追加一枚长期强波。
    assert_eq!(water_surface.player_ripples.len(), 3); // 初始入水、第一轮起跳和合并后的着底/再起跳强波必须同时保留。
    assert_eq!(water_surface.player_ripples.last().unwrap().age, 0.0); // 合并事件只追加一枚零年龄强波，不能浪费两个同相槽位。
    assert!(matches!(
        water_surface.jump_cycle,
        WaterJumpCycle::Armed { .. }
    )); // 落水长期圈生成后必须已为下一次真实着底武装新周期。
} // 结束深水连续跳跃衔接回归测试。

#[test] // 单帧Tnua ActionStarted必须立即生成完整大波，不依赖持续按键或渲染帧高度差。
fn single_frame_takeoff_sequence_emits_full_ripple_without_hold() {
    let mut water_surface = test_water_surface(); // 构造与正式运行时相同的统一八槽波源池。
    let deep_water_foot_y = water_surface.rest_water_level - 2.0; // 使用支持交互的深水脚底高度。
    let _entry = update_player_ripples(
        &mut water_surface,                           // 首帧建立合法水中接触。
        Some(Vec3::new(0.0, deep_water_foot_y, 0.0)), // 玩家位于水面足迹中心。
        1.0 / 60.0,                                   // 使用常见帧长建立竖直历史。
    ); // 普通入水占用第一个统一槽并建立序号零基线。
    let takeoff = update_player_ripples_with_takeoff(
        &mut water_surface,                           // 第二帧只发布一次真实起跳身份。
        Some(Vec3::new(0.0, deep_water_foot_y, 0.0)), // 故意保持完全相同高度，证明不再依赖Transform帧差。
        1,                                            // 模拟短按已经被Tnua真正接受的ActionStarted。
        1.0 / 60.0,                                   // 只持续一个渲染帧。
    ); // 起跳必须立即进入统一长期池。
    assert_eq!(
        takeoff.map(|ripple| ripple.amplitude),
        Some(PLAYER_RIPPLE_JUMP_AMPLITUDE)
    ); // 单帧短按必须获得与落水完全相同的九厘米大波。
    assert_eq!(water_surface.player_ripples.len(), 2); // 入水圈与起跳大波必须同时存在。
    let held_without_new_action = update_player_ripples_with_takeoff(
        &mut water_surface,                           // 模拟下一帧按键状态仍被喂给Tnua。
        Some(Vec3::new(0.0, deep_water_foot_y, 0.0)), // 玩家位置仍未改变。
        1,          // ActionStarted序号没有增加，表示同一跳跃持续而非新跳。
        1.0 / 60.0, // 再推进一个渲染帧。
    ); // 相同序号不得重复发波。
    assert!(held_without_new_action.is_none()); // 长按电平本身不能每帧生成新的起跳波。
    assert_eq!(water_surface.player_ripples.len(), 2); // 同一实际跳跃只能占用一个起跳槽。
} // 结束单帧实际起跳大波回归测试。

#[test] // 低帧率漏掉上一轮完整起落时，下一次权威起跳必须接管旧周期而不能被永久吞掉。
fn next_takeoff_recovers_when_previous_complete_cycle_was_between_render_samples() {
    let mut water_surface = test_water_surface(); // 构造正式八槽池和空闲跳跃周期。
    let frame_seconds = 1.0 / 60.0; // 入水基线后只观察两次起跳采样，模拟中间发生多个固定物理步。
    let shallow_y = water_surface.rest_water_level - 0.05; // 玩家脚底位于合法五厘米浅水。
    let position = Some(Vec3::new(0.0, shallow_y, 0.0)); // 两次表现采样都落在同一水中位置。
    let _entry = update_player_ripples(&mut water_surface, position, frame_seconds); // 首帧建立水中接触与序号零基线。
    let first_takeoff = update_player_ripples_with_takeoff(
        &mut water_surface,
        position,      // 故意不依赖本帧高度变化。
        1,             // 第一轮实际起跳由Tnua确认。
        frame_seconds, // 水面立即发出第一枚起跳大波并进入Armed。
    );
    assert_eq!(
        first_takeoff.map(|ripple| ripple.amplitude),
        Some(PLAYER_RIPPLE_JUMP_AMPLITUDE)
    ); // 第一轮起跳必须正常发波。
    assert!(matches!(
        water_surface.jump_cycle,
        WaterJumpCycle::Armed { .. }
    )); // 没采到空中与落水帧时旧周期仍保持武装，复现低帧率边界。
    let next_takeoff = update_player_ripples_with_takeoff(
        &mut water_surface,
        position,      // 下一次表现采样时角色已经完成上一跳并再次位于水中起跳。
        2,             // 新ActionStarted是上一周期已经结束的权威证据。
        frame_seconds, // 新事件应接管旧Armed并重新武装，而不是先消费后丢弃。
    );
    assert_eq!(
        next_takeoff.map(|ripple| ripple.amplitude),
        Some(PLAYER_RIPPLE_JUMP_AMPLITUDE)
    ); // 第二轮实际起跳仍必须立即获得同规格大波。
    assert_eq!(water_surface.player_ripples.len(), 3); // 入水圈与两轮起跳大波必须同时保留。
    assert!(matches!(
        water_surface.jump_cycle,
        WaterJumpCycle::Armed { .. }
    )); // 接管后必须为第二轮落水重新建立干净周期。
} // 结束低帧率漏采完整起落恢复回归。

#[test] // 两个空中采样之间完成落水并再次起跳时，新ActionStarted也必须接管旧浅水周期。
fn next_takeoff_recovers_when_landing_happens_between_airborne_samples() {
    let mut water_surface = test_water_surface(); // 构造正式浅水接触状态和统一八槽池。
    let frame_seconds = 1.0 / 60.0; // 表现帧之间允许Tnua与物理推进多个固定步。
    let rest_water_level = water_surface.rest_water_level; // 冻结测试静止水位。
    let shallow_position = Some(Vec3::new(0.0, rest_water_level - 0.05, 0.0)); // 首帧脚底位于合法五厘米浅水。
    let airborne_position = Some(Vec3::new(0.0, rest_water_level + 0.05, 0.0)); // 后续两次表现采样都位于同一水面上空。
    let _entry = update_player_ripples(&mut water_surface, shallow_position, frame_seconds); // 建立合法水中接触与序号基线。
    let first_takeoff = update_player_ripples_with_takeoff(
        &mut water_surface,
        airborne_position, // 第一跳已经穿出静止水位。
        1,                 // Tnua发布第一轮实际起跳。
        frame_seconds,     // 发起跳大波并把周期标记为已经离水。
    );
    assert_eq!(
        first_takeoff.map(|ripple| ripple.amplitude),
        Some(PLAYER_RIPPLE_JUMP_AMPLITUDE)
    ); // 第一轮浅水起跳必须正常发波。
    assert!(!water_surface.player_was_in_water); // 第一空中采样后只剩Armed周期提供水域上下文。
    assert!(matches!(
        water_surface.jump_cycle,
        WaterJumpCycle::Armed {
            left_surface: true,
            ..
        }
    )); // 明确复现已观察到离水但尚未观察到落水的状态。
    let second_takeoff = update_player_ripples_with_takeoff(
        &mut water_surface,
        airborne_position, // 两次表现采样之间已落水并再次起跳，因此本帧又位于空中。
        2,                 // 第二个ActionStarted是中间真实落地的权威证据。
        frame_seconds,     // 旧周期必须被合并接管，而不能因player_was_in_water=false吞掉事件。
    );
    assert_eq!(
        second_takeoff.map(|ripple| ripple.amplitude),
        Some(PLAYER_RIPPLE_JUMP_AMPLITUDE)
    ); // 第二次短按起跳仍必须获得同规格大波。
    assert_eq!(water_surface.player_ripples.len(), 3); // 入水圈和两次起跳大波均应保留。
    assert!(matches!(
        water_surface.jump_cycle,
        WaterJumpCycle::Armed {
            left_surface: true,
            ..
        }
    )); // 第二次空中采样后新周期必须继续等待后续真实落水。
} // 结束低帧率浅水空中到空中接管回归。

#[test] // 玩家实体无空帧重建导致序号回退时只能重建基线，不能凭空生成水波。
fn takeoff_sequence_rollback_rebuilds_baseline_without_ghost_wave() {
    let mut water_surface = test_water_surface(); // 构造仍保留水中接触状态的正式水面。
    let frame_seconds = 1.0 / 60.0; // 固定帧长推进现有入水圈年龄。
    let foot_y = water_surface.rest_water_level - 0.05; // 玩家位于合法浅水。
    let position = Some(Vec3::new(0.0, foot_y, 0.0)); // 重建前后保持同一脚底位置以暴露幽灵触发风险。
    let _entry = update_player_ripples(&mut water_surface, position, frame_seconds); // 建立正常水中接触。
    water_surface.last_player_takeoff_sequence = Some(5); // 模拟旧玩家已完成五次实际起跳。
    let replacement_baseline = crate::shared::large_scene::water::ripple::update_player_ripples(
        &mut water_surface,
        position, // 新玩家在两次PostUpdate间原位替换，没有可见None空档。
        Some(0),  // Required Component让新玩家序号从零重新开始。
        frame_seconds,
    );
    assert!(replacement_baseline.is_none()); // 序号回退不能被误认成ActionStarted。
    assert_eq!(water_surface.player_ripples.len(), 1); // 只能保留此前入水圈，不得新增幽灵大波。
    assert_eq!(water_surface.last_player_takeoff_sequence, Some(0)); // 当前零值成为新玩家的干净比较基线。
    let real_takeoff_after_rebuild = update_player_ripples_with_takeoff(
        &mut water_surface,
        position,      // 新玩家随后仍在同一合法浅水位置。
        1,             // 新玩家第一次真实ActionStarted把序号从零增到一。
        frame_seconds, // 正常递增必须恢复发波能力。
    );
    assert_eq!(
        real_takeoff_after_rebuild.map(|ripple| ripple.amplitude),
        Some(PLAYER_RIPPLE_JUMP_AMPLITUDE)
    ); // 基线重建不能吞掉后续真正的起跳。
} // 结束玩家序号回退隔离回归。

#[test] // 原地连续浅水跳必须在起跳与落水各追加同规格大波，并只在第九次事件淘汰最旧圈。
fn repeated_same_position_jumps_fill_eight_slots_without_resetting_active_ripples() {
    let mut water_surface = test_water_surface(); // 使用正式八槽统一池验证连续叠加和满池FIFO。
    let frame_seconds = 1.0 / 60.0; // 短周期测试确保全部波源都远未到二点四秒寿命。
    let rest_water_level = water_surface.rest_water_level; // 冻结静止水位避免可变借用冲突。
    let shallow_y = rest_water_level - 0.05; // 初始脚底位于五厘米浅水中。
    let above_water_y = rest_water_level + 0.05; // 起跳帧直接穿出静止水位五厘米。
    let center = Vec2::ZERO; // 全部跳跃使用同一XZ，专门覆盖原地疯狂跳跃场景。
    let entry = update_player_ripples(
        &mut water_surface,
        Some(Vec3::new(center.x, shallow_y, center.y)),
        frame_seconds,
    ); // 普通首次入水先生成一圈中等波。
    assert_eq!(
        entry.map(|ripple| ripple.amplitude),
        Some(PLAYER_RIPPLE_ENTRY_AMPLITUDE)
    ); // 确认第一槽可通过不同振幅稳定识别。

    for cycle in 1_u64..=4 {
        let takeoff = update_player_ripples_with_takeoff(
            &mut water_surface,
            Some(Vec3::new(center.x, above_water_y, center.y)),
            cycle,
            frame_seconds,
        ); // 每轮真实ActionStarted在离水帧立即追加一枚完整大波。
        assert_eq!(
            takeoff.map(|ripple| ripple.amplitude),
            Some(PLAYER_RIPPLE_JUMP_AMPLITUDE)
        ); // 起跳与落水必须使用完全相同的强度和长期波形。
        assert_eq!(
            water_surface.player_ripples.len(),
            (cycle as usize * 2).min(PLAYER_RIPPLE_CAPACITY)
        ); // 前三轮保留入水圈；第四轮起跳恰好填满第八槽。
        let airborne_fall = update_player_ripples(
            &mut water_surface,
            Some(Vec3::new(center.x, rest_water_level + 0.02, center.y)),
            frame_seconds,
        ); // 仍位于水面上方的下降帧继续保留旧波和待落水周期。
        assert!(airborne_fall.is_none()); // 空中下降不能提前结算冲击。
        let landing = update_player_ripples(
            &mut water_surface,
            Some(Vec3::new(center.x, shallow_y, center.y)),
            frame_seconds,
        ); // 重新满足真实水深时在同一点追加同规格落水大波。
        assert_eq!(
            landing.map(|ripple| ripple.amplitude),
            Some(PLAYER_RIPPLE_JUMP_AMPLITUDE)
        ); // 每轮落水边沿也必须生成同一个九厘米长期波。
        assert_eq!(
            water_surface.player_ripples.len(),
            (1 + cycle as usize * 2).min(PLAYER_RIPPLE_CAPACITY)
        ); // 第九次事件仅淘汰最旧一圈，池长度始终不超过八。
        assert_eq!(water_surface.player_ripples.last().unwrap().age, 0.0); // 只有本轮最新落水圈从零年龄开始。
        assert!(
            water_surface
                .player_ripples
                .iter()
                .take(water_surface.player_ripples.len() - 1)
                .all(|ripple| ripple.age > 0.0)
        ); // 仍驻留的旧圈年龄必须持续前进，不能被新波整体重置。
        if cycle < 4 {
            assert_eq!(
                water_surface.player_ripples[0].amplitude,
                PLAYER_RIPPLE_ENTRY_AMPLITUDE
            ); // 八槽未满时最早的普通入水圈必须始终保留。
        } else {
            assert!(
                water_surface
                    .player_ripples
                    .iter()
                    .all(|ripple| ripple.amplitude == PLAYER_RIPPLE_JUMP_AMPLITUDE)
            ); // 第九次事件只按FIFO淘汰唯一的最旧入水圈。
        } // 完成未满池保留与满池淘汰断言。
    } // 完成四次完整起落，覆盖八槽填满和第九事件FIFO。
} // 结束原地连续跳跃统一大波叠加回归测试。

#[test] // 浅水完整跳跃必须在实际起跳与真实落水各追加一枚同规格大波，其余帧不得双发。
fn shallow_jump_cycle_emits_matching_takeoff_and_landing_ripples() {
    let mut water_surface = test_water_surface(); // 使用同一GLB接触门排除足迹差异。
    let frame_seconds = 1.0 / 60.0; // 固定帧长构造明确上升与下降速度。
    let rest_water_level = water_surface.rest_water_level; // 冻结静止水位供完整周期复用。
    let start_y = rest_water_level - 0.20; // 初始脚底在水下二十厘米。
    let _entry = update_player_ripples(
        &mut water_surface,
        Some(Vec3::new(0.0, start_y, 0.0)),
        frame_seconds,
    ); // 建立水中接触并发标准入水圈。
    let ripple_count_before_jump = water_surface.player_ripples.len(); // 锁定跳跃前仍在传播的旧波数量。
    let takeoff = update_player_ripples_with_takeoff(
        &mut water_surface,
        Some(Vec3::new(0.0, start_y + 0.10, 0.0)),
        1,
        frame_seconds,
    ); // Tnua实际接受跳跃时立即追加完整起跳大波，不依赖渲染帧高度差。
    assert_eq!(
        takeoff.map(|ripple| ripple.amplitude),
        Some(PLAYER_RIPPLE_JUMP_AMPLITUDE)
    ); // 起跳必须与落水使用同一个九厘米强波。
    let surface_exit = update_player_ripples(
        &mut water_surface,
        Some(Vec3::new(0.0, rest_water_level + 0.05, 0.0)),
        frame_seconds,
    ); // 下一帧穿出静止水位仍不生成波源。
    let airborne = update_player_ripples(
        &mut water_surface,
        Some(Vec3::new(0.0, rest_water_level + 0.10, 0.0)),
        frame_seconds,
    ); // 同一水面上空继续保留待落水周期。
    assert!(surface_exit.is_none() && airborne.is_none()); // ActionStarted之后的离水和空中帧不能重复追加波纹。
    assert_eq!(
        water_surface.player_ripples.len(),
        ripple_count_before_jump + 1
    ); // 起跳大波与此前旧圈必须同时存在。
    let landing = update_player_ripples(
        &mut water_surface,
        Some(Vec3::new(0.0, start_y, 0.0)),
        frame_seconds,
    ); // 下落重新进入当前水面时结算唯一冲击。
    assert_eq!(
        landing.map(|ripple| ripple.amplitude),
        Some(PLAYER_RIPPLE_JUMP_AMPLITUDE)
    ); // 完整浅水跳只在落水时追加一枚强波。
    assert_eq!(
        water_surface.player_ripples.len(),
        ripple_count_before_jump + 2
    ); // 旧波、起跳大波和落水大波必须同时存在。
} // 结束浅水完整周期双大波回归测试。

#[test] // 玩家从水边横向跳出水平足迹时，不得在岸外启动起跳大波。
fn lateral_jump_outside_water_footprint_does_not_emit_takeoff_wave() {
    let mut water_surface = test_water_surface(); // 使用局部Z正负五米的矩形水面足迹。
    let in_water_foot_y = water_surface.rest_water_level - 0.20; // 脚底保持二十厘米合法水深。
    let _entry = update_player_ripples(
        &mut water_surface,                         // 首帧在岸内建立合法接触与位置历史。
        Some(Vec3::new(0.0, in_water_foot_y, 4.9)), // 距北侧岸线只剩十厘米。
        1.0 / 60.0,                                 // 使用固定帧长建立竖直速度基准。
    ); // 普通入水波与本边界测试无关。
    let outside_rise = update_player_ripples_with_takeoff(
        &mut water_surface, // 下一帧同时上升并横向越过岸线。
        Some(Vec3::new(0.0, in_water_foot_y + 0.05, 5.1)), // 位移不足传送阈值但当前点已经在水域外。
        1,                  // 即使Tnua已经确认实际起跳，岸外位置仍不能授权水波。
        1.0 / 60.0,         // 五厘米上升足以触发真实起跳速度，专门验证水平门控。
    ); // 岸外位置不得授权任何水面波源。
    assert!(outside_rise.is_none()); // 横向离水不能向长期池追加波纹。
    assert_eq!(water_surface.player_ripples.len(), 1); // 固定池中只能保留此前合法的普通入水圈。
    assert!(matches!(water_surface.jump_cycle, WaterJumpCycle::Idle)); // 岸外ActionStarted不得留下稍后补发的待落水周期。
} // 结束岸外起跳大波门控回归测试。

#[test] // 浅水起跳后的空中大位移必须取消待落水周期，不能在传送目标补发跳跃强波。
fn airborne_teleport_over_same_water_cancels_pending_landing() {
    let mut water_surface = test_water_surface(); // 测试水域X范围负十到正十，可在足迹内构造十八米大位移。
    let frame_seconds = 1.0 / 60.0; // 固定帧长建立正常起跳速度。
    let shallow_y = water_surface.rest_water_level - 0.05; // 玩家起点位于五厘米浅水中。
    let airborne_y = water_surface.rest_water_level + 0.05; // 空中脚底位于静止水位上方五厘米。
    let _entry = update_player_ripples(
        &mut water_surface,
        Some(Vec3::new(-9.0, shallow_y, 0.0)),
        frame_seconds,
    ); // 在水域西端建立合法接触与位置历史。
    let _takeoff = update_player_ripples_with_takeoff(
        &mut water_surface,
        Some(Vec3::new(-9.0, airborne_y, 0.0)),
        1,
        frame_seconds,
    ); // 原地穿出水面并武装待落水周期。
    let airborne_teleport = update_player_ripples(
        &mut water_surface,
        Some(Vec3::new(9.0, airborne_y, 0.0)),
        frame_seconds,
    ); // 下一空中帧仍在同一足迹内，但水平跨越十八米，必须识别为传送。
    assert!(airborne_teleport.is_none()); // 空中传送本身不能生成任何长期波源。
    assert!(matches!(water_surface.jump_cycle, WaterJumpCycle::Idle)); // 传送必须立即取消原地点武装的跳跃周期。
    let landing_after_teleport = update_player_ripples(
        &mut water_surface,
        Some(Vec3::new(9.0, shallow_y, 0.0)),
        frame_seconds,
    ); // 在目标位置重新进入浅水，只能按普通入水处理。
    assert_eq!(
        landing_after_teleport.map(|ripple| ripple.amplitude),
        Some(PLAYER_RIPPLE_ENTRY_AMPLITUDE)
    ); // 目标位置不得补发原跳跃周期的强落水波。
} // 结束同水域空中传送周期隔离测试。
