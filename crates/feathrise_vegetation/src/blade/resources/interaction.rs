use bevy::prelude::{Resource, Vec2, Vec4};
use bevy::render::extract_resource::ExtractResource;
use bevy::render::render_resource::ShaderType;

/// 顶点着色器每帧最多读取的局部草地交互源数量。
pub const GPU_GRASS_INTERACTION_SOURCE_CAPACITY: usize = 8; // 一个玩家当前压草源加七个短寿命足迹源，固定上限避免逐草或无限队列。

/// 正式草地默认草尖风摆幅度。
pub const GPU_GRASS_DEFAULT_WIND_TIP_STRENGTH: f32 = 0.16; // 十六厘米能在正常镜头距离明确看到风浪，同时仍低于强弯曲上限。

/// 测试面板和公开风场接口允许的最大草尖风摆幅度。
pub const GPU_GRASS_MAX_WIND_TIP_STRENGTH: f32 = 0.35; // 三十五厘米保持在现有批次AABB和总弯曲钳制的安全范围内。

/// 单个世界空间草地交互源。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GpuBladeInteractionSource {
    pub position_xz: Vec2, // 世界X/Z中心决定哪些单根草进入局部弯曲范围。
    pub radius: f32,       // 米制影响半径，零或负值由上传接口过滤。
    pub strength: f32,     // 零至一弯曲强度，足迹可随寿命逐帧衰减。
}

impl GpuBladeInteractionSource {
    /// 构造一个会由全局uniform上传接口执行安全钳制的交互源。
    pub fn new(position_xz: Vec2, radius: f32, strength: f32) -> Self {
        Self {
            position_xz, // 保留调用方提供的世界X/Z位置，单根草根在GPU逐一计算距离。
            radius,      // 半径稍后统一钳制，便于场景层使用自然米制参数。
            strength,    // 强度稍后统一钳制，防止异常玩法值放大草叶位移。
        } // 完成轻量CPU交互源构造。
    }
}

/// 全部草窗共享的一份GPU风场与玩家足迹uniform。
///
/// 【功能设计】CPU每帧只上传176字节固定数据；世界稳定风摆和每根草对足迹的距离响应都在顶点着色器完成，
/// 不创建逐草ECS实体、不保存逐草弯曲历史，也不随一千万候选规模增长CPU或上传成本。
#[derive(Resource, ExtractResource, Clone, Copy, Debug, PartialEq, ShaderType)]
#[extract_app(bevy::render::RenderApp)]
pub struct GpuBladeInteraction {
    pub(crate) wind: Vec4, // xy为归一化世界风向，z为草尖最大风摆米数，w为动画速度。
    pub(crate) wind_phase: Vec4, // x为天气变速/一小时回绕的相位补偿，其余槽保留为零。
    pub(crate) interaction_bounds: Vec4, // xy为全部足迹包围圆中心，z为半径，w为有效源数量。
    pub(crate) sources: [Vec4; GPU_GRASS_INTERACTION_SOURCE_CAPACITY], // 每项xy为中心、z为半径、w为当前衰减强度。
}

impl Default for GpuBladeInteraction {
    fn default() -> Self {
        Self {
            wind: Vec4::new(0.80, 0.60, GPU_GRASS_DEFAULT_WIND_TIP_STRENGTH, 1.10), // 默认风提高到十六厘米草尖摆幅，方向固定但波形由世界坐标变化。
            wind_phase: Vec4::ZERO, // 独立Blade示例继续使用Bevy原始时间，不受天气桥影响。
            interaction_bounds: Vec4::ZERO, // 没有玩家或不在草场景时让所有草株一次包围圆判断后快速跳过。
            sources: [Vec4::ZERO; GPU_GRASS_INTERACTION_SOURCE_CAPACITY], // 固定槽零初始化表示没有局部踩踏源。
        } // 完成无需场景配置即可工作的低成本默认风场。
    }
}

