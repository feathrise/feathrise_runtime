//! 输入来源、许可投影、暂停应用和光标交接的公共调度契约。

use bevy::prelude::*; // SystemSet 派生用于建立 UI 状态、暂停应用和世界推进的稳定顺序。

/// 所有来源先完成收集与许可发布，再由公共唯一写侧应用时间。
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)] // 调度标签只描述顺序，不携带运行时状态。
pub enum GamePauseSystemSet {
    /// 应用和作者工具先完成各自状态变化，不在这里发布半份最终许可。
    CollectBlockers,
    /// 把应用具体原因发布为共享许可，等待作者显隐及所有UI收集完成。
    PublishGateFacts,
    /// 把最终阻塞状态统一应用到虚拟时间与物理时间。
    Apply, // 世界时钟推进必须排在该阶段之后，避免弹框打开帧多走一步。
}

/// 玩法窗口系统鼠标的唯一应用阶段；UI插件可在此前发布自定义光标意图。
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameCursorSystemSet {
    /// 根据最终普通UI与自定义光标状态写入`CursorOptions`。
    Apply,
}
