//! 正式大地图动态水面的背景波采样、玩家波纹采样和接触状态更新纯逻辑。

use bevy::prelude::*; // Vec2 与 Vec3 用于水面局部坐标和玩家脚底位置计算。

use super::components::{DynamicWaterSurface, WaterJumpCycle}; // 接触更新纯函数读写当前动态水面组件状态。
use super::constants::*; // 复用水波幅度、速度、寿命、容量和接触阈值。
use super::footprint::{WaterInteractionFootprint, water_interaction_sample}; // 水平静水与斜河按真实表面采样，瀑布明确禁用互动。
#[cfg(test)] // GPU迁移后完整顶点采样值只用于Shader参考公式测试。
use super::types::PlayerRippleSample;
use super::types::PlayerWaterRipple; // 运行时统一维护固定容量的入水、脚步、起跳与落水波源。

/// 将零到一权重转换为两端无折角的三次平滑曲线。
#[cfg(test)] // 逐顶点平滑已迁移WGSL，CPU实现只保留为语义回归基准。
fn smoothstep_unit(value: f32) -> f32 {
    // 所有岸边和波圈包络共用同一平滑公式，保持视觉衰减一致。
    let clamped = value.clamp(0.0, 1.0); // 防止调用方的距离误差把权重推出合法范围。
    clamped * clamped * (3.0 - 2.0 * clamped) // 返回三次 smoothstep 结果。
} // 结束单位平滑权重计算。

/// 根据横向 UV 计算从两岸零逐渐增加到河心一的波浪权重。
#[cfg(test)] // 运行时由GPU读取统一岸带参数计算，CPU函数只服务纯逻辑测试。
pub(super) fn calculate_bank_fade(rest_uv: &[f32; 2]) -> f32 {
    // V=0 与 V=1 对应两岸埋边，只在窄岸带内压低波幅而不削弱大部分河面。
    let nearest_bank_uv_distance = rest_uv[1].min(1.0 - rest_uv[1]).max(0.0); // 计算顶点到最近一侧河岸的横向 UV 距离。
    let bank_fade_ratio = (nearest_bank_uv_distance / BANK_WAVE_FADE_UV_WIDTH).clamp(0.0, 1.0); // 将外侧岸带映射为零到一的局部衰减比例。
    smoothstep_unit(bank_fade_ratio) // 平滑固定边界顶点，进入河道内部后恢复完整几何波幅。
} // 结束岸边衰减计算。

/// 计算一个水面顶点的组合波高，并在两侧河岸平滑衰减到零。
#[cfg(test)] // 运行时背景缓波完全由顶点Shader执行，CPU公式只锁定视觉语义。
pub(super) fn calculate_wave_height(
    rest_position: &[f32; 3], // 当前顶点的静止局部坐标。
    rest_uv: &[f32; 2],       // 当前顶点的静止 UV。
    elapsed: f32,             // 当前连续运行秒数。
) -> f32 {
    // 使用两组清晰大波，并通过横向 UV 固定河岸边界。
    let bank_fade = calculate_bank_fade(rest_uv); // 使用共享岸边权重固定水面两侧边界。
    let primary_phase = rest_position[0] * 0.105 + rest_position[2] * 0.035 - elapsed * 1.10; // 沿河道传播的宽缓主波。
    let crossing_phase = rest_position[0] * 0.045 - rest_position[2] * 0.140 + elapsed * 0.75; // 斜向次波保持较慢流速。
    (primary_phase.sin() * PRIMARY_WAVE_AMPLITUDE + crossing_phase.sin() * CROSSING_WAVE_AMPLITUDE)
        * bank_fade // 两层大波只影响水面内部，两岸边界始终贴合地形。
} // 结束组合河流波高计算。

