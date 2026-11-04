// 天气输出纯逻辑：定义五类天气目标参数并平滑解析自动过渡与运行时调试覆盖。

use bevy::color::LinearRgba; // 线性颜色通道可以与其他连续天气参数使用同一插值函数。
use bevy::math::Vec2; // 连续天气输出包含供雨、云和植被共享的水平风向。

use crate::weather::constants::{DEFAULT_WIND_DIRECTION, MIN_PLAYABLE_AMBIENT}; // 所有天气共享中性默认风向和午夜可玩环境光下限。
use crate::weather::resources::{WeatherDebugOverride, WeatherOutput, WeatherState}; // 输出解析只读取自然状态与唯一运行时调试覆盖。
use crate::weather::types::{WeatherGameplayModifiers, WeatherKind}; // 五类天气目标值共用公开普通类型。

impl Default for WeatherOutput {
    fn default() -> Self {
        weather_profile(WeatherKind::Clear) // 插件安装首帧使用完整晴天输出，避免未解析时场景异常压暗。
    } // 结束连续天气输出默认构造。
} // 结束天气输出Default实现。

pub(super) fn natural_precipitation_at(state: &WeatherState, progress: f32) -> f32 {
    // 按实际段中点采样，避免大步进忽略整段降雨。
    interpolate_automatic_transition(
        weather_profile(state.current()),
        weather_profile(state.target()),
        progress,
    )
    .precipitation // 复用正式先云后雨曲线。
} // 结束自然雨量采样。

/// 将自动状态与可选运行时强制天气合成为最终连续输出。
pub(crate) fn resolved_weather_output(
    state: &WeatherState,         // 自动天气当前节点、目标和过渡进度。
    debug: &WeatherDebugOverride, // F12 运行时强制天气与预览比例。
) -> WeatherOutput {
    let mut output = resolved_atmosphere_output(state, debug); // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
    super::surface::apply_surface_output(state, debug, &mut output); // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
    output // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
} // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。

fn resolved_atmosphere_output(state: &WeatherState, debug: &WeatherDebugOverride) -> WeatherOutput {
    // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
    let automatic = interpolate_automatic_transition(
        weather_profile(state.current()), // 读取自动过渡起点目标参数。
        weather_profile(state.target()),  // 读取自动过渡终点目标参数。
        state.transition_progress(),      // 分阶段过渡内部会分别平滑云层与降雨进度。
    ); // 完成符合“先聚云后落雨、先停雨再散云”的正式区域自动天气解析。
    let Some(forced) = debug.forced() else {
        if debug.preview_override_active() {
            return interpolate_automatic_transition(
                weather_profile(state.current()), // 自动预览左端始终是当前离散天气。
                weather_profile(state.target()),  // 自动预览右端使用状态机已经抽取的目标天气。
                debug.preview(), // F12 暂停期间按手动比例检查完整过渡，不消费自然状态机时间。
            ); // 返回自动模式下临时手动接管的连续预览输出。
        } // 结束自动天气手动预览判断。
        return automatic; // 未拖动预览时返回状态机真实过渡输出。
    }; // 结束运行时强制天气存在性判断。
    interpolate_automatic_transition(
        automatic,               // 预览零端保持当前自动天气完整效果。
        weather_profile(forced), // 预览一端显示强制天气完整目标参数。
        debug.preview(),         // F12 调试覆盖同样遵循先聚云后落雨、先停雨再散云的分阶段曲线。
    ) // 返回可直接验收天气形成顺序的调试覆盖输出。
} // 结束天气输出解析。

