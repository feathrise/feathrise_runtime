// 损坏快照与外部天气时长参数的安全规范化。

use super::*;

/// 将零随机种子替换为项目默认值。
pub(super) fn normalize_seed(seed: u64) -> u64 {
    if seed == 0 {
        DEFAULT_WEATHER_SEED // 零值通常来自缺失或损坏快照，恢复稳定默认序列。
    } else {
        seed // 任意非零种子都是合法可复现序列。
    } // 返回规范化随机种子。
} // 结束随机种子规范化。

/// 将外部或存档中的单段天气过渡时长规范化为安全正数。
pub(super) fn normalize_transition_duration(duration_game_seconds: f32) -> f32 {
    if duration_game_seconds.is_finite() && duration_game_seconds > 0.0 {
        duration_game_seconds.clamp(1.0, MAX_SNAPSHOT_TRANSITION_GAME_SECONDS)
        // 有限正数限制在一游戏秒至六游戏小时，兼顾即时工具与损坏存档保护。
    } else {
        DEFAULT_TRANSITION_GAME_SECONDS // 零、负数和非有限值统一使用二十五游戏分钟默认值。
    } // 返回规范化过渡时长。
} // 结束天气过渡时长规范化。

/// 将剧情最低维持时间规范化为零至三十游戏日。
pub(super) fn normalize_hold_duration(hold_game_seconds: f32) -> f32 {
    if hold_game_seconds.is_finite() {
        hold_game_seconds.clamp(0.0, MAX_STABLE_GAME_SECONDS) // 负数回到零，异常超长请求限制到存档安全上限。
    } else {
        0.0 // 非有限剧情参数不应锁死自动天气，安全视为没有额外最低维持时间。
    } // 返回规范化最低维持秒数。
} // 结束剧情最低维持时间规范化。
