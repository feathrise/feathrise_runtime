//! 大场景公共场景运行时。
//! 统一生成动态场景环境，并管理规划显隐及资源、碰撞和显式应用交接门。

mod systems; // 场景公共环境、规划显隐与场景启动系统。
mod types; // 场景环境配置。

pub(crate) use systems::*; // 向crate内各场景开放公共生成与启动系统。
pub(crate) use types::*; // 向crate内各场景开放环境配置。
