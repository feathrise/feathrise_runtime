//! 独立 DLSS Super Resolution 相机接管、 SR/RR 交接边界。

use bevy::prelude::*; // App、系统集、相机与资源常用类型。

use crate::{DlssRuntimeBlocker, DlssRuntimeStatus, RenderSettings}; // 正式设置请求、结构化状态与首次安装迁移阶段。

/// DLSS 相机管理在 PostUpdate 的公开排序锚点。
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DlssRuntimeSystems {
    /// 在普通画质收敛和租约恢复前维护独立 SR 所有权。
    MaintainSuperResolution,
}

/// 注册所有构建都需要的首次默认系统，以及正式 RTX 构建的独立 SR 管理器。
pub(crate) fn configure_dlss(app: &mut App) {
    #[cfg(feature = "dlss")]
    app.init_resource::<crate::resources::DlssSuperResolutionLeaseIndex>() // 相机直接销毁时仍可回收稳定租约票据。
        .add_systems(
            PostUpdate,
            (maintain_dlss_super_resolution, ApplyDeferred)
                .chain() // SR 标记和组件在普通 CAS 与租约系统读取前同帧落地。
                .in_set(DlssRuntimeSystems::MaintainSuperResolution)
                .before(super::GraphicsRuntimeLeaseSystems::RestoreReleased), // 先释放或取得 SR 租约，再由普通画质恢复/应用同帧结算。
        );
    #[cfg(not(feature = "dlss"))]
    app.add_systems(
        PostUpdate,
        publish_disabled_dlss_status.in_set(DlssRuntimeSystems::MaintainSuperResolution), // 便携构建仍向 UI 发布明确构建阻断。
    );
}

/// 便携构建把玩家可能从当前配置读到的 RTX 请求安全回退并发布原因。
#[cfg(not(feature = "dlss"))]
fn publish_disabled_dlss_status(
    settings: Res<RenderSettings>,           // 读取唯一持久请求源。
    mut status: ResMut<DlssRuntimeStatus>, // 更新 UI 可观察状态但不改设置。
) {
    status.requested_mode = settings.graphics.dlss_mode; // 显示玩家原请求，即使当前构建不能执行。
    status.effective_mode = crate::DlssMode::Off; // 无 SDK 构建永远没有实际 DLSS 模式。
    status.super_resolution_supported = false; // 当前二进制没有能力查询路径。
    status.ray_reconstruction_supported = false; // RR 同样未链接。
    status.raytracing_requested = settings.graphics.raytracing_enabled; // 保留玩家请求供状态页解释。
    status.ray_reconstruction_requested = settings.graphics.ray_reconstruction_enabled; // 不抹除玩家偏好。
    status.raytracing_effective = false; // 正式 Solari 由 rtx_rendering 特性装配，便携设置构建不生效。
    status.ray_reconstruction_effective = false; // 不存在 RR 相机组件。
    status.super_resolution_blocker = (settings.graphics.dlss_mode != crate::DlssMode::Off)
        .then_some(DlssRuntimeBlocker::BuildDisabled); // 只有真实请求时才显示构建阻断。
    status.refresh_path(0); // 最终路径固定安全光栅并重建原因列表。
}

#[cfg(feature = "dlss")]
use crate::{
    DlssMode,
    components::{DlssPostProcessTarget, DlssSuperResolutionOwnership},
    graphics::{GraphicsRuntimeLeaseRegistry, GraphicsRuntimeRequirements},
    resources::DlssSuperResolutionLeaseIndex,
};
#[cfg(feature = "dlss")] // 下列实现只在 Bevy 官方 DLSS 类型可用时编译。
use bevy::{
    anti_alias::dlss::{
        Dlss, DlssPerfQualityMode, DlssRayReconstructionFeature, DlssRayReconstructionSupported,
        DlssRuntimeTelemetry, DlssSuperResolutionFeature, DlssSuperResolutionSupported,
    }, // SR/RR 互斥组件、明确质量档与官方支持资源。
    camera::RenderTarget, // 纹理、无输出与局部视口相机必须排除。
    render::camera::{MipBias, TemporalJitter}, // 精确保存和恢复 DLSS required 附件。
}; // 独立 SR 所有权、画质租约与稳定模式映射。

