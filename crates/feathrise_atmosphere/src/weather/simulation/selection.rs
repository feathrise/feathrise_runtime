// 天气抽取纯逻辑：实现合法状态图、输入气候权重、持续时间和确定性随机值。

use crate::weather::climate::WeatherClimateProfile; // 正式地图通过显式气候配置提供季节权重与局部倍率。
use crate::weather::constants::{
    // 导入自动天气抽取使用的统一时长与晨雾窗口。
    GAME_SECONDS_PER_HOUR,        // 小时配置需要转换为游戏秒。
    MAX_TRANSITION_GAME_SECONDS,  // 普通过渡随机上限为三十分钟。
    MIN_TRANSITION_GAME_SECONDS,  // 普通过渡随机下限为二十分钟。
    MIST_DISPERSAL_START_HOUR,    // 薄雾稳定期必须在九点半前结束。
    MIST_GENERATION_END_HOUR,     // 薄雾只允许在八点前进入候选池。
    MIST_GENERATION_START_HOUR,   // 薄雾只允许在三点后进入候选池。
    RAIN_TRANSITION_GAME_SECONDS, // 小雨和大雨之间固定使用十分钟过渡。
}; // 完成天气抽取常量导入。
use crate::weather::types::WeatherKind; // 抽取、状态图和持续时间都围绕公开天气种类计算。

/// 返回两个天气是否相同或在状态图上直接相邻。
pub(crate) fn is_same_or_adjacent(from: WeatherKind, to: WeatherKind) -> bool {
    if from == to {
        return true; // 自身重抽用于延长当前稳定天气，不构成非法跳转。
    } // 结束自身节点快速判断。
    matches!(
        (from, to), // 状态图边全部声明为双向连接。
        (WeatherKind::Clear, WeatherKind::Overcast) // 晴天只能先转阴再进入降雨。
            | (WeatherKind::Overcast, WeatherKind::Clear) // 阴天可以恢复晴朗。
            | (WeatherKind::Overcast, WeatherKind::LightRain) // 阴天可以逐渐形成小雨。
            | (WeatherKind::LightRain, WeatherKind::Overcast) // 小雨停止后先回到阴天。
            | (WeatherKind::LightRain, WeatherKind::HeavyRain) // 小雨可以增强为大雨。
            | (WeatherKind::HeavyRain, WeatherKind::LightRain) // 大雨必须先减弱为小雨。
            | (WeatherKind::Clear, WeatherKind::Mist) // 晴朗清晨可以形成薄雾。
            | (WeatherKind::Mist, WeatherKind::Clear) // 薄雾可以直接散为晴天。
            | (WeatherKind::Overcast, WeatherKind::Mist) // 阴天清晨也可以形成薄雾。
            | (WeatherKind::Mist, WeatherKind::Overcast)  // 薄雾可以散为阴天。
    ) // 返回状态图边匹配结果。
} // 结束相邻节点判断。

/// 将任意请求路由到不会越过状态图的下一节点。
pub(crate) fn safe_next_step(from: WeatherKind, requested: WeatherKind) -> WeatherKind {
    if is_same_or_adjacent(from, requested) {
        return requested; // 相同或直接相邻的请求可以原样执行。
    } // 结束合法请求快速返回。
    match from {
        WeatherKind::Clear => WeatherKind::Overcast, // 晴天前往任何降雨都必须先转阴。
        WeatherKind::Overcast => match requested {
            WeatherKind::HeavyRain => WeatherKind::LightRain, // 阴天前往大雨必须先形成小雨。
            _ => requested, // 其余非相邻情况理论上不可达，保留请求便于未来扩展。
        }, // 结束阴天路径选择。
        WeatherKind::LightRain => WeatherKind::Overcast, // 小雨前往晴天或薄雾必须先停止降雨。
        WeatherKind::HeavyRain => WeatherKind::LightRain, // 大雨离开当前节点必须先减弱为小雨。
        WeatherKind::Mist => WeatherKind::Overcast,  // 薄雾前往降雨必须先形成阴天。
    } // 返回合法下一节点。
} // 结束安全路径选择。