impl GpuBladeInteraction {
    /// 设置世界风向、草尖最大位移和时间速度，不改变当前玩家足迹。
    pub fn set_wind(&mut self, direction_xz: Vec2, tip_strength: f32, speed: f32) {
        let direction_is_finite = direction_xz.x.is_finite() && direction_xz.y.is_finite(); // 两个方向分量必须同时为有限值才允许进入归一化。
        let safe_direction = if direction_is_finite {
            if (direction_xz.length_squared() - 1.0).abs() < 0.000_001 {
                direction_xz // 天气输出已是单位向量，避免二次归一化舍入造成稳定帧反复写GPU。
            } else {
                direction_xz.try_normalize().unwrap_or(Vec2::X) // 非单位向量安全归一化，合法零向量回退世界X。
            } // 完成幂等单位风向判断。
        } else {
            Vec2::X // NaN或无穷方向直接回退，绝不把非法值上传给shader。
        }; // 完成世界风向有限值与长度校验。
        let safe_tip_strength = if tip_strength.is_finite() {
            tip_strength // 合法米制摆幅保留给统一范围钳制。
        } else {
            0.0 // 非有限摆幅按无风处理。
        }; // 完成风力有限值过滤。
        let safe_speed = if speed.is_finite() {
            speed // 合法动画速度保留给统一范围钳制。
        } else {
            0.0 // 非有限速度停止时间推进，避免整片顶点位置变成NaN。
        }; // 完成风速有限值过滤。
        self.wind = Vec4::new(
            safe_direction.x, // 上传归一化且有限的世界风向X。
            safe_direction.y, // 上传归一化且有限的世界风向Z。
            safe_tip_strength.clamp(0.0, GPU_GRASS_MAX_WIND_TIP_STRENGTH), // 半米草最大风摆限制在35厘米，防止穿地或大幅越出AABB。
            safe_speed.clamp(0.0, 8.0), // 动画速度保持有限，零可用于静态性能对照。
        ); // 完成一份全局风场参数更新。
    }

    /// 只调整草尖最大风摆幅度，保留当前世界风向、动画速度和全部玩家足迹。
    pub fn set_wind_tip_strength(&mut self, tip_strength: f32) {
        let tip_strength = if tip_strength.is_finite() {
            tip_strength // 合法配置保留原值。
        } else {
            0.0 // 非有限值回退为无风，绝不把NaN或无穷大上传给shader。
        }; // 完成外部风力参数有限值过滤。
        self.wind.z = tip_strength.clamp(0.0, GPU_GRASS_MAX_WIND_TIP_STRENGTH); // F1滑块与外部配置共享同一安全钳制，不允许NaN或越界值进入shader。
    }

    /// 补偿风速变化和渲染时间回绕，不改变水草水流速度或玩家足迹。
    pub fn set_wind_phase_offset(&mut self, offset: f32) {
        self.wind_phase.x = if offset.is_finite() { offset } else { 0.0 }; // 相位损坏时安全回到零，不让GPU顶点变成NaN。
    } // 完成唯一风相位偏移入口。

    /// 清除全部局部踩踏源，但保留持续运行的世界风场。
    pub fn clear_sources(&mut self) {
        self.interaction_bounds = Vec4::ZERO; // 零包围半径让绝大多数顶点在进入固定数组循环前返回。
        self.sources.fill(Vec4::ZERO); // 清除旧场景或旧玩家留下的足迹槽，避免重进场景后错误压草。
    }