/// 正式 SR 管理器读取的每台三维相机完整数据。
#[cfg(feature = "dlss")]
type DlssCameraData<'a> = (
    Entity,                                         // 目标相机实体，同时作为租约与所有权键。
    &'a Camera,                                     // 活动状态、局部 viewport 与最终物理尺寸。
    &'a Projection,                                 // 只允许透视相机，正交 UI/预览相机排除。
    &'a RenderTarget,                               // 只允许输出到窗口。
    Option<&'a Dlss<DlssSuperResolutionFeature>>,   // 当前作者或正式设置安装的 SR 组件。
    Option<&'a Dlss<DlssRayReconstructionFeature>>, // RR 存在时 SR 必须退出。
    Option<&'a TemporalJitter>,                     // 首次接管前的作者抖动值。
    Option<&'a MipBias>,                            // 首次接管前的作者纹理偏移。
    Option<&'a DlssSuperResolutionOwnership>,       // 本模块已有所有权与快照。
);

/// 幂等维护全部合格窗口透视相机的独立 SR，并按租约声明与 RR 原子交接。
#[cfg(feature = "dlss")]
#[allow(clippy::too_many_arguments)] // 支持资源、租约、索引与相机查询分别承担独立所有权职责。
fn maintain_dlss_super_resolution(
    mut commands: Commands, // 只在首次挂接、真实切档、尺寸变化或恢复时提交命令。
    settings: Res<RenderSettings>, // 唯一持久 DLSS/光追/RR 请求源。
    support: Option<Res<DlssSuperResolutionSupported>>, // 官方 SR 综合能力事实。
    rr_support: Option<Res<DlssRayReconstructionSupported>>, // 设置页提前显示 RR 能力。
    telemetry: Option<Res<DlssRuntimeTelemetry>>, // RenderApp 实际内部尺寸与上下文失败的共享快照。
    mut leases: ResMut<GraphicsRuntimeLeaseRegistry>, // 临时接管普通画质但不改玩家值。
    mut lease_index: ResMut<DlssSuperResolutionLeaseIndex>, // 回收直接销毁相机的票据。
    cameras: Query<DlssCameraData<'_>, With<Camera3d>>, // 仅枚举有限三维相机集合。
    mut status: ResMut<DlssRuntimeStatus>, // 发布请求、实际模式、路径、尺寸和原因。
) {
    let sr_telemetry = telemetry
        .as_deref()
        .map(DlssRuntimeTelemetry::snapshot)
        .map(|snapshot| snapshot.super_resolution)
        .unwrap_or_default(); // SDK 尚未初始化时使用空快照，不猜测内部尺寸。
    lease_index.leases.retain(|camera, lease| {
        if cameras.get(*camera).is_ok() {
            true // 相机仍存在时由下面的正常所有权路径维护。
        } else {
            leases.release(*lease); // 相机或 Camera3d 被直接移除时立即回收遗留票据。
            false // 同步移除索引，稳定帧不再重复释放。
        }
    });

    let requested_mode = settings.graphics.dlss_mode; // 一帧只读取一次玩家请求。
    let sr_supported = support.is_some(); // 不猜 GPU 名称或环境变量。
    status.requested_mode = requested_mode; // UI 始终展示持久请求而非静默回退值。
    status.super_resolution_supported = sr_supported; // 发布官方支持资源存在性。
    status.ray_reconstruction_supported = rr_support.is_some(); // RR 依赖状态即使光追关闭也可见。
    status.raytracing_requested = settings.graphics.raytracing_enabled; // 与正式设置保持同帧一致。
    status.ray_reconstruction_requested = settings.graphics.ray_reconstruction_enabled; // 灰显时仍保留选择。
    status.transitioning = false; // 仅下面观察到 SR/RR 交接时置位。

    let mut eligible_camera_count = 0_u32; // 统计资格用于区分无相机与能力失败。
    let mut active_sr_count = 0_u32; // 统计本帧命令落地后的目标 SR 数量。
    let mut first_output = UVec2::ZERO; // 设置页报告第一台合格窗口相机尺寸。
    let mut first_effective_mode = DlssMode::Off; // 多相机通常同尺寸，报告第一台实际模式。
    let mut first_resolved_mode = DlssMode::Off; // SR/RR交接空帧仍保留即将采用的明确模式，避免请求振荡。
    let mut auto_resolution_blocker = None; // Auto 低分辨率只需发布一个稳定原因。
    let mut runtime_failure_blocker = None; // 相同模式/尺寸失败后停止重试并释放普通画质租约。

    for (entity, camera, projection, target, current_sr, current_rr, jitter, mip_bias, ownership) in
        &cameras
    {
        let eligible = dlss_camera_is_eligible(camera, projection, target); // 统一执行活动/窗口/透视/无 viewport 过滤。
        let output = camera.physical_target_size().unwrap_or(UVec2::ZERO); // 新建或最小化相机可能暂时没有有效尺寸。
        if eligible && output.x > 0 && output.y > 0 {
            eligible_camera_count += 1; // 只有真实物理尺寸相机可创建 NGX 上下文。
            if first_output == UVec2::ZERO {
                first_output = output; // 固定取查询顺序第一台作为状态主尺寸。
            }
        }
        let effective_mode = requested_mode.resolve_for_output(output); // Auto 永不直接传给当前封装。
        if eligible
            && output.x > 0
            && output.y > 0
            && effective_mode.is_enabled()
            && first_resolved_mode == DlssMode::Off
        {
            first_resolved_mode = effective_mode; // 组件交接空帧仍发布稳定明确模式。
        }
        if eligible && requested_mode == DlssMode::Auto && effective_mode == DlssMode::Off {
            auto_resolution_blocker = Some(DlssRuntimeBlocker::ResolutionTooLow { output }); // 低于 1080p 是正式主动停用而非失败。
        }
        let suspended = leases
            .requirements_for_camera(entity)
            .is_some_and(|requirements| requirements.suspend_dlss_super_resolution); // RR先通过声明要求 SR 退出。
        let desired_sdk_mode = dlss_perf_quality_mode(effective_mode); // Off/Auto低分辨率没有 SDK 档位。
        let telemetry_failure = desired_sdk_mode.and_then(|mode| {
            (sr_telemetry.output_resolution == output
                && sr_telemetry.perf_quality_mode == Some(mode))
            .then(|| sr_telemetry.failure.clone())
            .flatten()
        }); // 只阻断 SDK 已明确拒绝的同一输出尺寸与模式，切档或缩放可重新尝试。
        if runtime_failure_blocker.is_none()
            && let Some(detail) = telemetry_failure.clone()
        {
            runtime_failure_blocker = Some(DlssRuntimeBlocker::RuntimeFailure { detail });
        }
        let should_attach = eligible
            && output.x > 0
            && output.y > 0
            && sr_supported
            && effective_mode.is_enabled()
            && !suspended
            && current_rr.is_none()
            && telemetry_failure.is_none(); // 非致命失败回传后同帧恢复普通渲染路径。

        if let Some(ownership) = ownership {
            if !should_attach {
                restore_super_resolution(&mut commands, entity, current_sr, ownership); // 先恢复作者 SR、抖动与 Mip。
                leases.release(ownership.lease); // 同帧归还普通画质接管。
                lease_index.leases.remove(&entity); // 清除实体至票据索引。
                if suspended || current_rr.is_some() {
                    status.transitioning = settings.graphics.raytracing_enabled
                        && settings.graphics.ray_reconstruction_enabled
                        && requested_mode.is_enabled(); // 只有 RR 组合请求才把正常暂停解释为交接。
                }
                continue;
            }

            let desired_mode = desired_sdk_mode.expect("已过滤 Off 与 Auto，实际模式必须可映射"); // Auto 已解析，Off 已被 should_attach 排除。
            let context_changed = current_sr.is_none_or(|current| {
                current.perf_quality_mode != desired_mode
                    || ownership.applied_mode != effective_mode
                    || ownership.output_resolution != output
            }); // 只有首次缺失、真实切档或输出尺寸变化才重建上下文并 reset。
            if context_changed {
                commands
                    .entity(entity)
                    .insert(Dlss::<DlssSuperResolutionFeature> {
                        perf_quality_mode: desired_mode, // 明确档位，禁止封装内部 Auto 产生另一套策略。
                        reset: true, // 模式或尺寸变化只在该帧清空一次 DLSS 历史。
                        _phantom_data: Default::default(), // 特征类型固定为 Super Resolution。
                    });
                commands
                    .entity(entity)
                    .insert(DlssSuperResolutionOwnership {
                        applied_mode: effective_mode, // 更新明确模式但保留首次作者快照。
                        output_resolution: output,    // 更新上下文对应最终尺寸。
                        ..ownership.clone()
                    });
            }
            active_sr_count += 1; // 稳定或本帧更新后都继续由 SR 接管。
            if first_effective_mode == DlssMode::Off {
                first_effective_mode = effective_mode; // 发布第一台实际明确模式。
            }
            continue;
        }

        if !should_attach {
            if current_sr.is_none() {
                lease_index.leases.remove(&entity); // 清理任何不完整旧索引，防止后续双租约。
            }
            if (suspended || current_rr.is_some())
                && settings.graphics.raytracing_enabled
                && settings.graphics.ray_reconstruction_enabled
                && requested_mode.is_enabled()
            {
                status.transitioning = current_rr.is_none(); // RR 已存在时交接完成，不再显示过渡。
            }
            continue; // 不接管作者在非目标相机上的 SR 组件。
        }

        if let Some(stale_lease) = lease_index.leases.remove(&entity) {
            leases.release(stale_lease); // 防御命令中断留下的索引，先回收再取得唯一新租约。
        }
        let lease = leases.acquire(GraphicsRuntimeRequirements::dlss_super_resolution(entity)); // 普通画质值保持持久不变。
        let desired_mode = desired_sdk_mode.expect("已过滤 Off 与 Auto，实际模式必须可映射"); // 只把 SDK 支持的明确枚举写入组件。
        commands.entity(entity).insert((
            Dlss::<DlssSuperResolutionFeature> {
                perf_quality_mode: desired_mode,   // 首次挂接使用当前明确模式。
                reset: true,                       // 新上下文没有可复用历史。
                _phantom_data: Default::default(), // 固定 SR 特征类型。
            },
            DlssSuperResolutionOwnership {
                lease,                              // 保存唯一租约票据供关闭、交接或相机复用时释放。
                authored_dlss: current_sr.cloned(), // 完整保存作者组件或缺失状态。
                temporal_jitter: jitter.cloned(),   // 保存 required 插入前抖动作者值。
                mip_bias: mip_bias.cloned(),        // 保存 required 插入前 Mip 作者值。
                applied_mode: effective_mode,       // 记录明确实际模式，禁止保存 Auto。
                output_resolution: output,          // 记录创建上下文时的最终尺寸。
            },
            DlssPostProcessTarget, // 只有真实 SR 目标可使用专用后处理锐化。
        ));
        lease_index.leases.insert(entity, lease); // 相机销毁时仍可回收租约。
        active_sr_count += 1; // 本帧 PostUpdate 末命令会成为实际 SR 目标。
        if first_effective_mode == DlssMode::Off {
            first_effective_mode = effective_mode; // 发布第一台实际明确模式。
        }
    }

    status.output_resolution = first_output; // 无合格相机时明确回到零尺寸。
    let runtime_failed = runtime_failure_blocker.is_some();
    status.effective_mode = if status.ray_reconstruction_effective {
        status.effective_mode // RR 集成系统已经发布其明确模式，不由 SR 覆盖。
    } else if first_effective_mode != DlssMode::Off {
        first_effective_mode // 至少一台SR已真实接管时发布其明确模式。
    } else if runtime_failed {
        DlssMode::Off // SDK 创建失败后的实际路径已经恢复，不得继续宣称明确模式生效。
    } else {
        first_resolved_mode // SR/RR交接空帧保持即将采用的明确模式，Auto低分辨率仍为Off。
    };
    status.internal_resolution = if active_sr_count > 0
        && sr_telemetry.output_resolution == first_output
        && sr_telemetry.perf_quality_mode == dlss_perf_quality_mode(first_effective_mode)
        && sr_telemetry.failure.is_none()
    {
        sr_telemetry.internal_resolution // 只报告 SDK 真实创建成功后返回的最佳固定内部尺寸。
    } else if !status.ray_reconstruction_effective {
        UVec2::ZERO // 交接、关闭或 SR 失败时清除陈旧尺寸；RR 集成可保留自己的尺寸。
    } else {
        status.internal_resolution
    };
    status.super_resolution_blocker = if requested_mode == DlssMode::Off {
        None // 主动关闭不是错误，也不显示能力噪声。
    } else if !sr_supported {
        Some(DlssRuntimeBlocker::NgxUnavailable {
            detail: "Vulkan、RTX、驱动或 nvngx_dlss.dll 未通过官方初始化".to_string(), // 官方支持资源不区分这些底层失败，状态保持诚实合并描述。
        })
    } else if eligible_camera_count == 0 {
        Some(DlssRuntimeBlocker::NoEligibleCamera) // 纹理、正交、局部 viewport 和未就绪相机均排除。
    } else if let Some(blocker) = runtime_failure_blocker {
        Some(blocker) // RenderApp 已回退；相同模式/尺寸保持稳定且不重复创建上下文。
    } else if active_sr_count == 0 && !status.ray_reconstruction_effective {
        auto_resolution_blocker // Auto 低分辨率或 RR 交接会分别给出明确原因。
    } else {
        None // 至少一台 SR 或 RR 目标真实生效。
    };
    status.refresh_path(active_sr_count); // 最后统一计算互斥路径、目标数与公开阻断列表。
}

