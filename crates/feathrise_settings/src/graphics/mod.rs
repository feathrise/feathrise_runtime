// 游戏设置 — 画质运行时功能域。

mod dlss; // 独立 DLSS SR 相机接管、首次硬件默认与 SR/RR 交接。
mod fsr; // FSR 3候选相机接管、DLSS互斥、状态与安全回退。
mod render_scale; // 窗口 3D 主通道固定分辨率缩放与空间放大。
mod runtime_lease; // 可选渲染功能的目标相机附件与标准阴影临时租约。
mod systems; // 按窗口、相机后处理和阴影职责拆分的应用系统。
mod texture_filtering; // StandardMaterial 纹理过滤和有界 RenderApp 采样器接管。

pub(crate) use render_scale::{apply_render_scale, configure_render_scale}; // 主世界应用与 RenderApp 配置入口。
pub use runtime_lease::{
    GraphicsRuntimeLeaseId, GraphicsRuntimeLeaseRegistry, GraphicsRuntimeLeaseSystems,
    GraphicsRuntimeRequirements,
}; // 对外只开放声明式租约，不开放私有作者状态组件。
pub(crate) use runtime_lease::{
    apply_graphics_runtime_leases, restore_released_graphics_runtime_leases,
}; // 作者附件恢复与合并租约应用入口。
pub(crate) use texture_filtering::configure_texture_filtering; // 纹理过滤 RenderApp 注册入口。

pub use dlss::DlssRuntimeSystems; // 光追插件通过公开排序锚点协调 RR 交接。
pub(crate) use dlss::configure_dlss; // 由画质插件统一注册 DLSS 状态与可选 SR 系统。
pub(crate) use fsr::configure_fsr3; // 由画质插件统一注册FSR候选状态与可选运行时桥。
pub(crate) use systems::{
    apply_ambient_occlusion, apply_antialiasing, apply_bloom, apply_contact_shadows,
    apply_exposure_compensation, apply_shadow_distance, apply_shadow_filtering,
    apply_shadow_quality, apply_sharpening, apply_vsync,
    cleanup_stale_background_motion_vector_state, reset_taa_history_on_camera_discontinuity,
}; // 保持画质插件的单一注册入口。