/// 返回单一天气的完整目标参数。
pub(crate) const fn weather_profile(kind: WeatherKind) -> WeatherOutput {
    match kind {
        WeatherKind::Clear => WeatherOutput {
            surface_wetness: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            puddle_amount: 0.0,   // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            surface_effect_time: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            cloud_cover: 0.05,    // 晴天只保留少量高空云感，天空基本通透。
            precipitation: 0.0,   // 晴天没有降水粒子与雨声。
            visibility: 1.0,      // 晴天使用完整地形视距。
            wind: 0.15,           // 晴天保持轻微空气流动与植被摆动。
            wind_direction: DEFAULT_WIND_DIRECTION, // 晴天与其他天气共享不绑定地域的默认+X风向。
            direct_light_factor: 1.0, // 晴天保留世界时间完整太阳或月光照度。
            ambient_factor: 1.0,  // 晴天保留完整昼夜环境光。
            min_ambient: MIN_PLAYABLE_AMBIENT, // 午夜环境光仍使用统一可玩下限。
            sky_factor: 1.0,      // 晴天天空保持作者基准亮度。
            tint: LinearRgba::new(1.0, 1.0, 1.0, 1.0), // 晴天不额外偏移昼夜颜色。
            gameplay: WeatherGameplayModifiers {
                ai_visibility_factor: 1.0, // 晴天AI使用完整视认距离。
                ranged_aim_factor: 1.0,    // 晴天远程武器使用完整有效瞄准距离。
                firearm_speed_factor: 1.0, // 晴天火铳动作速度不受天气影响。
            }, // 结束晴天玩法修正。
        }, // 结束晴天目标参数。
        WeatherKind::Overcast => WeatherOutput {
            surface_wetness: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            puddle_amount: 0.0,   // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            surface_effect_time: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            cloud_cover: 0.75,    // 阴天大部分天空被云幕覆盖。
            precipitation: 0.0,   // 阴天本身尚未形成降水。
            visibility: 0.90,     // 阴天只轻微降低远景清晰度。
            wind: 0.30,           // 阴天使用中低风力推动云层和植被。
            wind_direction: DEFAULT_WIND_DIRECTION, // 区域风向保持稳定，过渡只连续改变风力。
            direct_light_factor: 0.55, // 阴云显著削弱直射天体光。
            ambient_factor: 0.90, // 散射环境光只小幅降低。
            min_ambient: MIN_PLAYABLE_AMBIENT, // 阴天夜间仍保持统一可玩下限。
            sky_factor: 0.65,     // 阴天天空亮度降低但不进入降雨黑云程度。
            tint: LinearRgba::new(0.82, 0.86, 0.92, 1.0), // 阴天使用低饱和冷灰蓝色调。
            gameplay: WeatherGameplayModifiers {
                ai_visibility_factor: 0.95, // 阴天AI视认距离轻微降低。
                ranged_aim_factor: 1.0,     // 阴天不降低远程有效瞄准距离。
                firearm_speed_factor: 1.0,  // 无降雨时火铳动作速度保持完整。
            }, // 结束阴天玩法修正。
        }, // 结束阴天目标参数。
        WeatherKind::LightRain => WeatherOutput {
            surface_wetness: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            puddle_amount: 0.0,   // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            surface_effect_time: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            cloud_cover: 0.90,    // 小雨需要接近完整云幕避免晴空下雨。
            precipitation: 0.40,  // 小雨使用四成归一化雨量。
            visibility: 0.75,     // 小雨轻雾与雨丝适度压缩远景。
            wind: 0.45,           // 小雨使用中等风力统一推动雨向和植被。
            wind_direction: DEFAULT_WIND_DIRECTION, // 雨丝、云幕和植被统一读取同一方向。
            direct_light_factor: 0.35, // 小雨云层进一步压低直射天体光。
            ambient_factor: 0.82, // 小雨保留足够散射环境亮度。
            min_ambient: MIN_PLAYABLE_AMBIENT, // 小雨午夜仍保持统一可玩下限。
            sky_factor: 0.45,     // 小雨天空明显压暗。
            tint: LinearRgba::new(0.65, 0.72, 0.82, 1.0), // 小雨使用偏冷湿润蓝灰色调。
            gameplay: WeatherGameplayModifiers {
                ai_visibility_factor: 0.85, // 小雨AI视认距离降低到八成半。
                ranged_aim_factor: 0.90,    // 小雨远程有效瞄准距离降低到九成。
                firearm_speed_factor: 0.90, // 小雨火铳举铳与装填速度降低到九成。
            }, // 结束小雨玩法修正。
        }, // 结束小雨目标参数。
        WeatherKind::HeavyRain => WeatherOutput {
            surface_wetness: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            puddle_amount: 0.0,   // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            surface_effect_time: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            cloud_cover: 1.0,     // 大雨使用完整厚云覆盖。
            precipitation: 1.0,   // 大雨使用完整归一化雨量。
            visibility: 0.45,     // 大雨明显降低远景能见度。
            wind: 0.90,           // 大雨使用接近风暴级风力但保留首版上限余量。
            wind_direction: DEFAULT_WIND_DIRECTION, // 暴雨增强风速但不让各表现层自行改变方向。
            direct_light_factor: 0.18, // 厚重雨云只保留少量直射天体光。
            ambient_factor: 0.70, // 大雨环境光降低但不能与午夜叠加成全黑。
            min_ambient: MIN_PLAYABLE_AMBIENT, // 大雨午夜强制保留五亮度环境光。
            sky_factor: 0.25,     // 大雨天空压暗到四分之一作者基准。
            tint: LinearRgba::new(0.48, 0.57, 0.70, 1.0), // 大雨使用深冷蓝灰色调。
            gameplay: WeatherGameplayModifiers {
                ai_visibility_factor: 0.65, // 大雨AI视认距离降低到六成半。
                ranged_aim_factor: 0.75,    // 大雨远程有效瞄准距离降低到四分之三。
                firearm_speed_factor: 0.80, // 大雨火铳举铳与装填速度降低到八成。
            }, // 结束大雨玩法修正。
        }, // 结束大雨目标参数。
        WeatherKind::Mist => WeatherOutput {
            surface_wetness: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            puddle_amount: 0.0,   // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            surface_effect_time: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            cloud_cover: 0.45,    // 薄雾允许看到部分天空但保持湿润朦胧感。
            precipitation: 0.0,   // 薄雾不生成雨粒子或雨声。
            visibility: 0.35,     // 薄雾是首版能见度最低的天气。
            wind: 0.05,           // 薄雾使用极弱风避免视觉上迅速吹散。
            wind_direction: DEFAULT_WIND_DIRECTION, // 薄雾仍保留同一区域风向，只显著降低风力。
            direct_light_factor: 0.45, // 薄雾散射并削弱直射太阳光。
            ambient_factor: 0.78, // 薄雾环境光降低并保持柔和散射感。
            min_ambient: MIN_PLAYABLE_AMBIENT, // 清晨前薄雾仍保持统一可玩亮度下限。
            sky_factor: 0.50,     // 薄雾天空亮度降低到一半。
            tint: LinearRgba::new(0.72, 0.78, 0.82, 1.0), // 薄雾使用浅冷灰蓝色调。
            gameplay: WeatherGameplayModifiers {
                ai_visibility_factor: 0.55, // 薄雾AI视认距离降低到五成半。
                ranged_aim_factor: 0.70,    // 薄雾远程有效瞄准距离降低到七成。
                firearm_speed_factor: 1.0,  // 无降雨时火铳动作速度保持完整。
            }, // 结束薄雾玩法修正。
        }, // 结束薄雾目标参数。
    } // 返回指定天气完整目标参数。
} // 结束天气目标参数查询。

