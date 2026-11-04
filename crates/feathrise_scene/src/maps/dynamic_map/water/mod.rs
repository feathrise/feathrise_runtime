//! 动态地图R8水域与GLB派生内存河床的热加载、后台建网格与原子采用。
//!
//! 已保存水面与F12作者预览最终都交给共享`large_map::water`材质和波纹系统；本模块只负责
//! 当前MapId、R8作者源/GLB派生代次、single-flight后台任务和运行时实体所有权；水域KTX2保留为GPU派生物。

mod asset_events; // 只匹配当前强句柄，并让新成功代次优先于迟到失败消息。
mod build; // 消费共享R8作者源快照并在后台调用统一静水与样条水建网格纯函数。
mod clipmap; // Geometry Clipmap共享正式环带/高度页的静水专用后端。
mod resources; // 公开阶段、诊断与唯一写侧事务资源。
mod runtime_batch; // 三类水体作者值冻结与统一实体安装，保持系统编排文件紧凑。
mod systems; // R8/内存高度输入冻结、任务启动、迟到丢弃和原子实体替换。
pub(super) fn install_volume(app:&mut bevy::prelude::App){clipmap::install_volume(app);}
pub(in crate::maps::dynamic_map) use clipmap::add_volume_instance_systems;

pub(super) use asset_events::observe_dynamic_water_assets; // 父插件在准备系统前合并全部当前输入事件。
pub(crate) use clipmap::ClipmapRainHeightMaterial; // 雨水捕获只在scene crate内复用专用材质。
pub use clipmap::{ClipmapSplineWaterSurface, ClipmapWaterMaterial, ClipmapWaterSurface};
pub(super) use clipmap::{
    ClipmapWaterMaterialPlugin, ClipmapWaterRuntime, begin_clipmap_water, clear_clipmap_water,
    drive_clipmap_water, follow_clipmap_water,
};
#[cfg(test)]
pub(crate) use clipmap::{test_clipmap_water_material, test_clipmap_water_surface};
pub(super) use resources::DynamicWaterState; // 动态地图插件初始化内部single-flight状态。
pub use resources::{
    DynamicClipmapWaterDraft, DynamicClipmapWaterDraftSnapshot, DynamicWaterPhase,
    DynamicWaterRebuildRequest, DynamicWaterStatus, TerrainWaterRebuildRequest,
}; // F3/F12读取状态，编辑器发布完整Clipmap静水草稿，保存交接显式请求新事务代次。
pub(super) use systems::{
    begin_dynamic_water, clear_dynamic_water, commit_dynamic_water_for_terrain,
    install_dynamic_water, prepare_dynamic_water,
}; // 父插件按地图生命周期装配水域事务。