/// 判断相机是否属于正式 DLSS 接管范围，不依赖 GPU 或分辨率能力。
#[cfg(feature = "dlss")]
fn dlss_camera_is_eligible(
    camera: &Camera,
    projection: &Projection,
    target: &RenderTarget,
) -> bool {
    camera.is_active // 已暂停或隐藏的相机不创建昂贵 NGX 上下文。
        && camera.viewport.is_none() // 局部 viewport 与分屏相机明确排除。
        && camera.sub_camera_view.is_none() // 大视图子相机同样不满足完整输出契约。
        && matches!(projection, Projection::Perspective(_)) // 正交相机不提供 DLSS 所需投影语义。
        && matches!(target, RenderTarget::Window(_)) // 角色预览、小地图和纹理烘焙相机保持作者路径。
}

/// 把项目公开明确模式映射到 Bevy 当前 DLSS 封装枚举；Auto 必须先解析。
#[cfg(feature = "dlss")]
pub(crate) const fn dlss_perf_quality_mode(mode: DlssMode) -> Option<DlssPerfQualityMode> {
    match mode {
        DlssMode::Off | DlssMode::Auto => None, // 关闭不挂接，Auto 不得传给当前封装。
        DlssMode::Quality => Some(DlssPerfQualityMode::Quality), // 质量档一一映射。
        DlssMode::Balanced => Some(DlssPerfQualityMode::Balanced), // 平衡档一一映射。
        DlssMode::Performance => Some(DlssPerfQualityMode::Performance), // 性能档一一映射。
        DlssMode::UltraPerformance => Some(DlssPerfQualityMode::UltraPerformance), // 超性能档一一映射。
        DlssMode::Dlaa => Some(DlssPerfQualityMode::Dlaa), // DLAA 保持原生内部尺寸。
    }
}