/// 在两组天气输出之间进行逐字段线性插值。
fn interpolate_output(from: WeatherOutput, to: WeatherOutput, factor: f32) -> WeatherOutput {
    let t = if factor.is_finite() {
        factor.clamp(0.0, 1.0) // 有限插值因子统一钳制到零至一。
    } else {
        0.0 // 非有限因子安全保留起点输出。
    }; // 完成插值因子规范化。
    if t <= 0.0 {
        return from; // 左端直接返回起点，避免浮点运算破坏调试预览精确端点。
    } // 结束插值左端快速返回。
    if t >= 1.0 {
        return to; // 右端直接返回终点，避免颜色通道产生末位舍入误差。
    } // 结束插值右端快速返回。
    WeatherOutput {
        surface_wetness: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
        puddle_amount: 0.0,   // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
        surface_effect_time: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
        cloud_cover: lerp(from.cloud_cover, to.cloud_cover, t), // 连续插值云层覆盖程度。
        precipitation: lerp(from.precipitation, to.precipitation, t), // 连续插值降水强度。
        visibility: lerp(from.visibility, to.visibility, t), // 连续插值归一化能见度。
        wind: lerp(from.wind, to.wind, t), // 连续插值归一化风力。
        wind_direction: safe_wind_direction(from.wind_direction.lerp(to.wind_direction, t)), // 风向未来扩展到不同区域时也使用最短连续向量插值并重新归一化。
        direct_light_factor: lerp(from.direct_light_factor, to.direct_light_factor, t), // 连续插值直射光倍率。
        ambient_factor: lerp(from.ambient_factor, to.ambient_factor, t), // 连续插值环境光倍率。
        min_ambient: lerp(from.min_ambient, to.min_ambient, t),          // 连续插值可玩环境光下限。
        sky_factor: lerp(from.sky_factor, to.sky_factor, t),             // 连续插值天空亮度倍率。
        tint: LinearRgba::new(
            lerp(from.tint.red, to.tint.red, t), // 连续插值天气色调红通道。
            lerp(from.tint.green, to.tint.green, t), // 连续插值天气色调绿通道。
            lerp(from.tint.blue, to.tint.blue, t), // 连续插值天气色调蓝通道。
            lerp(from.tint.alpha, to.tint.alpha, t), // 连续插值天气色调透明通道。
        ), // 完成天气色调线性插值。
        gameplay: WeatherGameplayModifiers {
            ai_visibility_factor: lerp(
                from.gameplay.ai_visibility_factor, // 读取起点AI视认距离倍率。
                to.gameplay.ai_visibility_factor,   // 读取终点AI视认距离倍率。
                t,                                  // 使用与视觉效果相同的连续进度。
            ), // 完成AI视认距离倍率插值。
            ranged_aim_factor: lerp(
                from.gameplay.ranged_aim_factor, // 读取起点远程有效瞄准倍率。
                to.gameplay.ranged_aim_factor,   // 读取终点远程有效瞄准倍率。
                t,                               // 使用与视觉效果相同的连续进度。
            ), // 完成远程有效瞄准倍率插值。
            firearm_speed_factor: lerp(
                from.gameplay.firearm_speed_factor, // 读取起点火铳动作速度倍率。
                to.gameplay.firearm_speed_factor,   // 读取终点火铳动作速度倍率。
                t,                                  // 使用与视觉效果相同的连续进度。
            ), // 完成火铳动作速度倍率插值。
        }, // 结束连续玩法修正插值。
    } // 返回所有字段使用同一进度的连续天气输出。
} // 结束天气输出插值。

