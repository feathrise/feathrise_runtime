// 世界时间普通类型：声明可序列化存档快照和公开系统调度集。

use bevy::prelude::*; // SystemSet 派生用于跨 crate 固定输入、推进与光照顺序。
use serde::{Deserialize, Serialize}; // 世界时间快照需要兼容 JSON 存档读写。

/// 可序列化的世界时间存档快照；恢复时会统一钳制损坏或越界字段。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)] // 轻量值类型便于存档层直接复制。
#[serde(default)] // 旧版或字段不完整的 JSON 使用世界时钟默认值补齐。
pub struct WorldTimeSnapshot {
    /// 环境月份。
    pub month: u8, // 合法范围为一至十二，恢复接口负责钳制。
    /// 当月日期。
    pub day: u8, // 合法上限取决于月份，恢复接口负责钳制。
    /// 从午夜起经过的秒数，允许 86400 保存 24:00 哨兵。
    pub seconds_since_midnight: f32, // 非有限值和越界值会在恢复时回退或钳制。
}

/// 世界时间系统调度集，供测试面板把滑块输入稳定排在时间推进之前。
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)] // 调度标签不携带运行期状态。
pub enum WorldTimeSystemSet {
    /// 自动推进世界时钟的阶段。
    Advance, // 外部时间输入应使用 before(Advance) 以在同帧生效。
    /// 时间与天气快照统一保存或恢复的阶段。
    SnapshotIo, // 存读档固定发生在模拟推进后、环境解析和最终光照前。
    /// 根据最终时刻应用方向光、环境光与天空颜色的阶段。
    ApplyLighting, // 光照阶段固定在推进阶段之后执行。
}
