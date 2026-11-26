//! 工程变量与事件规则。作者定义在启动时编译一次，游戏和脚本共享同一局的状态。

mod definitions;
mod dispatch;
mod locations;
mod resources;
mod types;

pub use locations::LocationTracker;
pub use resources::ProjectVariables;
pub use types::{ProjectVariablesSnapshot, Value};

/// 相对于工程主资产根，与作者编辑和发布后的查找位置一致。
pub const RULES_FILE: &str = "project_rules.json";

#[cfg(test)]
mod tests;