/// 按降雨开始与停止方向分阶段解析自然天气过渡。
fn interpolate_automatic_transition(
    from: WeatherOutput, // 当前天气的完整连续参数。
    to: WeatherOutput,   // 相邻目标天气的完整连续参数。
    progress: f32,       // 状态机保存的原始零至一过渡进度。
) -> WeatherOutput {
    let progress = unit_progress(progress); // 损坏进度统一回到合法定义域。
    if progress <= 0.0 {
        return from; // 零端必须精确保持当前天气。
    } // 结束自动过渡零端守卫。
    if progress >= 1.0 {
        return to; // 一端必须精确命中目标天气。
    } // 结束自动过渡一端守卫。

    let starts_raining =
        from.precipitation <= f32::EPSILON && to.precipitation > from.precipitation; // 无雨进入降雨时，云幕必须先于雨滴建立。
    let stops_raining = from.precipitation > f32::EPSILON && to.precipitation <= f32::EPSILON; // 降雨转无雨时，雨滴必须先于云幕消散。
    let (cloud_progress, rain_progress) = if starts_raining {
        (
            smoothstep(remap_progress(progress, 0.0, 0.55)), // 前五成半过渡先完成主要聚云与压光。
            smoothstep(remap_progress(progress, 0.30, 1.0)), // 三成进度后才逐渐出现雨滴与雨势玩法影响。
        )
    } else if stops_raining {
        (
            smoothstep(remap_progress(progress, 0.35, 1.0)), // 雨势明显减弱后再逐渐散云并恢复天光。
            smoothstep(remap_progress(progress, 0.0, 0.70)), // 前七成过渡优先停雨并恢复视距。
        )
    } else {
        let factor = smoothstep(progress); // 不跨越有雨与无雨边界时保持普通同步平滑过渡。
        (factor, factor)
    }; // 完成云层阶段与降雨阶段进度计算。

    let cloud_output = interpolate_output(from, to, cloud_progress); // 云层阶段控制天空、光照与天气色调。
    let rain_output = interpolate_output(from, to, rain_progress); // 降雨阶段控制粒子、视距、风和玩法影响。
    WeatherOutput {
        surface_wetness: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
        puddle_amount: 0.0,   // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
        surface_effect_time: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
        cloud_cover: cloud_output.cloud_cover, // 云覆盖随聚云或散云阶段推进。
        precipitation: rain_output.precipitation, // 雨量使用延后开始或提前结束的独立进度。
        visibility: rain_output.visibility, // 能见度与雨幕出现和消失保持同步。
        wind: rain_output.wind, // 风力与降雨阶段同步，避免先出现暴雨风再出现雨滴。
        wind_direction: rain_output.wind_direction, // 风向与雨势阶段使用同一连续输出供所有表现层共享。
        direct_light_factor: cloud_output.direct_light_factor, // 直射光先被云幕压暗、停雨后再恢复。
        ambient_factor: cloud_output.ambient_factor, // 环境光跟随云幕阶段连续变化。
        min_ambient: cloud_output.min_ambient,      // 午夜可玩下限跟随天空阶段连续变化。
        sky_factor: cloud_output.sky_factor,        // 天空亮度由云层阶段统一控制。
        tint: cloud_output.tint,                    // 天空与天体光色调跟随云幕阶段变化。
        gameplay: rain_output.gameplay,             // 降雨玩法修正与实际雨势同阶段生效和解除。
    } // 返回分阶段组合后的自动天气连续输出。
} // 结束自然天气分阶段过渡解析。