/// 关闭、能力回退或 RR 交接时精确归还作者 SR、抖动与 Mip 状态。
#[cfg(feature = "dlss")]
fn restore_super_resolution(
    commands: &mut Commands, // 目标相机组件恢复入口。
    entity: Entity,          // 本次所有权绑定的明确相机。
    current_sr: Option<&Dlss<DlssSuperResolutionFeature>>, // 当前 SR 存在性用于避免无意义移除。
    ownership: &DlssSuperResolutionOwnership, // 首次接管快照与稳定租约。
) {
    let mut camera = commands.entity(entity); // 后续恢复命令只访问同一目标实体。
    match ownership.authored_dlss.clone() {
        Some(authored) => camera.insert(authored), // 作者原有 SR 时恢复完整模式与 reset 值。
        None if current_sr.is_some() => camera.remove::<Dlss<DlssSuperResolutionFeature>>(), // 移除仅由正式设置引入的 SR。
        None => &mut camera, // 当前已经缺失时保持零写。
    };
    match ownership.temporal_jitter.clone() {
        Some(authored) => camera.insert(authored), // 恢复作者完整抖动偏移。
        None => camera.remove::<TemporalJitter>(), // 移除仅由 DLSS required 关系引入的抖动。
    };
    match ownership.mip_bias.clone() {
        Some(authored) => camera.insert(authored), // 恢复作者完整 Mip 偏移。
        None => camera.remove::<MipBias>(),        // 移除仅由 DLSS required 关系引入的偏移。
    };
    camera.remove::<DlssSuperResolutionOwnership>(); // 作者状态归还后结束本轮所有权。
    camera.remove::<DlssPostProcessTarget>(); // SR 退出后同帧恢复普通 CAS 控制权。
}

