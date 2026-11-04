//! 动态地形就绪状态驱动的发现、GPU预热、原子激活、历史重置与完整回退。

mod deferred_bridge; // 仍由光栅绘制的角色/植被材质在Solari期写入有效Deferred GBuffer。
mod deferred_views; // 地图与角色预览离屏相机在全局Deferred材质桥期间保持可见并精确回退。
mod discovery; // 当前完整候选资产契约与预算核对。
mod history; // 相机、尺寸、地图和天体光不连续时重置Solari历史。
mod materials; // 普通/通用地表材质副本、代理、预热和原子切换。
mod systems; // 请求、换代、超时、热更、回退和公开状态协调。
mod target; // 动态地图唯一正式地形的目标登记和存活核对。

pub(crate) use deferred_bridge::{maintain_deferred_raster_bridge, restore_deferred_raster_bridge};
pub(crate) use deferred_views::maintain_deferred_auxiliary_camera_leases;
pub(crate) use discovery::discover_candidates;
pub(crate) use history::reset_solari_history_on_discontinuity;
pub(crate) use materials::{drive_gpu_preparation_and_warmup, validate_active_sources};
pub(crate) use systems::{
    apply_pending_rollback, coordinate_lifecycle, monitor_source_asset_changes,
    publish_public_status,
};
pub(crate) use target::observe_dynamic_map_target;