/// 把某一子阶段的起止点映射为零至一进度。
fn remap_progress(progress: f32, start: f32, end: f32) -> f32 {
    ((progress - start) / (end - start).max(f32::EPSILON)).clamp(0.0, 1.0)
    // 固定阶段边界均合法，额外最小分母保护后续维护时误设相同端点。
} // 结束子阶段进度映射。

/// 将任意浮点进度安全规范化到零至一。
fn unit_progress(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0) // 有限进度直接钳制到合法定义域。
    } else {
        0.0 // 非有限进度安全保留当前天气完整效果。
    } // 返回规范化进度。
} // 结束进度规范化。

/// 返回合法归一化水平风向，损坏或零向量回退默认+X风向。
fn safe_wind_direction(direction: Vec2) -> Vec2 {
    if direction.is_finite() && direction.length_squared() > f32::EPSILON {
        if (direction.length_squared() - 1.0).abs() < 0.000_001 {
            direction // 已是单位向量时保持位值稳定，避免每帧重复归一化产生末位漂移。
        } else {
            direction.normalize() // 插值后长度明显变化时重新归一化，避免影响下游风速。
        }
    } else {
        DEFAULT_WIND_DIRECTION // 非有限或相互抵消的风向不能传播到雨、云和植被。
    } // 返回安全单位风向。
} // 结束风向规范化。

/// 返回两个标量的线性插值结果。
fn lerp(from: f32, to: f32, factor: f32) -> f32 {
    from + (to - from) * factor // 标准线性插值保证零端和一端精确命中目标值。
} // 结束标量线性插值。

/// 返回零至一输入的三次平滑曲线。
fn smoothstep(value: f32) -> f32 {
    let t = if value.is_finite() {
        value.clamp(0.0, 1.0) // 有限输入钳制到平滑曲线定义域。
    } else {
        0.0 // 非有限输入安全回退到过渡起点。
    }; // 完成平滑曲线输入规范化。
    t * t * (3.0 - 2.0 * t) // 三次smoothstep在两端一阶导数为零，避免突变感。
} // 结束平滑曲线计算。

#[cfg(test)] // 输出纯函数测试锁定五类参数、自动平滑和 F12 覆盖端点。
mod tests {
    use super::*; // 测试直接访问天气目标参数与插值函数。
    use crate::weather::constants::DEFAULT_TRANSITION_GAME_SECONDS; // 构造进行中的合法自动天气过渡。

    #[test] // 所有默认天气与损坏风向的退路都采用同一个中性+X方向。
    fn weather_profiles_share_neutral_default_wind() {
        for kind in WeatherKind::ALL {
            assert_eq!(weather_profile(kind).wind_direction, Vec2::X);
        }
        assert_eq!(safe_wind_direction(Vec2::ZERO), Vec2::X);
        assert_eq!(safe_wind_direction(Vec2::splat(f32::NAN)), Vec2::X);
        assert_eq!(safe_wind_direction(Vec2::new(0.0, 2.0)), Vec2::Y);
    }

