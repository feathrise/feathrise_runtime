// 游戏设置 — 画质运行时系统分域入口。

mod ambient_occlusion; // 玩法窗口相机 SSAO 环境光遮蔽与作者状态恢复。
#[cfg(test)]
mod ambient_occlusion_integration_tests; // 锁定 TAA 与 SSAO 双向切换的共享预通道所有权。
mod camera; // 窗口 3D 相机的抗锯齿、阴影滤波与 Bloom。
mod contact_shadows; // 玩法窗口相机接触阴影与共享深度作者状态恢复。
#[cfg(test)]
mod contact_shadows_integration_tests; // 锁定 TAA、SSAO 与接触阴影的三方深度所有权。
mod exposure; // 窗口 3D 相机的玩家曝光补偿与作者状态恢复。
mod motion_vectors_pipeline; // 运行时关闭 TAA 后清理 Bevy 0.19.1 仍遗留的背景运动矢量派生状态。
mod shadow; // 全局阴影贴图、灯光资格与方向光级联。
mod sharpening; // 窗口 3D 相机的 CAS 自适应锐化与作者状态恢复。
mod window; // 主窗口垂直同步。

pub(crate) use ambient_occlusion::apply_ambient_occlusion; // 玩法相机 SSAO 系统注册入口。
pub(crate) use camera::{
    apply_antialiasing, apply_bloom, apply_shadow_filtering,
    reset_taa_history_on_camera_discontinuity,
}; // 相机系统注册入口。
pub(crate) use contact_shadows::apply_contact_shadows; // 玩法相机接触阴影系统注册入口。
pub(crate) use exposure::apply_exposure_compensation; // 窗口 3D 曝光补偿系统注册入口。
pub(crate) use motion_vectors_pipeline::cleanup_stale_background_motion_vector_state; // RenderApp 预通道兼容清理入口。
pub(crate) use shadow::{apply_shadow_distance, apply_shadow_quality}; // 阴影档位应用入口。
pub(crate) use sharpening::apply_sharpening; // CAS 系统注册入口。
pub(crate) use window::apply_vsync; // 窗口系统注册入口。