    /// 用固定容量源替换本帧局部交互，超出八项的低优先级尾部源直接忽略。
    pub fn set_sources(&mut self, sources: &[GpuBladeInteractionSource]) {
        self.clear_sources(); // 每帧完整覆盖固定小数组，不让过期足迹残留在未写槽位。
        let mut bounds_min = Vec2::splat(f32::INFINITY); // 从首个合法源开始收集包含半径的世界最小角。
        let mut bounds_max = Vec2::splat(f32::NEG_INFINITY); // 同步收集全部合法源的世界最大角。
        let mut active_count = 0usize; // 只统计半径和强度均有效且真正写入GPU的源。
        for source in sources.iter().take(GPU_GRASS_INTERACTION_SOURCE_CAPACITY) {
            if !source.position_xz.x.is_finite() || !source.position_xz.y.is_finite() {
                continue; // 非有限世界中心会污染包围圆与全部命中草顶点，必须整项拒绝。
            } // 完成世界位置有限值校验。
            let safe_radius = if source.radius.is_finite() {
                source.radius.clamp(0.0, 8.0) // 单个角色交互半径限制在八码内，防止错误值拖累全景顶点。
            } else {
                0.0 // 非有限半径按无效源处理。
            }; // 完成半径有限值和范围钳制。
            let safe_strength = if source.strength.is_finite() {
                source.strength.clamp(0.0, 1.0) // 多源叠加强度由shader统一钳制，单源先保持标准范围。
            } else {
                0.0 // 非有限强度按无效源处理。
            }; // 完成强度有限值和范围钳制。
            if safe_radius <= 0.001 || safe_strength <= 0.001 {
                continue; // 已过期或无范围源不占有效槽，也不扩大局部循环包围圆。
            }
            self.sources[active_count] = Vec4::new(
                source.position_xz.x, // 当前源世界X中心。
                source.position_xz.y, // 当前源世界Z中心。
                safe_radius,          // shader使用平方距离与该米制半径比较。
                safe_strength,        // 当前足迹寿命已经折算成实际弯曲强度。
            ); // 完成一个连续有效槽写入。
            bounds_min = bounds_min.min(source.position_xz - Vec2::splat(safe_radius)); // 包含圆形源的最小外接矩形角。
            bounds_max = bounds_max.max(source.position_xz + Vec2::splat(safe_radius)); // 包含圆形源的最大外接矩形角。
            active_count += 1; // 仅在成功写槽后推进下一个固定位置。
        }
        if active_count == 0 {
            return; // 没有合法源时保持清零后的快速跳过状态。
        }
        let bounds_center = (bounds_min + bounds_max) * 0.5; // 用全部足迹外接矩形中心构造一次粗粒度GPU包围圆。
        let bounds_radius = (bounds_max - bounds_center).length(); // 半对角线覆盖所有源圆，宁可保守进入局部循环也不漏压草。
        self.interaction_bounds = Vec4::new(
            bounds_center.x,          // 包围圆世界X中心。
            bounds_center.y,          // 包围圆世界Z中心。
            bounds_radius.max(0.001), // 保持非零包围半径供shader平方比较。
            active_count as f32,      // WGSL转换为整数后只循环有效连续槽。
        ); // 完成本帧全部局部交互源的粗剔除信息。
    }

    /// 返回当前真正上传给GPU的有效玩家与足迹源数量。
    pub fn active_source_count(&self) -> usize {
        self.interaction_bounds.w.round() as usize // 数量由CPU以精确小整数写入f32，可无损还原零至八。
    }

    /// 返回默认或场景配置后的草尖最大风摆米数供F3显示。
    pub fn wind_tip_strength(&self) -> f32 {
        self.wind.z // 不暴露内部Vec4写权限，诊断只读取当前值。
    }

    /// 返回当前归一化世界X/Z风向供树木等植被共用同一风场。
    pub fn wind_direction(&self) -> Vec2 {
        Vec2::new(self.wind.x, self.wind.y) // 风向已由默认值或set_wind统一完成有限值校验和归一化。
    }

    /// 返回当前风动画速度供F3区分关闭与正常运行状态。
    pub fn wind_speed(&self) -> f32 {
        self.wind.w // 零表示保留静态风向但停止时间推进。
    }

    /// 返回草叶与Mesh风材质共享的时间相位补偿。
    pub fn wind_phase_offset(&self) -> f32 {
        self.wind_phase.x // 时间速度仍由wind.w独立表示，单位是无量纲动画倍率。
    } // 完成只读相位访问。
}
