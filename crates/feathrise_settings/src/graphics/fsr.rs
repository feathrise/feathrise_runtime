//! FSR 3超分候选的相机接管、厂商互斥、运行状态与安全回退。

// 候选构建在相机系统之后读取最终物理尺寸。
#[cfg(feature = "fsr3_upscaling")]
use bevy::camera::CameraUpdateSystems;
use bevy::prelude::*; // App、相机状态与安全回退系统。

use crate::{Fsr3RuntimeBlocker, Fsr3RuntimeStatus, FsrMode, RenderSettings}; // 唯一请求与公开状态。

/// 注册便携构建状态发布器，或候选构建的完整相机所有权管理器。
pub(crate) fn configure_fsr3(app: &mut App) {
    #[cfg(feature = "fsr3_upscaling")]
    app.init_resource::<crate::resources::Fsr3LeaseIndex>()
        .add_systems(
            PostUpdate,
            (maintain_fsr3_upscaling, ApplyDeferred)
                .chain() // FSR组件、所有权与恢复必须在Render Extract前同帧落地。
                .after(CameraUpdateSystems) // 窗口缩放必须读取本帧已经解析的物理目标尺寸。
                .after(super::DlssRuntimeSystems::MaintainSuperResolution) // 最终相机状态观察已结算的DLSS请求。
                .before(super::GraphicsRuntimeLeaseSystems::RestoreReleased), // 先维护租约声明，再统一恢复和应用。
        );
    #[cfg(not(feature = "fsr3_upscaling"))]
    app.add_systems(
        PostUpdate,
        publish_disabled_fsr3_status
            .after(super::DlssRuntimeSystems::MaintainSuperResolution)
            .before(super::GraphicsRuntimeLeaseSystems::RestoreReleased),
    ); // 默认构建只发布明确阻断，不接触相机或GPU资源。
}

/// 无FSR候选特性的构建保留玩家请求并明确报告构建阻断。
#[cfg(not(feature = "fsr3_upscaling"))]
fn publish_disabled_fsr3_status(
    settings: Res<RenderSettings>,           // 读取当前持久请求但不改写它。
    mut status: ResMut<Fsr3RuntimeStatus>, // UI始终拥有结构化状态。
) {
    status.requested_mode = settings.graphics.fsr_mode;
    status.effective_mode = FsrMode::Off;
    status.target_camera_count = 0;
    status.output_resolution = UVec2::ZERO;
    status.internal_resolution = UVec2::ZERO;
    status.supported = false;
    status.reactive_mask_available = false;
    status.transparency_and_composition_mask_available = false;
    status.blocker = settings
        .graphics
        .fsr_mode
        .is_enabled()
        .then_some(Fsr3RuntimeBlocker::BuildDisabled); // 关闭状态不显示无关警告。
}

#[cfg(feature = "fsr3_upscaling")]
use crate::{
    components::{Fsr3PostProcessTarget, Fsr3UpscalingOwnership},
    graphics::{GraphicsRuntimeLeaseRegistry, GraphicsRuntimeRequirements},
    resources::Fsr3LeaseIndex,
}; // 候选构建的相机作者快照、租约和销毁索引。
#[cfg(feature = "fsr3_upscaling")]
use bevy::{
    camera::RenderTarget,                      // 完整窗口资格过滤。
    render::camera::{MipBias, TemporalJitter}, // 作者纹理LOD偏移与抖动快照。
};
#[cfg(feature = "fsr3_upscaling")]
use feathrise_fsr3::{Fsr3BackendSupported, Fsr3Quality, Fsr3RuntimeTelemetry, Fsr3Upscaling}; // 真实运行组件、后端资格和RenderApp低频结果。

/// FSR管理器读取的每台窗口三维相机完整数据。
#[cfg(feature = "fsr3_upscaling")]
type Fsr3CameraData<'a> = (
    Entity,
    &'a Camera,
    &'a Projection,
    &'a RenderTarget,
    Option<&'a Fsr3Upscaling>,
    Option<&'a TemporalJitter>,
    Option<&'a MipBias>,
    Option<&'a Fsr3UpscalingOwnership>,
);