/// 返回指定月份、时刻与地图偏置下的天气权重。
pub(crate) fn weather_weight(
    kind: WeatherKind,              // 需要查询的候选天气。
    month: u8,                      // 一至十二月决定春夏秋冬基础权重。
    hour: f32,                      // 当前时刻决定薄雾能否进入候选池。
    climate: WeatherClimateProfile, // 显式地图配置提供季节基础权重和局部倍率。
) -> f32 {
    if kind == WeatherKind::Mist && !mist_can_generate(hour) {
        return 0.0; // 非凌晨至早晨窗口绝不自动生成新薄雾。
    } // 结束薄雾生成窗口守卫。
    climate.weight(kind, month) // 气候配置统一返回季节基础权重叠加地图局部倍率。
} // 结束天气权重计算。

/// 按当前天气相邻节点与自身节点进行确定性加权抽取。
pub(crate) fn select_next_weather(
    current: WeatherKind,           // 当前节点决定允许进入的候选集合。
    month: u8,                      // 月份决定季节权重。
    hour: f32,                      // 时刻决定薄雾是否可生成。
    climate: WeatherClimateProfile, // 当前地图显式气候配置。
    random_unit: f32,               // 零至一的确定性随机样本。
) -> WeatherKind {
    let mut total_weight = 0.0; // 先累计所有合法候选的总权重。
    for candidate in WeatherKind::ALL {
        if is_same_or_adjacent(current, candidate) {
            total_weight += weather_weight(candidate, month, hour, climate);
            // 自身与相邻节点进入抽取池。
        } // 结束单个候选合法性判断。
    } // 完成候选总权重统计。
    if total_weight <= f32::EPSILON {
        return current; // 极端权重配置下延长当前天气，避免除零或非法跳转。
    } // 结束空候选池保护。
    let normalized_random = if random_unit.is_finite() {
        random_unit.clamp(0.0, 1.0 - f32::EPSILON) // 有限样本钳制到左闭右开单位区间。
    } else {
        0.0 // 非有限测试输入安全回退到首个候选。
    }; // 完成随机样本规范化。
    let mut threshold = normalized_random * total_weight; // 将单位样本映射到候选累计权重区间。
    for candidate in WeatherKind::ALL {
        if !is_same_or_adjacent(current, candidate) {
            continue; // 非相邻节点不参与抽取，保证晴天不会直接跳大雨。
        } // 结束非法候选跳过。
        let weight = weather_weight(candidate, month, hour, climate); // 读取候选最终季节与地图权重。
        if threshold < weight {
            return candidate; // 首个覆盖阈值的候选成为下一天气。
        } // 结束当前候选命中判断。
        threshold -= weight; // 未命中时扣除当前区间并继续查找。
    } // 完成加权候选遍历。
    current // 浮点舍入落到区间外时安全保留当前天气。
} // 结束下一天气抽取。

