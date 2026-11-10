//! 场景作者动态天空/天空盒的相机级异步加载、Cube 校验、热更新与精确回退。

mod components; // 公开请求/状态与私有原状态快照。
mod environment; // 小型漫反射Cube缩放缓存把补光与高分辨率镜面强度分开。
mod systems; // 只响应请求和Image资产事件的有界运行时。

pub use components::{
    AuthoredSkyMode, AuthoredSkyboxPhase, AuthoredSkyboxRequest, AuthoredSkyboxStatus,
}; // 场景与 F12 共用同一互斥模式、请求入口和诊断。
pub(crate) use environment::install_authored_environment_map_support; // 插件启动时建立共享黑Cube与有界派生缓存。
pub(crate) use systems::{
    apply_authored_skyboxes, begin_authored_skybox_requests, observe_authored_skybox_assets,
}; // 世界时间插件固定加载、事件与采用顺序。
