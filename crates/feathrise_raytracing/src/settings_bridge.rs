//! 正式 `RenderSettings` 请求到 Solari 生命周期的单向桥，以及统一 DLSS 状态发布。

use bevy::prelude::*; // ECS 资源与系统参数。
use feathrise_settings::{
    DlssMode, DlssRuntimeBlocker, DlssRuntimeStatus, RenderSettings, RaytracingSceneCoverage,
}; // 唯一持久请求源与跨设置/光追公开状态。

use crate::{
    resources::{RaytracingPrototypeRequest, RaytracingPrototypeStatus, RaytracingRuntime},
    types::{
        DlssRayReconstructionMode, RaytracingPrototypeBlocker, RaytracingPrototypeStage,
        RaytracingPrototypeTarget,
    },
}; // 内部兼容请求、生命周期事实与旧公开诊断。

/// 每帧先把正式画质设置镜像到内部兼容请求，F1 不再维护第二份光追或 RR 状态。
pub(crate) fn sync_raytracing_request_from_settings(
    settings: Res<RenderSettings>,         // 光追、DLSS与RR的唯一玩家请求源。
    dlss_status: Res<DlssRuntimeStatus>, // Auto 已按当前输出解析后的明确模式。
    mut request: ResMut<RaytracingPrototypeRequest>, // 只作为既有生命周期的内部兼容输入。
) {
    let desired_enabled = settings.graphics.raytracing_enabled; // Solari 请求直接跟随持久设置。
    let desired_rr_mode = if settings.graphics.raytracing_enabled
        && settings.graphics.ray_reconstruction_enabled
        && settings.graphics.dlss_mode.is_enabled()
    {
        rr_mode_from_dlss(dlss_status.effective_mode) // Auto 只使用设置模块已经解析的明确结果。
    } else {
        DlssRayReconstructionMode::Off // 光追或DLSS依赖关闭时RR退出，但偏好仍保存在设置中。
    };
    if request.enabled != desired_enabled {
        request.enabled = desired_enabled; // 真实变化才触发生命周期开关变更检测。
    }
    if request.dlss_rr_mode != desired_rr_mode {
        request.dlss_rr_mode = desired_rr_mode; // 真实模式变化才重建RR上下文与历史。
    }
}

/// 不支持 Solari 的设备和关闭态均不启动场景高度准备，避免无用后台工作。
pub(crate) fn sync_clipmap_proxy_request(
    request: Res<RaytracingPrototypeRequest>,
    capability: Res<crate::resources::RaytracingCapability>,
    mut clipmap: ResMut<feathrise_scene::ClipmapRaytracingRequest>,
) {
    let enabled = request.enabled && capability.available;
    if clipmap.enabled != enabled {
        clipmap.enabled = enabled;
    }
}

/// 把设置公开模式映射到旧生命周期兼容枚举；Off 与尚未解析的 Auto 都保持关闭。
const fn rr_mode_from_dlss(mode: DlssMode) -> DlssRayReconstructionMode {
    match mode {
        DlssMode::Off | DlssMode::Auto => DlssRayReconstructionMode::Off, // Auto不得直接传入厂商封装。
        DlssMode::Quality => DlssRayReconstructionMode::Quality,          // 质量档一一映射。
        DlssMode::Balanced => DlssRayReconstructionMode::Balanced,        // 平衡档一一映射。
        DlssMode::Performance => DlssRayReconstructionMode::Performance,  // 性能档一一映射。
        DlssMode::UltraPerformance => DlssRayReconstructionMode::UltraPerformance, // 超性能档一一映射。
        DlssMode::Dlaa => DlssRayReconstructionMode::Dlaa, // DLAA保持原生内部尺寸。
    }
}

