//! 动态场景独立植被密度层运行时。
//!
//! Blade层复用通用GPU候选窗口；Mesh层冻结同一密度与高度源，再按相机范围精确生成固定空间块。
//! 修订源按身份采用，稳定帧不扫描整个场景，空间常驻也不改变作者密度。

mod clipmap; // 已采用分页高度轻量同步。
pub(super) use clipmap::sync_dynamic_blade_clipmap; // 场景调度在建根后发布同代高度。
mod build; // 当前修订CPU源冻结与单空间块精确后台散布。
mod draft; // F12未保存Blade定义与内存密度的完整快照。
pub use draft::{DynamicBladeDraft, DynamicBladeDraftSnapshot}; // 编辑器只发布完整输入，不操作运行时窗口。
mod clearance; // Blade建筑净空从原始R8后台派生临时单Mip图片。
mod collision; // 玩家近场按预设创建圆柱或盒体代理，远景共享批次不承担物理实体。
mod components; // 根实体和玩家跟随GPU窗口标记。
#[cfg(test)]
pub(in crate::scenes::dynamic_scene) use components::DynamicBladeWindow; // 高度采用回归核对真实四档窗口，不另造消费组件。
mod follow; // 稳定帧内有界更新GPU叶片窗口。
mod helpers; // 作者定义校验、数值纹理解码与资产请求。
mod interaction; // 固定八槽玩家压草与回弹足迹。
mod loading; // 目录事件观察、定义原子校验与当前修订强资产请求。
mod residency; // Mesh层按相机与LOD范围常驻固定空间块，不截断全图作者密度。
mod resources; // 唯一事务、待采用计划和公开诊断状态。
mod spawn; // 建立Blade修订根，并把Mesh块分帧排入隐藏父根后一次显示。
mod systems; // 事件驱动加载、非阻塞轮询、热更与窗口跟随。
mod transactions; // 首载与热更共用失败收口，避免状态分叉。
mod volume;
pub(super) fn install_volume(app:&mut bevy::prelude::App) {volume::install(app);}
pub(in crate::scenes::dynamic_scene) fn add_volume_instance_systems(schedule:&mut bevy::prelude::Schedule) {volume::add_instance_systems(schedule);}

pub(super) use clearance::drive_dynamic_blade_clearance;
pub(super) use follow::follow_dynamic_blade_windows;
pub(super) use follow::follow_subscene_blade_windows;
pub(super) use interaction::{reset_dynamic_blade_interaction, update_dynamic_blade_interaction};
pub(super) use loading::{
    observe_dynamic_vegetation_assets, prepare_dynamic_vegetation_definition,
};
pub(super) use residency::drive_dynamic_mesh_residency;
pub(super) use resources::DynamicVegetationState;
pub use resources::{DynamicVegetationPhase, DynamicVegetationStatus};
pub(super) use systems::{
    begin_dynamic_vegetation, clear_dynamic_vegetation, commit_dynamic_vegetation_for_terrain,
    install_dynamic_vegetation, start_dynamic_vegetation_build,
};
