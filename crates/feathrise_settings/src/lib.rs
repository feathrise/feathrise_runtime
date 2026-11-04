// 共享渲染请求与运行状态；应用聚合设置和配置文件由游戏或编辑器各自拥有。

mod components; // 组件：灯光与窗口相机作者状态备份
mod graphics; // 功能域：把画质设置应用到窗口、相机和灯光
mod resources; // 只承载共享运行输入和后端状态。
mod types; // 渲染质量的中性值类型。

use bevy::{
    camera::CameraUpdateSystems, // 渲染比例读取本帧已解析的窗口物理目标尺寸。
    light::{DirectionalLightShadowMap, PointLightShadowMap, SimulationLightSystems}, // 全局阴影资源与 Bevy 光照阶段排序集。
    prelude::*,                                 // App、Plugin、PostUpdate 与系统排序配置。
    render::{Render, RenderApp, RenderSystems}, // 画质插件仍清理渲染世界背景运动矢量派生状态。
    transform::TransformSystems,                // TAA 瞬移判定必须读取本帧已经传播的最终相机姿态。
};

/// 将本帧 UI 画质修改应用到 Bevy 渲染状态，并持续接管后续生成的相机和灯光。
pub struct GraphicsSettingsPlugin;

impl Plugin for GraphicsSettingsPlugin {
    fn build(&self, app: &mut App) {
        graphics::configure_render_scale(app); // 先注册提取与 RenderApp 管线，主世界系统随后只维护强类型视图组件。
        graphics::configure_texture_filtering(app); // 分批接管 PBR 线性纹理采样器，并在材质准备前完成绑定刷新。
        graphics::configure_dlss(app); // 初始化结构化状态，并在正式特性下维护独立 SR 所有权。
        graphics::configure_fsr3(app); // 初始化FSR状态，并在候选特性下维护互斥相机与可回退租约。
        app.init_resource::<RenderSettings>()
            .init_resource::<SceneDisplaySettings>()
            .init_resource::<EnvironmentPreviewSettings>() // 独立使用设置 crate 时也具备安全默认值。
            .init_resource::<DlssRuntimeStatus>() // UI 与可选光追插件始终可读取安全回退状态。
            .init_resource::<Fsr3RuntimeStatus>() // 便携构建也能显示明确的FSR构建阻断。
            .init_resource::<graphics::GraphicsRuntimeLeaseRegistry>() // 可选渲染功能通过统一声明覆盖，不直接改玩家设置。
            .init_resource::<DirectionalLightShadowMap>() // 方向光/聚光灯默认保持 2048。
            .init_resource::<PointLightShadowMap>() // 点光默认保持 1024。
            .add_systems(
                PostUpdate,
                (
                    graphics::restore_released_graphics_runtime_leases, // 释放帧先恢复非设置附件作者状态。
                    ApplyDeferred, // 最新玩家画质系统必须看到已经落地的作者附件。
                )
                    .chain()
                    .in_set(graphics::GraphicsRuntimeLeaseSystems::RestoreReleased)
                    .before(graphics::apply_antialiasing)
                    .before(graphics::apply_bloom)
                    .before(graphics::apply_render_scale),
            )
            .add_systems(
                PostUpdate,
                (
                    graphics::apply_vsync,                 // 应用主窗口垂直同步。
                    graphics::apply_shadow_filtering,      // 应用窗口目标 3D 相机阴影滤波。
                    graphics::apply_bloom,                 // 应用 Bloom 并管理 HDR 所有权。
                    graphics::apply_exposure_compensation, // 相对作者基线应用窗口 3D 曝光补偿。
                    graphics::apply_sharpening
                        .after(graphics::DlssRuntimeSystems::MaintainSuperResolution), // SR/RR目标标记落地后应用普通或DLSS专用CAS。
                    graphics::apply_render_scale.after(CameraUpdateSystems), // 相机目标尺寸解析后接管完整窗口 3D 视图。
                ),
            )
            .add_systems(
                PostUpdate,
                (
                    graphics::apply_antialiasing, // 应用窗口目标 3D 相机 FXAA/MSAA/TAA 互斥状态。
                    ApplyDeferred, // 让新增 TAA 及其 Bevy required 组件在历史系统读取前同帧落地。
                    graphics::apply_ambient_occlusion, // 在最终采样数确定后接管玩法窗口相机 SSAO。
                    ApplyDeferred, // 让 SSAO 的 Depth/Normal required 预通道在本帧提取前落地。
                    graphics::apply_contact_shadows, // 在 TAA/SSAO 后接管玩法相机接触阴影与共享深度。
                    ApplyDeferred, // 让接触阴影 required 深度和恢复命令在本帧提取前落地。
                    graphics::reset_taa_history_on_camera_discontinuity, // 传送、镜头切换或投影变化时清空历史。
                )
                    .chain()
                    .after(TransformSystems::Propagate), // 使用相机跟随和层级传播完成后的最终世界姿态。
            )
            .add_systems(
                PostUpdate,
                (
                    graphics::apply_shadow_quality, // 在 Bevy 构建级联前应用阴影贴图与灯光资格。
                    graphics::apply_shadow_distance, // 在 Bevy 构建级联前统一所有方向光的距离与数量。
                )
                    .before(SimulationLightSystems::UpdateDirectionalLightCascades), // 禁止同帧混用旧级联派生数据与新配置。
            )
            .add_systems(
                PostUpdate,
                graphics::apply_graphics_runtime_leases
                    .in_set(graphics::GraphicsRuntimeLeaseSystems::ApplyActive)
                    .after(graphics::reset_taa_history_on_camera_discontinuity)
                    .after(graphics::apply_bloom)
                    .after(graphics::apply_render_scale)
                    .before(SimulationLightSystems::UpdateDirectionalLightCascades), // 普通画质收敛后补齐租约附件，稳定帧不重复写入。
            );

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(
                Render,
                (
                    graphics::cleanup_stale_background_motion_vector_state, // TAA 关闭后移除 Bevy 0.19.1 仍未自动撤销的背景运动矢量派生状态。
                    ApplyDeferred, // 预通道图执行前立即落地清理，不能留到 Render 调度末尾。
                )
                    .chain()
                    .in_set(RenderSystems::PrepareViews), // 纹理附件状态已提取，材质排队与 GPU 绑定准备尚未开始。
            );
        }
    }
}