/// 幂等维护全部合格窗口透视相机的FSR组件、作者快照和图形租约。
#[cfg(feature = "fsr3_upscaling")]
#[allow(clippy::too_many_arguments)] // 支持、遥测、租约、索引和相机分别承担独立所有权职责。
fn maintain_fsr3_upscaling(
    mut commands: Commands, // 只在挂接、切档、缩放、失败或恢复时提交组件命令。
    settings: Res<RenderSettings>, // 唯一持久请求和厂商互斥事实。
    support: Option<Res<Fsr3BackendSupported>>, // Vulkan资格；管线只在真实请求后延迟创建。
    telemetry: Option<Res<Fsr3RuntimeTelemetry>>, // RenderApp实际上下文尺寸和失败。
    mut leases: ResMut<GraphicsRuntimeLeaseRegistry>, // 普通画质可回退临时所有权。
    mut lease_index: ResMut<Fsr3LeaseIndex>, // 相机直接销毁时仍能回收票据。
    cameras: Query<Fsr3CameraData<'_>, With<Camera3d>>, // 只枚举有限三维相机集合。
    mut status: ResMut<Fsr3RuntimeStatus>, // 发布请求、实际模式、尺寸和单一阻断。
) {
    lease_index.leases.retain(|camera, lease| {
        if cameras.get(*camera).is_ok() {
            true // 仍存在的相机由下面正常路径维护。
        } else {
            leases.release(*lease); // 相机或Camera3d直接销毁时立即回收遗留租约。
            false
        }
    });

    let requested = settings.graphics.fsr_mode; // 一帧只读取一次模式。
    let desired_quality = fsr_quality(requested); // Off没有运行时质量档。
    let backend_supported = support.is_some();
    let conflicts_with_dlss =
        settings.graphics.dlss_mode.is_enabled() || settings.graphics.ray_reconstruction_enabled; // 手工JSON冲突优先保持既有DLSS/RR请求。
    let snapshot = telemetry
        .as_deref()
        .map(Fsr3RuntimeTelemetry::snapshot)
        .unwrap_or_default(); // 插件未安装或尚未完成时使用空快照。
    let pipeline_failure = (!snapshot.pipeline_ready && snapshot.status.quality.is_none())
        .then(|| snapshot.status.failure.clone())
        .flatten(); // 全局失败没有质量/尺寸键，且只在首次真实请求后出现。
    status.requested_mode = requested;
    status.effective_mode = FsrMode::Off;
    status.target_camera_count = 0;
    status.output_resolution = UVec2::ZERO;
    status.internal_resolution = UVec2::ZERO;
    status.supported = backend_supported && snapshot.pipeline_ready;
    status.reactive_mask_available = snapshot.reactive_mask_available;
    status.transparency_and_composition_mask_available =
        snapshot.transparency_and_composition_mask_available;

    let mut eligible_count = 0_u32;
    let mut active_count = 0_u32;
    let mut first_output = UVec2::ZERO;
    let mut runtime_failure = None::<String>;

    for (entity, camera, projection, target, current_fsr, jitter, mip_bias, ownership) in &cameras {
        let eligible = fsr_camera_is_eligible(camera, projection, target);
        let output = camera.physical_target_size().unwrap_or(UVec2::ZERO);
        if eligible && fsr_output_is_supported(output) {
            eligible_count += 1;
            if first_output == UVec2::ZERO {
                first_output = output; // 查询顺序第一台窗口相机作为公开主尺寸。
            }
        }
        let matching_failure = desired_quality.and_then(|quality| {
            (snapshot.status.quality == Some(quality)
                && snapshot.status.output_resolution == output)
                .then(|| snapshot.status.failure.clone())
                .flatten()
        }); // 失败只锁定同一质量和输出尺寸。
        if runtime_failure.is_none() {
            runtime_failure = matching_failure.clone(); // 多相机只公开第一个稳定主因。
        }
        let should_attach = eligible
            && fsr_output_is_supported(output)
            && requested.is_enabled()
            && backend_supported
            && !conflicts_with_dlss
            && pipeline_failure.is_none()
            && matching_failure.is_none();

        if let Some(ownership) = ownership {
            if !should_attach {
                restore_fsr3(&mut commands, entity, current_fsr, ownership); // 先归还作者组件与Jitter/Mip。
                leases.release(ownership.lease); // 同帧归还普通画质所有权。
                lease_index.leases.remove(&entity);
                continue;
            }
            let quality = desired_quality.expect("已过滤关闭模式，FSR质量必须存在");
            let desired_sharpening =
                f32::from(settings.graphics.fsr_sharpening_percent.min(100)) / 100.0;
            let mode_changed = ownership.applied_mode != requested;
            let output_changed = ownership.output_resolution != output;
            let component_changed = current_fsr.is_none_or(|value| {
                value.quality != quality
                    || (value.sharpening - desired_sharpening).abs() > f32::EPSILON
                    || mode_changed
                    || output_changed
            });
            if component_changed {
                commands.entity(entity).insert(Fsr3Upscaling {
                    quality,
                    sharpening: desired_sharpening,
                    reset: current_fsr.is_none_or(|value| value.reset)
                        || mode_changed
                        || output_changed,
                }); // 锐化变化不清历史，模式或窗口变化只reset一次。
                commands.entity(entity).insert(Fsr3UpscalingOwnership {
                    applied_mode: requested,
                    output_resolution: output,
                    ..ownership.clone()
                }); // 保留首次作者快照和同一租约票据。
            }
        } else if should_attach {
            if let Some(stale) = lease_index.leases.remove(&entity) {
                leases.release(stale); // 防御命令中断留下的孤立索引。
            }
            let quality = desired_quality.expect("已过滤关闭模式，FSR质量必须存在");
            let lease = leases.acquire(GraphicsRuntimeRequirements::fsr3_super_resolution(entity));
            lease_index.leases.insert(entity, lease);
            commands.entity(entity).insert((
                Fsr3Upscaling {
                    quality,
                    sharpening: f32::from(settings.graphics.fsr_sharpening_percent.min(100))
                        / 100.0,
                    reset: true,
                },
                Fsr3UpscalingOwnership {
                    lease,
                    authored_fsr: current_fsr.cloned(),
                    temporal_jitter: jitter.cloned(),
                    mip_bias: mip_bias.cloned(),
                    applied_mode: requested,
                    output_resolution: output,
                },
                Fsr3PostProcessTarget,
            )); // 同一命令批取得运行组件、作者快照和CAS互斥标记。
        }

        let telemetry_matches = desired_quality.is_some_and(|quality| {
            snapshot.status.quality == Some(quality)
                && snapshot.status.output_resolution == output
                && snapshot.status.internal_resolution.x > 0
                && snapshot.status.internal_resolution.y > 0
                && snapshot.status.failure.is_none()
        });
        if should_attach && telemetry_matches {
            active_count += 1; // 只有RenderApp真实上下文匹配才宣称生效。
            if status.effective_mode == FsrMode::Off {
                status.effective_mode = requested;
                status.internal_resolution = snapshot.status.internal_resolution;
            }
        }
    }

    status.target_camera_count = active_count;
    status.output_resolution = first_output;
    status.blocker = if !requested.is_enabled() {
        None // 关闭态不显示无关能力或相机警告。
    } else if conflicts_with_dlss {
        Some(Fsr3RuntimeBlocker::ConflictsWithDlss)
    } else if !backend_supported {
        Some(Fsr3RuntimeBlocker::BackendUnsupported {
            detail: snapshot
                .status
                .failure
                .clone()
                .unwrap_or_else(|| "Vulkan后端或FSR计算管线未就绪".into()),
        })
    } else if let Some(detail) = pipeline_failure {
        Some(Fsr3RuntimeBlocker::BackendUnsupported { detail })
    } else if eligible_count == 0 {
        Some(Fsr3RuntimeBlocker::NoEligibleCamera)
    } else if let Some(detail) = runtime_failure {
        if detail.contains("缺少")
            || detail.contains("需要 Rgba")
            || detail.contains("需要 Depth")
            || detail.contains("需要 Rg16")
        {
            Some(Fsr3RuntimeBlocker::MissingInputs)
        } else {
            Some(Fsr3RuntimeBlocker::RuntimeFailure { detail })
        }
    } else {
        None // 首帧上下文准备期间有效模式仍为Off，但没有虚构失败。
    };
    if backend_supported
        && (!requested.is_enabled() || conflicts_with_dlss || eligible_count == 0)
        && snapshot.status.quality.is_some()
        && let Some(telemetry) = telemetry.as_deref()
    {
        telemetry.clear_view_status(); // 会话结束后同模式重开必须等待新上下文，不复用陈旧成功或失败。
    }
}

