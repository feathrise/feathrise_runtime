//! 数据驱动动态场景地形的 Bevy Solari 隔离实现与有界资源预算。
//!
//! 根程序默认 `raytracing` 仅注册 Solari；显式 `rtx_rendering` 与正式发布再加入 DLSS SR/RR。
//! `test_panel` 提供调试界面；DLSS诊断使用 `rtx_rendering,test_panel`，不再提供旧构建别名。

mod capability; // RenderDevice真实后端与Solari必需features探测。
mod components; // 私有代理与时间历史组件。
mod constants; // 动态场景单地形契约、预算与诊断路径。
mod custom_materials; // 应用显式登记自身扩展材质，不把游戏相机或透明算法纳入共享依赖。
mod denoising; // 可选DLSS RR能力门、相机附件所有权、reset同步和精确回退。
mod environment_lighting; // 把标准天空漫反射与镜面IBL补回跳过Bevy Deferred照明的Solari视图。
mod lifecycle; // 根事件驱动的发现、预热、激活、历史和完整回退。
mod mesh_adapter; // 分帧CPU捕获、有界后台转换与单项主线程采用。
mod raster_shadows; // 把P3光栅内容的标准方向光阴影有界合成回Solari最终颜色。
mod resources; // 公开请求/状态与私有生命周期所有权。
mod settings_bridge; // 正式RenderSettings唯一请求源与统一DLSS/Solari状态发布。
mod telemetry; // RenderApp绑定证据、GPU分段计时与关闭态宿主开销。
mod types; // 公开阶段/阻断原因与私有不可混用身份。

use feathrise_scene_core::residency::SceneResidencyAppExt;

use bevy::{
    camera::CameraUpdateSystems, // 历史判定读取本帧已经解析的物理目标尺寸。
    pbr::DefaultOpaqueRendererMethod, // 关闭态保持Forward；目标会话由有界兼容桥临时切Deferred并精确恢复。
    prelude::*,                       // App、Plugin、Update/PostUpdate与系统链。
    render::diagnostic::RenderDiagnosticsPlugin, // 取得Solari通道的GPU时间戳诊断。
    render::extract_resource::ExtractResourcePlugin, // 把固定材质目标表同步到RenderApp核对Deferred准备状态。
    transform::TransformSystems,                     // 历史判定读取本帧最终世界相机姿态。
};
use bevy_solari::SolariPlugins;
use feathrise_settings::GraphicsRuntimeLeaseSystems; // 动态RTT在Update命令落地后、租约恢复前取得兼容所有权。 // Bevy官方第一方实验性实时直接光、间接光与反射插件组。

use capability::initialize_capability;
use denoising::{
    install_solari_realtime_shaders, maintain_dlss_ray_reconstruction,
    restore_dlss_ray_reconstruction_on_rollback, sync_dlss_antialiasing_requirement,
    sync_dlss_ray_reconstruction_reset,
};
use lifecycle::{
    apply_pending_rollback, coordinate_lifecycle, discover_candidates,
    drive_gpu_preparation_and_warmup, maintain_deferred_auxiliary_camera_leases,
    maintain_deferred_raster_bridge, monitor_source_asset_changes, observe_dynamic_scene_target,
    publish_public_status, reset_solari_history_on_discontinuity,
    restore_deferred_raster_bridge, validate_active_sources,
};
use mesh_adapter::{capture_mesh_inputs, poll_and_adopt_mesh_results};
use resources::{
    DeferredAuxiliaryCameraLeaseState, DeferredRasterBridgeState, DeferredRasterBridgeTargets,
    MeshRevisionRegistry, RaytracingCapability, RaytracingRuntime, RaytracingSceneTracker,
};
use settings_bridge::{
    publish_dlss_raytracing_status, sync_clipmap_proxy_request,
    sync_raytracing_request_from_settings,
};
use telemetry::{
    DisabledHostTimer, begin_disabled_host_measurement, finish_disabled_host_measurement,
    install_render_telemetry, sync_render_telemetry,
}; // 生命周期首尾分别同步唯一请求与公开路径。

pub use resources::{
    RaytracingPrototypeRequest, RaytracingPrototypeStatus, RaytracingRuntimeStatus,
};
pub use custom_materials::{DeferredRasterMaterial, DeferredRasterMaterialSystems, register_deferred_raster_material};
pub use types::{
    DlssRayReconstructionDebugView, DlssRayReconstructionMode, RaytracingPrototypeBlocker,
    RaytracingPrototypeStage, RaytracingPrototypeTarget,
}; // F1只通过项目自有枚举表达RR模式和呈现请求，不依赖厂商SDK类型。

/// 安装场景地形隔离生命周期、资源预算检查和 Bevy 官方实时 Solari；不包含非实时路径追踪器。
pub struct RaytracingPlugin;