/// 在旧公开状态完成结算后，把实际 Solari/RR 路径与结构化原因合并到画质页状态。
pub(crate) fn publish_dlss_raytracing_status(
    settings: Res<RenderSettings>,                // 发布本帧唯一请求依赖。
    raytracing: Res<RaytracingPrototypeStatus>, // 已由生命周期发布的公开能力、阶段与尺寸。
    runtime: Res<RaytracingRuntime>,            // 读取本帧真实Solari/RR组件所有权。
    mut dlss: ResMut<DlssRuntimeStatus>,        // 与独立SR系统共同组成最终正式路径。
) {
    let requested = settings.graphics.raytracing_enabled; // 玩家是否希望当前受支持场景进入Solari。
    let target = raytracing.target(); // 当前生命周期锁定的既有内容白名单。
    let solari_effective = requested
        && runtime.solari_inserted
        && matches!(
            runtime.stage,
            RaytracingPrototypeStage::WarmingSolari | RaytracingPrototypeStage::Active
        ); // 只把真实相机组件已挂接的预热或活动阶段视为Solari路径。
    let rr_effective = solari_effective
        && runtime.dlss_rr_active
        && raytracing.dlss_rr_render_resolution().is_some(); // 只有 RenderApp 成功创建上下文并回传内部尺寸才宣称 RR 实际生效。

    dlss.raytracing_requested = requested; // 设置页即时反映玩家开关。
    dlss.raytracing_effective = solari_effective; // 能力或场景不足时保持false并由SR回退。
    dlss.ray_reconstruction_requested = settings.graphics.ray_reconstruction_enabled; // 灰显不清除偏好。
    dlss.ray_reconstruction_supported = runtime.dlss_rr_supported; // 官方支持资源是唯一事实。
    dlss.ray_reconstruction_effective = rr_effective; // 与相机真实RR组件所有权一致。
    dlss.scene_coverage = if !requested {
        RaytracingSceneCoverage::RasterOnly // 光追关闭时不把当前场景解释为错误。
    } else if target == RaytracingPrototypeTarget::None {
        RaytracingSceneCoverage::Unsupported // 没有既有白名单目标时明确回退光栅。
    } else {
        RaytracingSceneCoverage::Partial // 当前Solari只覆盖既有动态地形边界。
    };

    if rr_effective {
        dlss.effective_mode = dlss_mode_from_rr(raytracing.dlss_rr_mode()); // RR成为实际重建器后发布明确模式。
        dlss.output_resolution = raytracing
            .dlss_rr_output_resolution()
            .unwrap_or(dlss.output_resolution); // RenderApp首帧未回传时保留相机主世界输出尺寸。
        dlss.internal_resolution = raytracing
            .dlss_rr_render_resolution()
            .unwrap_or(UVec2::ZERO); // 只报告RenderApp真实内部尺寸，不做比例猜测。
    } else if !dlss.path.uses_dlss() {
        dlss.internal_resolution = UVec2::ZERO; // RR/SR均未生效时清除旧内部尺寸。
    }

    dlss.raytracing_blocker = raytracing_blocker(
        requested,
        settings.graphics.dlss_mode,
        settings.graphics.ray_reconstruction_enabled,
        raytracing.as_ref(),
        solari_effective,
        rr_effective,
    ); // 场景、后端、设备、切换或RR能力主因单独归属光追域。
    dlss.rebuild_blockers(); // PostUpdate独立SR系统会在最终路径结算后再次无旧值重建。
}

/// 把旧RR兼容枚举恢复为设置层公开明确模式。
const fn dlss_mode_from_rr(mode: DlssRayReconstructionMode) -> DlssMode {
    match mode {
        DlssRayReconstructionMode::Off => DlssMode::Off, // 无RR组件。
        DlssRayReconstructionMode::Dlaa => DlssMode::Dlaa, // 原生时间重建。
        DlssRayReconstructionMode::Quality => DlssMode::Quality, // 质量档。
        DlssRayReconstructionMode::Balanced => DlssMode::Balanced, // 平衡档。
        DlssRayReconstructionMode::Performance => DlssMode::Performance, // 性能档。
        DlssRayReconstructionMode::UltraPerformance => DlssMode::UltraPerformance, // 超性能档。
    }
}