#[cfg(all(test, feature = "dlss"))]
mod tests {
    use super::*; // 纯函数覆盖模式映射与相机资格，不初始化真实 GPU。
    use bevy::window::WindowRef; // 构造主窗口目标。

    #[test]
    fn camera_filter_accepts_only_active_full_window_perspective_views() {
        let mut camera = Camera::default(); // 默认活动且没有局部 viewport。
        let perspective = Projection::Perspective(Default::default()); // 正式三维透视投影。
        let orthographic = Projection::Orthographic(OrthographicProjection::default_3d()); // 应排除的正交投影。
        let window = RenderTarget::Window(WindowRef::Primary); // 主菜单与玩法正式窗口目标。
        let texture = RenderTarget::None { size: UVec2::ONE }; // 代表纹理/无颜色辅助目标。
        assert!(dlss_camera_is_eligible(&camera, &perspective, &window)); // 完整窗口透视相机符合资格。
        assert!(!dlss_camera_is_eligible(&camera, &orthographic, &window)); // 正交相机排除。
        assert!(!dlss_camera_is_eligible(&camera, &perspective, &texture)); // 非窗口目标排除。
        camera.is_active = false; // 模拟切镜后旧相机暂停。
        assert!(!dlss_camera_is_eligible(&camera, &perspective, &window)); // 非活动相机排除。
    }

    #[test]
    fn all_explicit_modes_map_without_forwarding_auto() {
        assert_eq!(dlss_perf_quality_mode(DlssMode::Off), None); // 关闭不创建组件。
        assert_eq!(dlss_perf_quality_mode(DlssMode::Auto), None); // Auto 必须先按输出解析。
        for mode in [
            DlssMode::Quality,
            DlssMode::Balanced,
            DlssMode::Performance,
            DlssMode::UltraPerformance,
            DlssMode::Dlaa,
        ] {
            assert!(dlss_perf_quality_mode(mode).is_some()); // 五个明确 SDK 档位全部可用。
        }
    }
}