/// 旧 Prototype 插件名称继续作为完整兼容 re-export。
pub use RaytracingPlugin as RaytracingPrototypePlugin;
/// 正式阶段名称；旧 Prototype 名称继续兼容。
pub type RaytracingStage = RaytracingPrototypeStage;
/// 正式结构化阻断名称；旧 Prototype 名称继续兼容。
pub type RaytracingBlocker = RaytracingPrototypeBlocker;

impl Plugin for RaytracingPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(SolariPlugins); // 先注册官方实时光追与内嵌shader，下一行才能精确覆盖同名资源。
        install_solari_realtime_shaders(app); // 登记地表起点与直接光、间接光的复用门。
        raster_shadows::install_raster_shadow_compatibility(app); // 玩家与LOD0叶片仍走标准shadow map；Solari后、RR前消费其方向光可见度。
        environment_lighting::install_solari_environment_lighting(app); // 阴影兼容后、透明内容与RR前追加同一EnvironmentMapLight天空IBL。
        app.add_plugins((
            RenderDiagnosticsPlugin, // 提供Solari通道的GPU时间戳。
            ExtractResourcePlugin::<DeferredRasterBridgeTargets>::default(), // 最多256个材质ID进入RenderApp。
            ExtractResourcePlugin::<telemetry::TerrainGeometryTargets>::default(),
        ));
        app.insert_resource(DefaultOpaqueRendererMethod::forward()); // 必须紧跟Solari注册，确保关闭态与其他相机仍使用原Forward基线。

        app.init_resource::<RaytracingPrototypeRequest>()
            .init_resource::<custom_materials::CustomDeferredMaterials>()
            .init_resource::<feathrise_scene::ClipmapRaytracingRequest>()
            .init_resident_resource::<RaytracingPrototypeStatus>()
            .init_resource::<RaytracingCapability>()
            .init_resident_resource::<RaytracingSceneTracker>()
            .init_resource::<MeshRevisionRegistry>()
            .init_resident_resource::<RaytracingRuntime>()
            .init_resident_resource::<DeferredRasterBridgeState>()
            .init_resident_resource::<DeferredAuxiliaryCameraLeaseState>()
            .init_resident_resource::<DeferredRasterBridgeTargets>()
            .init_resource::<DisabledHostTimer>()
            .init_resource::<telemetry::TerrainGeometryTargets>()
            .init_resource::<feathrise_scene::ClipmapRaytracingInstanceRoute>()
            .add_resident_systems(
                Update,
                (
                    sync_raytracing_request_from_settings,
                    sync_clipmap_proxy_request,
                )
                    .chain()
                    .before(feathrise_scene::ClipmapRaytracingPublishSet),
            )
            .add_resident_systems(
                Update,
                (
                    begin_disabled_host_measurement,
                    monitor_source_asset_changes,
                    sync_render_telemetry,
                    observe_dynamic_scene_target, // Ready、未保存Mesh替换和GLB热重载都先换代目标，再推进生命周期。
                    coordinate_lifecycle,
                    discover_candidates,
                    capture_mesh_inputs,
                    poll_and_adopt_mesh_results,
                    drive_gpu_preparation_and_warmup,
                    maintain_dlss_ray_reconstruction,
                    sync_dlss_antialiasing_requirement, // RR活动时只暂停FXAA，Solari单采样与其他租约要求保持不变。
                    maintain_deferred_raster_bridge,
                    validate_active_sources,
                    restore_deferred_raster_bridge,
                    restore_dlss_ray_reconstruction_on_rollback,
                    apply_pending_rollback,
                    telemetry::publish_geometry_targets,
                    publish_public_status,
                    publish_dlss_raytracing_status, // 旧诊断结算后发布正式SR/Solari/RR组合与结构化原因。
                    finish_disabled_host_measurement,
                )
                    .chain()
                    .after(feathrise_scene::ClipmapRaytracingPublishSet)
                    .after(feathrise_scene::TerrainTopologySystems::Publish), // 正式三维地形先取得联合采用证明。
            )
            .add_resident_systems(
                PostUpdate,
                maintain_deferred_auxiliary_camera_leases
                    .before(GraphicsRuntimeLeaseSystems::RestoreReleased), // Update末才spawn/改target的RTT也在首次Render提取前完成租赁或释放。
            )
            .add_resident_systems(
                PostUpdate,
                (
                    reset_solari_history_on_discontinuity,
                    sync_dlss_ray_reconstruction_reset,
                )
                    .chain() // 正常历史系统先结算真实不连续原因，再保证RR与Solari同帧换代。
                    .after(TransformSystems::Propagate)
                    .after(CameraUpdateSystems), // 只比较最终世界姿态、投影和物理目标尺寸。
            );
    }

    fn finish(&self, app: &mut App) {
        initialize_capability(app); // 使用DefaultPlugins完成后建立的真实RenderDevice，不猜环境变量。
        install_render_telemetry(app); // 在完整Render帧末发布固定小型绑定证据。
    }
}