/// 返回指定天气的随机稳定时长，并限制晨雾在九点半前开始消散。
pub(crate) fn stable_duration_game_seconds(
    kind: WeatherKind, // 已经进入稳定期的天气。
    random_unit: f32,  // 零至一的确定性持续时长样本。
    hour: f32,         // 当前时刻用于钳制薄雾持续上限。
) -> f32 {
    let (minimum_hours, maximum_hours) = match kind {
        WeatherKind::Clear => (4.0, 10.0),    // 晴天持续四至十游戏小时。
        WeatherKind::Overcast => (3.0, 7.0),  // 阴天持续三至七游戏小时。
        WeatherKind::LightRain => (2.0, 5.0), // 小雨持续二至五游戏小时。
        WeatherKind::HeavyRain => (1.0, 3.0), // 大雨持续一至三游戏小时。
        WeatherKind::Mist => (1.0, 3.0),      // 薄雾持续一至三游戏小时但受消散时刻约束。
    }; // 完成天气持续范围选择。
    let normalized_random = if random_unit.is_finite() {
        random_unit.clamp(0.0, 1.0) // 有限样本钳制到完整单位区间。
    } else {
        0.0 // 非有限输入使用对应天气最短时长。
    }; // 完成持续样本规范化。
    let sampled_hours = minimum_hours + (maximum_hours - minimum_hours) * normalized_random; // 在线性范围内抽取稳定小时数。
    let sampled_seconds = sampled_hours * GAME_SECONDS_PER_HOUR; // 所有状态字段统一保存游戏秒。
    if kind != WeatherKind::Mist {
        return sampled_seconds; // 非薄雾天气不需要早晨时刻上限。
    } // 结束普通天气快速返回。
    let remaining_until_dispersal = (MIST_DISPERSAL_START_HOUR - hour.max(0.0))
        .max(0.0) // 九点半以后薄雾稳定期立即结束。
        * GAME_SECONDS_PER_HOUR; // 将剩余小时换算为游戏秒。
    sampled_seconds.min(remaining_until_dispersal) // 薄雾最迟在九点半进入消散过渡。
} // 结束稳定天气时长计算。

/// 返回两个相邻天气之间的自动过渡时长。
pub(crate) fn transition_duration_game_seconds(
    from: WeatherKind, // 当前过渡起点天气。
    to: WeatherKind,   // 当前过渡目标天气。
    random_unit: f32,  // 普通过渡在二十至三十分钟内随机。
) -> f32 {
    if matches!(
        (from, to), // 小雨与大雨之间无论增强或减弱都使用同一速度。
        (WeatherKind::LightRain, WeatherKind::HeavyRain)
            | (WeatherKind::HeavyRain, WeatherKind::LightRain)
    ) {
        return RAIN_TRANSITION_GAME_SECONDS; // 雨势变化固定在十游戏分钟完成。
    } // 结束降雨相邻节点快速返回。
    let normalized_random = if random_unit.is_finite() {
        random_unit.clamp(0.0, 1.0) // 有限样本钳制到完整单位区间。
    } else {
        0.5 // 非有限输入使用普通过渡中点二十五分钟。
    }; // 完成过渡样本规范化。
    MIN_TRANSITION_GAME_SECONDS
        + (MAX_TRANSITION_GAME_SECONDS - MIN_TRANSITION_GAME_SECONDS) * normalized_random
    // 普通过渡平滑分布在二十至三十分钟。
} // 结束自动过渡时长计算。

/// 返回当前时刻能否自动生成薄雾。
pub(crate) fn mist_can_generate(hour: f32) -> bool {
    hour.is_finite() // 非有限时刻不能进入天气概率计算。
        && (MIST_GENERATION_START_HOUR..MIST_GENERATION_END_HOUR).contains(&hour)
    // 只允许03:00至08:00左闭右开窗口。
} // 结束薄雾生成窗口判断。

/// 根据种子和序号生成跨平台稳定的零至一随机样本。
pub(crate) fn deterministic_random_unit(seed: u64, sequence: u64) -> f32 {
    let mut value = seed.wrapping_add(sequence.wrapping_mul(0x9E37_79B9_7F4A_7C15)); // 使用黄金比例步长分离连续序号。
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9); // SplitMix64第一轮混合扩散高低位。
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB); // SplitMix64第二轮混合减少相邻种子相关性。
    value ^= value >> 31; // 最终异或折叠获得均匀六十四位输出。
    let mantissa = (value >> 40) as u32; // 取高二十四位以匹配f32有效精度。
    mantissa as f32 / 16_777_216.0 // 除以二的二十四次方得到左闭右开单位区间。
} // 结束确定性随机样本生成。

#[cfg(test)] // 纯函数测试锁定季节权重、状态图和时间窗口。
mod tests {
    use super::*; // 测试直接访问本模块私有抽取函数。
    use crate::{UNIFORM_CLIMATE_PROFILE, WeatherRegion}; // 默认退路与自定义气候都使用中性区域。

