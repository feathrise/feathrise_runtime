// 天气消息：声明剧情、调试工具和存档流程可以发送的离散控制请求。

use bevy::prelude::*; // Message 派生用于Bevy 0.18消息队列。

use crate::weather::types::WeatherKind; // 过渡请求需要携带目标天气种类。

/// 天气控制消息。
#[derive(Message, Clone, Copy, Debug, PartialEq)] // 控制请求是无需所有权资源的轻量值消息。
pub enum WeatherCommand {
    /// 请求沿合法状态图向指定天气过渡。
    TransitionTo {
        kind: WeatherKind, // 最终意图天气；不相邻时先移动到安全的下一节点。
        duration_game_seconds: Option<f32>, // None使用二十五游戏分钟默认过渡时长。
        minimum_hold_game_seconds: Option<f32>, // 到达最终目标后至少维持的游戏秒数，None沿用自然稳定时长。
    }, // 结束剧情或工具天气过渡请求。
    /// 立即把自动天气状态落到指定天气并结束过渡。
    ApplyImmediate {
        kind: WeatherKind, // F1选中的目标天气会成为新的当前和目标节点。
    }, // 结束天气立即应用请求。
    /// 按当前季节、时刻、状态图和地图偏置重新抽取下一目标。
    Reroll, // 只在自动模式执行，强制天气模式下忽略该请求。
    /// 清除F12运行时调试天气并恢复自动输出。
    ClearOverride, // 只清调试覆盖，不重置自动天气状态和剩余时长。
    /// 更换确定性随机种子并重置随机序号。
    Reseed {
        seed: u64, // 零值会规范化为项目默认种子。
    }, // 结束天气随机序列重置请求。
} // 结束天气控制消息声明。