pub use components::{DlssPostProcessTarget, Fsr3PostProcessTarget}; // 厂商重建器通过标记声明普通CAS所有权。
pub use graphics::{
    DlssRuntimeSystems, GraphicsRuntimeLeaseId, GraphicsRuntimeLeaseRegistry,
    GraphicsRuntimeLeaseSystems, GraphicsRuntimeRequirements,
}; // 可选渲染crate只能通过公开租约声明临时运行时要求。
pub use resources::{
    DEFAULT_EXPOSURE_COMPENSATION, DisplaySettingsInputSet, DlssRuntimeBlocker, DlssRuntimePath,
    DlssRuntimeStatus, EnvironmentPreviewSettings, Fsr3RuntimeBlocker, Fsr3RuntimeStatus,
    MAX_EXPOSURE_COMPENSATION, MIN_EXPOSURE_COMPENSATION, RaytracingSceneCoverage, RenderSettings,
    RuntimeSettingsSyncSet, SceneDisplaySettings, StartupDisplayPreferences, StartupWindowPolicy,
    sanitize_exposure_compensation,
};
pub use types::{
    AirWallDisplayMode, AmbientOcclusionQuality, AntiAliasingMode, BloomQuality,
    ContactShadowQuality, DlssMode, EffectsQuality, FrameGenerationMode, FsrMode,
    GraphicsQualityPreset, GraphicsSettings, MeshLodSettings, RenderScaleQuality,
    SceneDetailQuality, ShadowDistanceQuality, ShadowFilteringQuality, ShadowQuality,
    SharpeningQuality, TextureFilteringQuality, VegetationQuality, VolumetricFogQuality,
}; // 导出设置 UI、持久化与运行时共同使用的公开类型。
