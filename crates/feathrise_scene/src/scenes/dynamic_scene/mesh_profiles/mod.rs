//! 动态场景按需加载开发者目录声明的完整数据Mesh原型。
//!
//! 空密度层不会触发大GLB；F12选择或运行时发现非零密度后，每次只解码一个预设，
//! 完整原型一次登记到公共注册表。任何失败都保持密度可编辑并拒绝简化替身。

mod mesh_proxy; // 灌木薄片和岩石凸包复用通用确定性算法。
mod generic; // 完全数据驱动的任意 LOD/部件 Mesh 预设加载与原型装配。
mod resources;
mod readiness;
pub use readiness::DynamicMeshProfileReadiness;
mod systems;
mod cache; // 父图、子场景与独立地形共享单飞加载和最后租约回收。

pub(super) use resources::{DynamicMeshProfileState, DynamicMeshProfileCache};
pub use resources::{
    DynamicMeshProfilePhase, DynamicMeshProfileRequests, DynamicMeshProfileStatus,
};
pub(super) use systems::{
    begin_dynamic_mesh_profiles, clear_dynamic_mesh_profiles, drive_dynamic_mesh_profiles,
    update_dynamic_mesh_profile_wind, collect_dynamic_mesh_profiles,
};