    #[test] // 五类天气直射光和玩法影响应按设计逐步增强。
    fn weather_profiles_match_primary_design_factors() {
        let clear = weather_profile(WeatherKind::Clear); // 读取晴天完整目标参数。
        let light_rain = weather_profile(WeatherKind::LightRain); // 读取小雨完整目标参数。
        let heavy_rain = weather_profile(WeatherKind::HeavyRain); // 读取大雨完整目标参数。
        let mist = weather_profile(WeatherKind::Mist); // 读取薄雾完整目标参数。
        assert_eq!(clear.direct_light_factor, 1.0); // 晴天保留完整直射光。
        assert_eq!(light_rain.direct_light_factor, 0.35); // 小雨直射光降到三成半。
        assert_eq!(heavy_rain.direct_light_factor, 0.18); // 大雨直射光降到一成八。
        assert_eq!(heavy_rain.precipitation, 1.0); // 大雨使用完整雨量。
        assert_eq!(mist.gameplay.ai_visibility_factor, 0.55); // 薄雾AI视认距离降到五成半。
        assert_eq!(heavy_rain.min_ambient, MIN_PLAYABLE_AMBIENT); // 大雨仍保留午夜可玩环境光下限。
    } // 结束天气目标参数测试。

    #[test] // 自动降雨过渡中段必须已经聚云，但雨势仍落后于同步线性插值。
    fn automatic_rain_transition_builds_clouds_before_precipitation() {
        let state = WeatherState::from_snapshot(crate::WeatherSnapshot {
            current: WeatherKind::Overcast, // 从阴天开始过渡。
            target: WeatherKind::LightRain, // 目标是相邻小雨节点。
            transition_progress: 0.5,       // 使用平滑曲线中点，结果仍精确为二分之一。
            transition_duration_game_seconds: DEFAULT_TRANSITION_GAME_SECONDS, // 提供合法正过渡时长。
            stable_remaining_game_seconds: 0.0, // 过渡期不消费稳定剩余时长。
            ..crate::WeatherSnapshot::default()  // 其余字段使用合法默认值。
        }); // 完成自动过渡状态构造。
        let output = resolved_weather_output(&state, &WeatherDebugOverride::default()); // 解析自动模式连续输出。
        assert!(output.cloud_cover > 0.85); // 中点时云幕应已接近小雨目标，建立先聚云的视觉前奏。
        assert!(output.direct_light_factor < 0.42); // 中点时直射光应已被云幕明显压暗。
        assert!(output.precipitation > 0.0 && output.precipitation < 0.20); // 雨滴已经出现，但必须落后于普通同步中点雨量。
        assert!(output.wind > 0.30 && output.wind < 0.375); // 风力与实际雨势同步延后增长。
    } // 结束自动天气先聚云测试。

    #[test] // 自动停雨过渡前段必须先减弱雨势，云幕和压光仍保持降雨天气水平。
    fn automatic_rain_stop_reduces_precipitation_before_clouds_clear() {
        let state = WeatherState::from_snapshot(crate::WeatherSnapshot {
            current: WeatherKind::LightRain, // 从稳定小雨开始转为无雨阴天。
            target: WeatherKind::Overcast,   // 相邻目标不再产生降水。
            transition_progress: 0.20,       // 处于停雨优先阶段，尚未开始明显散云。
            transition_duration_game_seconds: DEFAULT_TRANSITION_GAME_SECONDS, // 提供合法正过渡时长。
            stable_remaining_game_seconds: 0.0, // 过渡期不消费稳定剩余时长。
            ..crate::WeatherSnapshot::default()  // 其余字段使用合法默认值。
        }); // 完成停雨状态构造。
        let output = resolved_weather_output(&state, &WeatherDebugOverride::default()); // 解析分阶段停雨输出。
        let rain = weather_profile(WeatherKind::LightRain); // 读取过渡起点完整小雨参数。
        assert_eq!(output.cloud_cover, rain.cloud_cover); // 前两成进度云幕应仍保持小雨覆盖度。
        assert_eq!(output.direct_light_factor, rain.direct_light_factor); // 天光应等待停雨明显后再恢复。
        assert!(output.precipitation < rain.precipitation); // 雨量必须已经开始下降。
        assert!(output.visibility > rain.visibility); // 雨幕变弱时能见度应同步恢复。
    } // 结束自动天气先停雨测试。

