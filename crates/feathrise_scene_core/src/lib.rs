//! 场景基础契约：固定通道、视图资格、生命周期、输入许可和驻留所有权。
//!
//! 五个功能域保持各自模块与原调度行为；底座不依赖场景实现、设置或厂商后端。

pub mod constants;
pub mod view;
pub mod state;
pub mod input;
pub mod residency;