    #[test] // 晴天任何随机样本都不能直接跳到小雨或大雨。
    fn clear_weather_only_selects_itself_overcast_or_mist() {
        for sample_index in 0..100 {
            let sample = sample_index as f32 / 100.0; // 遍历单位区间的一百个稳定样本。
            let selected =
                select_next_weather(WeatherKind::Clear, 6, 12.0, UNIFORM_CLIMATE_PROFILE, sample); // 正午薄雾不进入候选池。
            assert!(matches!(
                selected,
                WeatherKind::Clear | WeatherKind::Overcast
            )); // 晴天只能延长自身或先转阴。
        } // 完成全部随机样本验证。
    } // 结束晴天状态图测试。

    #[test] // 薄雾只在凌晨三点至八点进入权重池。
    fn mist_generation_window_is_left_closed_and_right_open() {
        assert!(!mist_can_generate(2.999)); // 三点前不能生成薄雾。
        assert!(mist_can_generate(3.0)); // 三点整允许生成薄雾。
        assert!(mist_can_generate(7.999)); // 八点前仍允许生成薄雾。
        assert!(!mist_can_generate(8.0)); // 八点整起不再生成新薄雾。
    } // 结束薄雾窗口测试。

    #[test] // 输入的零权重必须真正关闭候选，不能被均匀默认值覆盖。
    fn custom_zero_weights_select_only_the_enabled_neighbor() {
        let climate = WeatherClimateProfile::new(
            WeatherRegion::Global,
            true,
            [[0.0, 0.0, 2.0, 0.0, 0.0]; 4],
            [1.0; 5],
        ); // 阴天的合法相邻候选里只开放小雨。
        for sample_index in 0..100 {
            assert_eq!(
                select_next_weather(
                    WeatherKind::Overcast,
                    12,
                    12.0,
                    climate,
                    sample_index as f32 / 100.0
                ),
                WeatherKind::LightRain,
            );
        }
    }

    #[test] // 禁用配置和全零配置必须保留当前节点，避免空概率池触发非法跳转。
    fn disabled_or_empty_profiles_keep_current_weather() {
        for climate in [
            WeatherClimateProfile::new(WeatherRegion::Global, false, [[1.0; 5]; 4], [1.0; 5]),
            WeatherClimateProfile::new(WeatherRegion::Global, true, [[0.0; 5]; 4], [1.0; 5]),
        ] {
            for current in WeatherKind::ALL {
                assert_eq!(select_next_weather(current, 6, 6.0, climate, 0.5), current);
            }
        }
    }

    #[test] // 中性默认气候不会暗中按季节关闭大雨，但依然遵守状态图。
    fn uniform_profile_allows_heavy_rain_in_winter_from_light_rain() {
        assert_eq!(
            weather_weight(WeatherKind::HeavyRain, 12, 12.0, UNIFORM_CLIMATE_PROFILE),
            1.0
        );
        assert_eq!(
            select_next_weather(
                WeatherKind::LightRain,
                12,
                12.0,
                UNIFORM_CLIMATE_PROFILE,
                0.9
            ),
            WeatherKind::HeavyRain,
        );
    }

    #[test] // 同一种子和序号必须产生相同随机样本且连续序号不同。
    fn deterministic_random_is_reproducible() {
        let first = deterministic_random_unit(42, 1); // 获取固定种子第一序号样本。
        let repeated = deterministic_random_unit(42, 1); // 再次获取完全相同输入样本。
        let next = deterministic_random_unit(42, 2); // 获取相邻序号样本。
        assert_eq!(first, repeated); // 相同输入必须跨读档稳定复现。
        assert_ne!(first, next); // 连续序号不应退化为相同样本。
        assert!((0.0..1.0).contains(&first)); // 样本必须位于左闭右开单位区间。
    } // 结束确定性随机测试。
} // 结束天气抽取纯函数测试模块。