/// 计算最近玩家波源在指定水面顶点上的几何高度与可视化信号。
#[cfg(test)] // 运行时逐顶点遍历已删除，CPU采样只用于验证WGSL复现的波包语义。
pub(super) fn calculate_player_ripple_sample(
    rest_position: &[f32; 3],             // 当前水面顶点的静止局部坐标。
    rest_uv: &[f32; 2],                   // 当前顶点的河岸横向 UV，用于固定埋边。
    player_ripples: &[PlayerWaterRipple], // 最近的固定容量玩家波源切片。
) -> PlayerRippleSample {
    // 每个波源只在围绕移动波前的有限环带内贡献高度，波前经过后中心会恢复平水。
    let vertex_local_xz = Vec2::new(rest_position[0], rest_position[2]); // 把三维顶点压到水面局部平面计算径向距离。
    let mut ripple_height = 0.0; // 从零开始累计仍在寿命内的玩家波圈。
    let mut visual_signal = 0.0; // 从零开始累计归一化峰谷信号供顶点颜色显示同心层。
    for ripple in player_ripples {
        // 固定最多八个波源，因此逐顶点遍历成本稳定且不产生临时分配。
        if ripple.age >= PLAYER_RIPPLE_LIFETIME {
            // 已到寿命的波源通常已被 retain 清理，此处保留防御性判断。
            continue; // 跳过不再可见的波圈。
        } // 完成波源寿命检查。
        let distance = vertex_local_xz.distance(ripple.center_local_xz); // 计算顶点到波源中心的平面距离。
        let front_radius = ripple.age * PLAYER_RIPPLE_SPEED; // 让环形波包中心随年龄匀速向外扩大。
        let signed_distance_from_front = distance - front_radius; // 计算顶点相对当前主波峰的有符号径向距离。
        if signed_distance_from_front.abs() > PLAYER_RIPPLE_BAND_HALF_WIDTH {
            // 波动只存在于有限宽度环带，远处静水和已经通过的环内区域不再起伏。
            continue; // 快速跳过当前波源。
        } // 完成移动环形波包范围裁剪。
        let band_ratio = signed_distance_from_front.abs() / PLAYER_RIPPLE_BAND_HALF_WIDTH; // 把波峰两侧距离归一化到零到一。
        let band_fade = 1.0 - smoothstep_unit(band_ratio); // 环带从中心主波峰向两侧平滑并回静水，消除硬边和实心亮区。
        let radial_phase =
            signed_distance_from_front * std::f32::consts::TAU / PLAYER_RIPPLE_LAYER_WAVELENGTH; // 按二点八米波长布置内波谷、主波峰和外波谷三层同心结构。
        let radial_wave = radial_phase.cos() * band_fade; // 有限余弦波包整体向外移动，环带前后都平滑恢复为静水。
        let attack_fade = smoothstep_unit(ripple.age / PLAYER_RIPPLE_ATTACK_SECONDS); // 新波源在极短时间内从静水平滑增长到完整振幅。
        let lifetime_ratio = 1.0 - ripple.age / PLAYER_RIPPLE_LIFETIME; // 计算波源剩余寿命比例。
        let lifetime_fade = lifetime_ratio.clamp(0.0, 1.0); // 使用线性衰减让外圈传播到可见半径后再消失。
        let animated_wave = radial_wave * attack_fade * lifetime_fade; // 合并空间环带、建立阶段和寿命衰减得到本波源当前信号。
        ripple_height += animated_wave * ripple.amplitude; // 按实际事件强度把有限环形波包累加到几何高度。
        visual_signal += animated_wave * (ripple.amplitude / PLAYER_RIPPLE_JUMP_AMPLITUDE);
        // 按跳跃波归一化强度累计用于同一网格的波谷颜色。
    } // 完成全部有效玩家波源累加。
    let bank_fade = calculate_bank_fade(rest_uv); // 复用同一岸边权重同步压低几何和颜色波纹。
    PlayerRippleSample {
        // 同时返回限幅高度和归一化颜色信号，保证两种表现来自完全相同的波形。
        height: ripple_height.clamp(-PLAYER_RIPPLE_MAX_HEIGHT, PLAYER_RIPPLE_MAX_HEIGHT)
            * bank_fade, // 限制叠加高度并让波纹在两侧埋边处归零。
        visual_signal: visual_signal.clamp(-1.0, 1.0) * bank_fade, // 限制颜色对比并让岸边保持 Blender 原生颜色。
    } // 返回当前顶点的完整玩家波纹采样。
} // 结束玩家互动波纹采样计算。