/// 判断相机是否属于FSR候选接管范围，不依赖显卡品牌或分辨率档。
#[cfg(feature = "fsr3_upscaling")]
fn fsr_camera_is_eligible(camera: &Camera, projection: &Projection, target: &RenderTarget) -> bool {
    let projection_supported = matches!(
        projection,
        Projection::Perspective(value)
            if value.near.is_finite()
                && value.near >= 0.075
                && value.fov.is_finite()
                && value.fov > 0.0
                && value.fov <= std::f32::consts::PI
    ); // 在降低主通道分辨率前排除上游确定会拒绝的相机参数。
    camera.is_active
        && camera.viewport.is_none()
        && camera.sub_camera_view.is_none()
        && projection_supported
        && matches!(target, RenderTarget::Window(_)) // 纹理、小地图和角色预览相机完全隔离。
}

/// 上游时间历史至少需要 `2×2` 输入；更小窗口保持原生路径等待恢复。
#[cfg(feature = "fsr3_upscaling")]
const fn fsr_output_is_supported(output: UVec2) -> bool {
    output.x >= 2 && output.y >= 2
}

/// 把持久模式映射到隔离运行时固定质量档。
#[cfg(feature = "fsr3_upscaling")]
const fn fsr_quality(mode: FsrMode) -> Option<Fsr3Quality> {
    match mode {
        FsrMode::Off => None,
        FsrMode::Quality => Some(Fsr3Quality::Quality),
        FsrMode::Balanced => Some(Fsr3Quality::Balanced),
        FsrMode::Performance => Some(Fsr3Quality::Performance),
        FsrMode::UltraPerformance => Some(Fsr3Quality::UltraPerformance),
    }
}

/// 关闭、冲突、能力回退或运行失败时精确归还作者FSR、抖动与Mip状态。
#[cfg(feature = "fsr3_upscaling")]
fn restore_fsr3(
    commands: &mut Commands,
    entity: Entity,
    current_fsr: Option<&Fsr3Upscaling>,
    ownership: &Fsr3UpscalingOwnership,
) {
    let mut camera = commands.entity(entity);
    match ownership.authored_fsr.clone() {
        Some(authored) => camera.insert(authored),
        None if current_fsr.is_some() => camera.remove::<Fsr3Upscaling>(),
        None => &mut camera,
    };
    match ownership.temporal_jitter.clone() {
        Some(authored) => camera.insert(authored),
        None => camera.remove::<TemporalJitter>(),
    };
    match ownership.mip_bias.clone() {
        Some(authored) => camera.insert(authored),
        None => camera.remove::<MipBias>(),
    };
    camera.remove::<(Fsr3UpscalingOwnership, Fsr3PostProcessTarget)>(); // 一轮所有权完整结束。
}

#[cfg(all(test, feature = "fsr3_upscaling"))]
mod tests; // 运行时所有权夹具独立放入同域子模块，保持实现文件低于500行。