    #[test] // F12 强制预览零端保持自动输出，一端命中完整目标天气。
    fn debug_preview_hits_both_endpoints() {
        let state = WeatherState::default(); // 自动天气保持默认晴天。
        let mut debug = WeatherDebugOverride::default(); // 默认从自动模式与完整预览开始。
        debug.set_mode_index(4); // F12 模式四对应大雨。
        debug.set_preview(0.0); // 把预览滑块拖到左端。
        let automatic_endpoint = resolved_weather_output(&state, &debug); // 解析预览零端输出。
        assert_eq!(automatic_endpoint, weather_profile(WeatherKind::Clear)); // 零端必须完全保持自动晴天。
        debug.set_preview(1.0); // 把预览滑块拖到右端。
        let forced_endpoint = resolved_weather_output(&state, &debug); // 解析预览一端输出。
        assert_eq!(
            forced_endpoint,
            WeatherOutput {
                surface_wetness: 1.0,
                puddle_amount: 1.0,
                ..weather_profile(WeatherKind::HeavyRain)
            }
        ); // 一端同时完整预览大雨和饱和地表。
    } // 结束 F12 强制预览端点测试。

    #[test] // F12 从晴天预览降雨时也必须先看到聚云压光，再逐渐出现雨滴。
    fn debug_rain_preview_uses_same_staged_transition_as_automatic_weather() {
        let state = WeatherState::default(); // 自动背景保持完整晴天。
        let mut debug = WeatherDebugOverride::default(); // 从自动模式构造强制预览。
        debug.set_mode_index(4); // 选择完整大雨目标。
        debug.set_preview(0.20); // 前两成预览仍处于只聚云、尚未落雨阶段。
        let output = resolved_weather_output(&state, &debug); // 解析 F12 分阶段预览输出。
        assert!(output.cloud_cover > weather_profile(WeatherKind::Clear).cloud_cover); // 云幕必须已经开始增加。
        assert!(output.direct_light_factor < 1.0); // 天体直射光必须随聚云先被压暗。
        assert_eq!(output.precipitation, 0.0); // 三成进度前不得出现晴空雨滴。
    } // 结束 F12 先聚云预览测试。

    #[test] // 自动模式拖动预览滑块必须临时覆盖真实过渡进度并直接检查当前到既定目标。
    fn automatic_mode_preview_overrides_state_progress_only_after_user_input() {
        let state = WeatherState::from_snapshot(crate::WeatherSnapshot {
            current: WeatherKind::Overcast, // 自动天气正在从阴天形成小雨。
            target: WeatherKind::LightRain, // 使用已有自动目标，不创建强制天气。
            transition_progress: 0.10,      // 状态机真实进度仍处于早期聚云阶段。
            transition_duration_game_seconds: DEFAULT_TRANSITION_GAME_SECONDS, // 提供合法自动过渡时长。
            stable_remaining_game_seconds: 0.0, // 过渡期没有稳定剩余时间。
            ..crate::WeatherSnapshot::default()  // 其余快照字段使用合法默认值。
        }); // 完成自动过渡状态构造。
        let mut debug = WeatherDebugOverride::default(); // 默认自动模式尚未手动接管预览。
        let natural = resolved_weather_output(&state, &debug); // 解析状态机真实一成进度输出。
        debug.set_preview(0.80); // F12 拖动到八成以检查接近目标天气的画面。
        let previewed = resolved_weather_output(&state, &debug); // 自动模式按手动八成比例重新解析。
        assert_eq!(debug.forced(), None); // 拖预览不能把自动模式改成某种强制天气。
        assert!(debug.preview_override_active()); // 用户输入必须显式标记临时预览接管。
        assert!(previewed.precipitation > natural.precipitation); // 八成预览应显示比真实一成进度更多的雨量。
        assert!(previewed.direct_light_factor < natural.direct_light_factor); // 八成预览应显示更明显的云层压光。
    } // 结束自动天气手动预览测试。
} // 结束天气输出纯函数测试模块。