/// 仅返回玩家互动几何波高，供纯逻辑测试使用。
#[cfg(test)] // 运行时直接使用完整采样值，便捷函数仅为纯逻辑测试减少重复字段访问。
pub(super) fn calculate_player_ripple_height(
    rest_position: &[f32; 3],             // 当前水面顶点的静止局部坐标。
    rest_uv: &[f32; 2],                   // 当前顶点的河岸横向 UV。
    player_ripples: &[PlayerWaterRipple], // 最近的固定容量玩家波源切片。
) -> f32 {
    calculate_player_ripple_sample(rest_position, rest_uv, player_ripples).height
    // 返回同一采样中的真实几何高度。
} // 结束玩家互动几何波高便捷计算。

/// 向固定容量波源池加入一圈新波纹，槽满时覆盖最旧波源。
fn emit_player_ripple(
    water_surface: &mut DynamicWaterSurface, // 接收新波源的当前水面运行时数据。
    center_local_xz: Vec2,                   // 新波圈在水面局部平面的中心。
    amplitude: f32,                          // 当前入水、脚步、起跳或落水事件的初始波高。
) -> PlayerWaterRipple {
    // Vec 仅在初始化时预分配八槽，运行时最多移动七个小型值而不继续申请内存。
    if water_surface.player_ripples.len() >= PLAYER_RIPPLE_CAPACITY {
        // 固定池满时优先淘汰年龄最大的首个波源。
        water_surface.player_ripples.remove(0); // 为最新统一事件波腾出一个槽位。
    } // 完成固定容量控制。
    let ripple = PlayerWaterRipple {
        // 新波源从零年龄开始在玩家脚下向外扩散。
        center_local_xz, // 保存发射瞬间的位置，让玩家走开后波圈仍留在原处。
        age: 0.0,        // 零年龄对应半径为零的初始事件扰动。
        amplitude,       // 保存本次统一水面事件采用的强度。
    }; // 完成本次新波源值构造。
    water_surface.player_ripples.push(ripple); // 把新波源写入固定容量几何波池。
    ripple // 返回同一波源供调用方判断本帧是否新增了几何水波。
} // 结束玩家波源发射。

/// 武装一次等待落水或深水着底结算的真实跳跃周期。
fn arm_player_jump(water_surface: &mut DynamicWaterSurface) {
    water_surface.jump_cycle = WaterJumpCycle::Armed {
        left_surface: false,     // 新周期尚未穿出静止水位。
        was_falling_fast: false, // 新周期尚未进入明确下降段。
    }; // 起跳强波由调用点写入统一八槽，本状态只负责等待同一周期的落水强波。
} // 结束水中跳跃周期武装。