/// 选择当前最有解释力的 Solari/RR 结构化阻断原因。
fn raytracing_blocker(
    requested: bool,                    // 玩家是否请求Solari。
    dlss_mode: DlssMode,                // DLSS依赖是否开启。
    rr_requested: bool,                 // 玩家是否保留RR偏好。
    status: &RaytracingPrototypeStatus, // 旧生命周期本帧公开状态。
    solari_effective: bool,             // Solari相机组件是否真实存在。
    rr_effective: bool,                 // RR相机组件是否真实存在。
) -> Option<DlssRuntimeBlocker> {
    if !requested {
        return None; // 主动关闭光追不是阻断。
    }
    if status.target() == RaytracingPrototypeTarget::None {
        return Some(DlssRuntimeBlocker::SceneUnsupported); // 当前场景没有既有内容目标。
    }
    if let Some(blocker) = status.blocker() {
        return Some(match blocker {
            RaytracingPrototypeBlocker::BackendNotVulkan { backend } => {
                DlssRuntimeBlocker::BackendUnsupported {
                    actual: backend.clone(), // 保留真实后端名称供玩家处理启动方式。
                }
            }
            RaytracingPrototypeBlocker::MissingDeviceFeatures { features } => {
                DlssRuntimeBlocker::DeviceUnsupported {
                    detail: features.clone(), // 保留缺失wgpu能力集合。
                }
            }
            RaytracingPrototypeBlocker::WaitingForSupportedScene => {
                DlssRuntimeBlocker::SceneUnsupported // 与目标None使用同一正式提示。
            }
            _ => DlssRuntimeBlocker::RuntimeFailure {
                detail: blocker.display_text(), // 资产、预算、任务或回退原因保持可审计文本。
            },
        });
    }
    if !solari_effective {
        return Some(DlssRuntimeBlocker::Transitioning); // 候选、适配、GPU准备与预热属于安全切换。
    }
    if rr_requested && dlss_mode.is_enabled() && !rr_effective && !status.dlss_rr_supported() {
        return Some(DlssRuntimeBlocker::NgxUnavailable {
            detail: "当前设备、驱动或 nvngx_dlssd.dll 不支持 Ray Reconstruction".to_string(), // SR继续作为正式回退。
        });
    }
    if rr_requested && dlss_mode.is_enabled() && !rr_effective && status.dlss_rr_runtime_failed() {
        return Some(DlssRuntimeBlocker::RuntimeFailure {
            detail: "Ray Reconstruction 上下文创建失败；同一模式/尺寸已停止重试并回退 DLSS SR"
                .to_string(),
        });
    }
    if rr_requested && dlss_mode.is_enabled() && !rr_effective {
        return Some(DlssRuntimeBlocker::Transitioning); // SR退出、RR挂接或RenderApp首个上下文准备帧。
    }
    None // Solari有效且RR未请求、已生效或正在由SR管理器标记交接。
}

#[cfg(test)]
mod tests {
    use super::*; // 纯模式映射覆盖完整组合，不初始化GPU。

    #[test]
    fn formal_graphics_settings_are_the_only_rr_request_source() {
        let mut app = App::new(); // 只验证主世界单向桥，不创建窗口或RenderApp。
        app.init_resource::<RenderSettings>()
            .init_resource::<DlssRuntimeStatus>()
            .init_resource::<RaytracingPrototypeRequest>()
            .add_systems(Update, sync_raytracing_request_from_settings);
        {
            let mut settings = app.world_mut().resource_mut::<RenderSettings>();
            settings.graphics.raytracing_enabled = true;
            settings.graphics.ray_reconstruction_enabled = true;
            settings.graphics.set_dlss_mode(DlssMode::Auto); // 持久层保留Auto并关闭冲突FSR，桥只消费已解析模式。
        }
        app.world_mut()
            .resource_mut::<DlssRuntimeStatus>()
            .effective_mode = DlssMode::Balanced;

        app.update();
        let request = app.world().resource::<RaytracingPrototypeRequest>();
        assert!(request.enabled);
        assert_eq!(request.dlss_rr_mode, DlssRayReconstructionMode::Balanced); // 内部兼容请求不得使用旧F1状态或直传Auto。

        {
            let mut settings = app.world_mut().resource_mut::<RenderSettings>();
            settings.graphics.raytracing_enabled = false;
            settings.graphics.set_dlss_mode(DlssMode::Off);
            assert!(settings.graphics.ray_reconstruction_enabled); // 依赖灰显只停止生效，不清除玩家RR偏好。
        }
        app.update();
        let request = app.world().resource::<RaytracingPrototypeRequest>();
        assert!(!request.enabled);
        assert_eq!(request.dlss_rr_mode, DlssRayReconstructionMode::Off);
        assert!(
            app.world()
                .resource::<RenderSettings>()
                .graphics
                .ray_reconstruction_enabled
        ); // 正式设置仍是唯一持久事实。
    }

    #[test]
    fn every_explicit_dlss_mode_scenes_to_the_same_rr_mode() {
        for mode in [
            DlssMode::Quality,
            DlssMode::Balanced,
            DlssMode::Performance,
            DlssMode::UltraPerformance,
            DlssMode::Dlaa,
        ] {
            assert_eq!(dlss_mode_from_rr(rr_mode_from_dlss(mode)), mode); // 五个明确模式必须双向无损。
        }
        assert_eq!(
            rr_mode_from_dlss(DlssMode::Auto),
            DlssRayReconstructionMode::Off
        ); // Auto不得直接进入封装。
    }
}