/// 更新玩家水面接触与统一事件波源，并只返回本帧最后新追加或合并的波源。
pub(super) fn update_player_ripples(
    water_surface: &mut DynamicWaterSurface, // 当前水面网格各自独立维护的运行时状态。
    player_local_foot_position: Option<Vec3>, // 已转换到水面子网格局部空间的玩家脚底位置。
    player_takeoff_sequence: Option<u64>, // Tnua实际接受跳跃后递增的单调序号，与玩家位置同时存在或同时缺失。
    delta_seconds: f32,                   // 本帧时长用于波源老化、水平移动和深水回落速度估算。
) -> Option<PlayerWaterRipple> {
    if matches!(
        &water_surface.interaction_footprint, // 只读取当前水体冻结的互动模式。
        WaterInteractionFootprint::Disabled   // 瀑布幕布属于持续视觉水流而不是玩家可踩水面。
    ) {
        return None; // 禁用水体不老化、不发射也不淘汰统一八槽中的任何玩家事件。
    }
    // 先推进已有波源年龄，即使玩家已经离水也让尾波继续自然衰减。
    for ripple in &mut water_surface.player_ripples {
        // 固定容量波源池可以直接原地更新年龄。
        ripple.age += delta_seconds; // 累加本帧时间推进波圈半径与寿命。
    } // 完成已有波源老化。
    water_surface
        .player_ripples
        .retain(|ripple| ripple.age < PLAYER_RIPPLE_LIFETIME); // 删除已完全衰减的波源，保持池内容紧凑。
    let (Some(player_local_foot_position), Some(player_takeoff_sequence)) =
        (player_local_foot_position, player_takeoff_sequence)
    else {
        // 玩家尚未生成或正在切换场景时清空接触跟踪，但保留已有尾波。
        water_surface.last_player_takeoff_sequence = None; // 玩家重新出现时只建立最新序号基线，禁止补发旧地图起跳。
        water_surface.previous_player_local_xz = None; // 下一次检测到玩家时重新建立移动基准。
        water_surface.previous_player_local_y = None; // 下一次检测到玩家时重新建立竖直速度基准。
        water_surface.distance_since_ripple = 0.0; // 不跨场景累计无效行走距离。
        water_surface.player_was_in_water = false; // 重新进入时允许生成一圈入水波。
        water_surface.jump_cycle = WaterJumpCycle::Idle; // 切图或玩家重建不能把旧地图的待落水周期带回来。
        return None; // 没有玩家位置时无需继续接触判断，也没有新几何波源。
    }; // 完成玩家局部脚底位置与权威起跳序号的成对解包。
    let player_takeoff_started = water_surface
        .last_player_takeoff_sequence
        .is_some_and(|last_sequence| player_takeoff_sequence > last_sequence); // 只有同一玩家单调递增的新ActionStarted序号才授权一次起跳波。
    water_surface.last_player_takeoff_sequence = Some(player_takeoff_sequence); // 无论当前是否在这片水域都采用最新基线；序号回退代表玩家重建而不是新起跳。

    let current_local_xz = Vec2::new(
        player_local_foot_position.x, // 使用玩家脚底的水面局部东西坐标。
        player_local_foot_position.z, // 使用玩家脚底的水面局部南北坐标。
    ); // 完成玩家脚底局部平面坐标构造。
    let vertical_speed = water_surface
        .previous_player_local_y // 使用上一帧脚底高度计算深水回落速度。
        .map(|previous_local_y| {
            // 除以安全帧时长，避免暂停恢复帧或测试中出现除零。
            (player_local_foot_position.y - previous_local_y) / delta_seconds.max(0.001)
            // 负值表示脚底正在向河床方向回落。
        }) // 完成可选竖直速度计算。
        .unwrap_or(0.0); // 首帧没有历史高度时不误判为深水下降。
    let player_is_falling_fast = vertical_speed <= -PLAYER_RIPPLE_LANDING_MIN_FALL_SPEED; // 竖直速度只负责确认深水跳已经进入明确回落段，起跳不再由渲染帧差猜测。
    let interaction_sample = water_interaction_sample(
        &water_surface.interaction_footprint, // GLB、R8静水和斜河分别走自身实际足迹查询。
        water_surface.rest_water_level,       // 只有水平水体读取该标量局部水位。
        current_local_xz,                     // 查询玩家脚底所在局部XZ。
    );
    let water_depth_at_feet = interaction_sample
        .map(|sample| sample.surface_local_y - player_local_foot_position.y) // 斜河使用当前位置实际三角形水面Y。
        .unwrap_or(f32::NEG_INFINITY); // 水平足迹外位置保持明确非水深度。
    let maximum_interaction_depth = interaction_sample
        .map(|sample| sample.author_depth.min(PLAYER_RIPPLE_MAX_DEPTH)) // 作者深度不能放宽既有三米玩家扰动上限。
        .unwrap_or(0.0); // 足迹外没有合法互动深度。
    let player_is_over_water = interaction_sample.is_some(); // 只有实际足迹采样成功才属于当前水体上方。
    let player_is_in_water = player_is_over_water
        && (PLAYER_RIPPLE_MIN_DEPTH..=maximum_interaction_depth).contains(&water_depth_at_feet); // 可见足迹、真实斜面高度和作者深度共同授权互动波。
    let player_is_above_same_water =
        player_is_over_water && water_depth_at_feet < PLAYER_RIPPLE_MIN_DEPTH; // 同一实际足迹内且脚底位于局部表面上方，供低帧率浅水跳周期连续性使用。

    let movement_distance = water_surface
        .previous_player_local_xz
        .map(|previous_local_xz| current_local_xz.distance(previous_local_xz)); // 只读取一次上一帧水中平面位移，供传送门和脚步节奏共用。
    let player_teleported =
        movement_distance.is_some_and(|distance| distance > PLAYER_RIPPLE_TELEPORT_DISTANCE); // 单帧跨过十二米不能被识别为真实跳跃或连续水中路径。
    if player_teleported {
        // 传送后的高度差和接触边沿都不属于一次物理跳跃。
        water_surface.jump_cycle = WaterJumpCycle::Idle; // 丢弃旧地点尚未结算的落水周期。
    } // 完成传送跳跃周期隔离。

    let shallow_landing_pending_this_frame = player_is_in_water
        && !water_surface.player_was_in_water
        && matches!(
            water_surface.jump_cycle,
            WaterJumpCycle::Armed {
                left_surface: true,
                ..
            }
        ); // 已观察到离水后重新入水，本帧后续分支会结算浅水落水强波。
    let deep_landing_pending_this_frame = player_is_in_water
        && water_surface.player_was_in_water
        && matches!(
            water_surface.jump_cycle,
            WaterJumpCycle::Armed {
                left_surface: false,
                was_falling_fast: true,
            }
        )
        && !player_is_falling_fast; // 已观察到深水下降后速度停止，本帧后续分支会结算着底强波。
    let landing_pending_this_frame =
        shallow_landing_pending_this_frame || deep_landing_pending_this_frame; // 同帧落水与新起跳必须合并成一个强波。
    let has_water_jump_context = water_surface.player_was_in_water
        || matches!(water_surface.jump_cycle, WaterJumpCycle::Armed { .. }); // 当前接触或仍武装的水中周期都能证明新ActionStarted属于这张水面。

    let mut emitted_ripple = None; // 默认本帧只推进已有波源，不向水面追加新扰动。
    if has_water_jump_context
        && !player_teleported
        && player_is_over_water
        && (player_is_in_water || player_is_above_same_water)
        && player_takeoff_started
        && !landing_pending_this_frame
    {
        // Tnua真实起跳立即发大波；若旧Armed周期的完整起落被低帧率采样跳过，新ActionStarted也会权威接管并重新武装。
        let takeoff_center_local_xz = if player_is_in_water {
            current_local_xz // 脚底仍有合法水深时使用真实起跳点，移动跳跃不会让大波滞后一帧。
        } else {
            water_surface
                .previous_player_local_xz
                .unwrap_or(current_local_xz) // 单帧已经穿面时退回上一同水域采样点，避免大波中心沿空中路径多前进一帧。
        }; // 在低帧率穿面、漏采落水与正常水中起跳之间选择最接近真实触水点的中心。
        emitted_ripple = Some(emit_player_ripple(
            water_surface,           // 起跳波直接进入统一八槽，旧起跳与落水外圈继续按原年龄传播。
            takeoff_center_local_xz, // 波心固定在实际离水点而不跟随空中角色。
            PLAYER_RIPPLE_JUMP_AMPLITUDE, // 起跳与落水使用同一九厘米强度和同一二点四秒波形。
        )); // 完成本周期起跳大波追加。
        arm_player_jump(water_surface); // 记录待落水周期，禁止同一ActionStarted重复发波。
    } // 完成起跳边沿识别。
    if let WaterJumpCycle::Armed {
        was_falling_fast, ..
    } = &mut water_surface.jump_cycle
    {
        // 一旦出现明确下降速度就保持标记，直到深水着底或浅水重新入水结算。
        *was_falling_fast |= player_is_falling_fast; // 后续落地帧速度归零时仍知道此前确实经历过下落。
    } // 完成跳跃下降阶段推进。

    if !player_is_in_water {
        // 浅水跳离水期间保留周期和竖直历史，真正落回可见水面时才发唯一冲击波。
        let remains_above_same_water = player_is_above_same_water; // 只有位于同一水平足迹且脚底在静止水位上方，才属于等待落水的空中阶段。
        if remains_above_same_water {
            if let WaterJumpCycle::Armed { left_surface, .. } = &mut water_surface.jump_cycle {
                *left_surface = true; // 标记本周期确实离开过水面，下一次入水必须按落水强波结算。
            } // 完成浅水离面标记。
            water_surface.previous_player_local_xz = Some(current_local_xz); // 保留同一水面上空的水平历史，使后续空中大位移仍能按传送取消周期。
            water_surface.previous_player_local_y = Some(player_local_foot_position.y); // 保留同一水面上空的真实升降历史。
        } else {
            // 横向离开水域、沉入超出支持深度或切到无效位置都不能在以后补发幽灵落水波。
            water_surface.previous_player_local_xz = None; // 下一次合法接触重新建立水平移动基准。
            water_surface.previous_player_local_y = None; // 下一次合法接触重新建立高度基准。
            water_surface.jump_cycle = WaterJumpCycle::Idle; // 取消未落回当前水面的跳跃周期。
        } // 完成同一水面上空与真正离开水域的区分。
        water_surface.distance_since_ripple = 0.0; // 防止再次入水立刻继承旧距离发波。
        water_surface.player_was_in_water = false; // 记录当前不接触水面。
        return emitted_ripple; // 已在穿面帧触发的起跳大波必须返回，旧外圈同时继续自然传播。
    } // 完成水面接触门控。

    let mut landed_this_frame = false; // 落水后同一固定步又开始新跳时用于合并同中心强波并重新武装。
    if !water_surface.player_was_in_water {
        // 从同一水面上空落回时结算强波；普通从岸边进入仍使用标准入水波。
        let landing_from_jump = matches!(
            water_surface.jump_cycle,
            WaterJumpCycle::Armed {
                left_surface: true,
                ..
            }
        ); // 只有此前由真实水中起跳武装并穿出静止水位，才属于本周期落水。
        emitted_ripple = Some(emit_player_ripple(
            water_surface,    // 新冲击只追加一个波源，既有同心外圈的年龄和位置完全不变。
            current_local_xz, // 落水或普通入水都以当前脚下真实水平位置为中心。
            if landing_from_jump {
                PLAYER_RIPPLE_JUMP_AMPLITUDE // 完整跳跃落水使用强波。
            } else {
                PLAYER_RIPPLE_ENTRY_AMPLITUDE // 普通走入水面仍使用中等入水强度。
            },
        )); // 完成本帧唯一入水或落水波源追加。
        landed_this_frame = landing_from_jump; // 记录本帧是否刚结算一个完整跳跃。
        water_surface.jump_cycle = WaterJumpCycle::Idle; // 无论普通入水还是落水，接触边沿都不遗留旧周期。
        water_surface.distance_since_ripple = 0.0; // 入水圈已经代表当前位置，重新开始累计步距。
    } else if matches!(
        water_surface.jump_cycle,
        WaterJumpCycle::Armed {
            left_surface: false,
            was_falling_fast: true,
        }
    ) && !player_is_falling_fast
    {
        // 深水中脚底未穿出静止水位：明确下降段结束即代表回到底部，只在此处结算一次冲击。
        emitted_ripple = Some(emit_player_ripple(
            water_surface,                // 只追加当前着底波，旧波继续向外传播。
            current_local_xz,             // 原地连续跳仍以每次实际着底位置为圆心。
            PLAYER_RIPPLE_JUMP_AMPLITUDE, // 深水着底与浅水落水使用同一跳跃冲击强度。
        )); // 完成深水跳跃周期的唯一波源追加。
        landed_this_frame = true; // 允许持续按跳时在同一帧为下一周期重新武装。
        water_surface.jump_cycle = WaterJumpCycle::Idle; // 当前完整跳跃已经结算完毕。
        water_surface.distance_since_ripple = 0.0; // 着底波已经代表当前位置，重新累计后续行走步距。
    } else if matches!(water_surface.jump_cycle, WaterJumpCycle::Idle)
        && let Some(previous_local_xz) = water_surface.previous_player_local_xz
    {
        // 已持续处于水中时按本帧真实位移累计脚步波间距。
        let movement_distance =
            movement_distance.unwrap_or_else(|| current_local_xz.distance(previous_local_xz)); // 正常路径复用前面已计算的同一帧水平位移。
        if movement_distance > PLAYER_RIPPLE_TELEPORT_DISTANCE {
            // 大位移视为传送或读档，不沿错误路径补发大量波圈。
            water_surface.distance_since_ripple = 0.0; // 从传送后的新位置重新累计水中步距。
            emitted_ripple = Some(emit_player_ripple(
                water_surface,                 // 仍在水中时以一圈轻波提示新位置。
                current_local_xz,              // 波源中心直接采用传送后的当前位置。
                PLAYER_RIPPLE_ENTRY_AMPLITUDE, // 复用入水强度避免传送产生夸张冲击波。
            )); // 完成传送后位置几何波发射并保存本帧新波源。
        } else {
            // 正常移动只累计真实路径距离，站定时不会持续生成波纹。
            water_surface.distance_since_ripple += movement_distance; // 把本帧位移加入上次发波后的累计距离。
            if water_surface.distance_since_ripple >= PLAYER_RIPPLE_SPACING {
                // 累积跨过固定步距时在当前脚下留下新波源。
                let movement_speed = movement_distance / delta_seconds.max(0.001); // 用本帧位移估算水中水平速度并避免除零。
                let speed_weight = ((movement_speed - 0.5) / 4.5).clamp(0.0, 1.0); // 把慢走到疾跑速度映射为零到一。
                let amplitude = PLAYER_RIPPLE_MIN_STEP_AMPLITUDE
                    + (PLAYER_RIPPLE_MAX_STEP_AMPLITUDE - PLAYER_RIPPLE_MIN_STEP_AMPLITUDE)
                        * speed_weight; // 按移动速度插值脚步波高度并保持安全上限。
                emitted_ripple = Some(emit_player_ripple(
                    water_surface,    // 把新脚步波写入固定容量池。
                    current_local_xz, // 波源停留在本次跨过步距时的脚下位置。
                    amplitude,        // 使用当前真实移动速度对应的波高。
                )); // 完成本次水中脚步几何波发射并保存本帧新波源。
                water_surface.distance_since_ripple %= PLAYER_RIPPLE_SPACING; // 保留不足一个步距的余量，让连续行走节奏稳定。
            } // 完成固定步距发波判断。
        } // 完成正常移动与传送分支。
    } // 完成持续入水路径更新。

    if landed_this_frame && player_takeoff_started {
        // 落水与下一次Tnua ActionStarted落在同一渲染帧时复用刚追加的同中心大波，避免双槽同相限幅。
        arm_player_jump(water_surface); // 只为下一周期重新武装，当前落水强波同时代表紧接着的起跳冲击。
    } // 完成着底同帧连续跳跃衔接。

    water_surface.previous_player_local_xz = Some(current_local_xz); // 保存当前局部位置供下一帧计算真实位移。
    water_surface.previous_player_local_y = Some(player_local_foot_position.y); // 保存当前脚底高度供下一帧计算真实竖直速度。
    water_surface.player_was_in_water = true; // 记录当前已接触水面，下一帧进入持续行走分支。
    emitted_ripple // 返回本帧唯一新几何波源，便于测试触发节奏与强度。
} // 结束玩家水面接触与波源更新。
