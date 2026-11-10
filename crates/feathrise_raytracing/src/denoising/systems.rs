//! DLSS RR 目标相机组件的幂等挂接、Solari历史同步与回退实现。

#[cfg(feature = "dlss_rr")] // 普通test_panel构建不能引用需要外部NVIDIA SDK的类型。
use bevy::{
    anti_alias::dlss::{
        Dlss, DlssPerfQualityMode, DlssRayReconstructionFeature,
        DlssRayReconstructionInputColorDebug, DlssRayReconstructionSupported, DlssRuntimeTelemetry,
        DlssSuperResolutionFeature,
    }, // Bevy官方DLSS RR相机组件、显式质量档与启动后支持资源。
    prelude::*,                                // Commands、Query、Res与玩法相机组件。
    render::camera::{MipBias, TemporalJitter}, // required组件中必须保存具体作者值的两项附件。
};
#[cfg(feature = "dlss_rr")] // 基础Solari构建不链接厂商降噪实现。
use bevy_solari::prelude::SolariLighting; // 同帧把Solari历史重置同步给DLSS RR。

#[cfg(feature = "dlss_rr")] // 真实feature才会动态暂停FXAA并继续维持Solari单采样租约。
use feathrise_settings::{
    DlssPostProcessTarget, GraphicsRuntimeLeaseRegistry, GraphicsRuntimeRequirements,
}; // RR只更新本会话已有租约并声明专用后处理目标，不直接写玩家画质值。

#[cfg(feature = "dlss_rr")] // 真实feature才需要访问相机快照与运行时所有权。
use crate::{
    resources::{RaytracingPrototypeRequest, RaytracingRuntime, RollbackDisposition}, // F1模式请求、单一会话所有权与相机换代回退入口。
    types::{
        DlssRayReconstructionCameraSnapshot, DlssRayReconstructionDebugView,
        DlssRayReconstructionMode, DlssRayReconstructionUnavailableReason,
        RaytracingPrototypeBlocker, RaytracingPrototypeStage, SolariHistoryResetReason,
    }, // 厂商组件快照、稳定诊断原因和阶段边界。
};

#[cfg(feature = "dlss_rr")] // 查询只在真实DLSS编译路径存在。
type DlssCameraData<'a> = (
    &'a Camera, // 失败锁必须绑定真实输出尺寸，窗口缩放后允许重新查询。
    Option<&'a Dlss<DlssSuperResolutionFeature>>, // 独立SR必须先退出，同一相机绝不允许两种DLSS组件共存。
    Option<&'a Dlss<DlssRayReconstructionFeature>>, // 接管前可能已有的作者RR组件。
    Option<&'a TemporalJitter>,                   // 图形租约已经稳定后、DLSS插入前的抖动作者值。
    Option<&'a MipBias>,                          // 图形租约已经稳定后、DLSS插入前的Mip偏移作者值。
    Option<&'a mut SolariLighting>,               // 模式或内部尺寸变化时同步失效Solari历史。
    Has<DlssRayReconstructionInputColorDebug>,    // 诊断呈现标记只在请求真实变化时写入。
    Has<DlssPostProcessTarget>,                   // RR活动时保持DLSS专用CAS目标标记。
);

/// 在Solari挂接完成后幂等安装RR；设备不支持时只发布原因，绝不阻断基础P1。
#[cfg(feature = "dlss_rr")] // 真实实现由dlss或rtx_rendering传播的内部能力注册。
pub(crate) fn maintain_dlss_ray_reconstruction(
    mut commands: Commands, // 仅在目标相机尚未拥有RR时提交一次组件插入。
    request: Res<RaytracingPrototypeRequest>, // F1只通过项目自有关闭/DLAA/Quality枚举提出请求。
    support: Option<Res<DlssRayReconstructionSupported>>, // Bevy完成Vulkan/NGX初始化后才存在。
    telemetry: Option<Res<DlssRuntimeTelemetry>>, // RenderApp 回传真实内部尺寸或非致命上下文失败。
    mut cameras: Query<DlssCameraData<'_>, With<Camera3d>>, // 只访问生命周期已经锁定的玩法三维相机。
    mut runtime: ResMut<RaytracingRuntime>,                 // 保存能力、挂接状态与精确回退快照。
) {
    let rr_telemetry = telemetry
        .as_deref()
        .map(DlssRuntimeTelemetry::snapshot)
        .map(|snapshot| snapshot.ray_reconstruction)
        .unwrap_or_default(); // SDK 首次创建前不猜测能力或内部尺寸。
    let supported = support.is_some(); // 不猜适配器名称，官方支持资源是唯一能力事实。
    runtime.dlss_rr_supported = supported; // F1在未进入玩法场景前也能显示真实设备能力。
    if request.dlss_rr_mode == DlssRayReconstructionMode::Off {
        if runtime.dlss_rr_active
            && let Some(camera) = runtime.camera
            && let Ok((_, _, _, _, _, Some(mut solari), _, _)) = cameras.get_mut(camera)
        {
            request_denoising_history_reset(&mut solari, &mut runtime); // 从低分辨率RR返回基础Solari时两套历史必须同时换代。
        }
        restore_dlss_camera_snapshot(&mut commands, &mut runtime); // 关闭模式立即恢复作者RR、抖动和Mip附件，不等待整个光追会话回退。
        runtime.dlss_rr_active = false; // 基础Solari继续活动，但厂商RR不再属于本原型。
        runtime.dlss_rr_cpu_ms = None; // 主动关闭后不能继续显示旧NGX同步调用时间。
        runtime.dlss_rr_gpu_ms = None; // 主动关闭后不能继续显示旧RR节点时间。
        runtime.dlss_rr_unavailable_reason =
            Some(DlssRayReconstructionUnavailableReason::RequestedOff); // F1明确这是主动A/B而非设备失败。
        return;
    }
    if !supported {
        if runtime.dlss_rr_active
            && let Some(camera) = runtime.camera
            && let Ok((_, _, _, _, _, Some(mut solari), _, _)) = cameras.get_mut(camera)
        {
            request_denoising_history_reset(&mut solari, &mut runtime); // 运行期NGX能力消失时恢复全分辨率历史代次。
        }
        restore_dlss_camera_snapshot(&mut commands, &mut runtime); // 能力丢失不能把已挂接厂商组件留在相机上。
        runtime.dlss_rr_active = false; // AMD或NGX初始化失败时保持无厂商组件路径。
        runtime.dlss_rr_cpu_ms = None; // 不允许显示上一次支持设备或旧会话的Render线程CPU时间。
        runtime.dlss_rr_gpu_ms = None; // 不允许显示上一次支持设备或旧会话的诊断值。
        runtime.dlss_rr_unavailable_reason =
            Some(DlssRayReconstructionUnavailableReason::DeviceOrBackendUnsupported); // 明确基础Solari仍可继续完成P1诊断。
        return;
    }

    let solari_camera_ready = runtime.solari_inserted
        && matches!(
            runtime.stage,
            RaytracingPrototypeStage::WarmingSolari | RaytracingPrototypeStage::Active
        ); // 只有租约已结算且Solari已挂接的唯一相机才能成为RR目标。
    if !solari_camera_ready {
        runtime.dlss_rr_active = false; // 候选、适配和回退阶段没有有效降噪视图。
        runtime.dlss_rr_cpu_ms = None; // 等待阶段不能沿用旧视图的NGX同步调用时间。
        runtime.dlss_rr_gpu_ms = None; // 等待阶段不能沿用旧视图时间。
        runtime.dlss_rr_unavailable_reason =
            Some(DlssRayReconstructionUnavailableReason::WaitingForSolariCamera); // F1区分设备支持与当前场景尚未激活。
        return;
    }

    let Some(camera) = runtime.camera else {
        runtime.dlss_rr_active = false; // 生命周期下一系统会完成相机缺失回退。
        runtime.dlss_rr_cpu_ms = None; // 相机缺失帧不能继续显示旧视图Render线程CPU诊断。
        runtime.dlss_rr_gpu_ms = None; // 相机缺失帧不能继续显示旧视图GPU诊断。
        runtime.dlss_rr_unavailable_reason =
            Some(DlssRayReconstructionUnavailableReason::WaitingForSolariCamera); // 不把短暂相机重建误报为设备不支持。
        return;
    };
    let Ok((
        camera_view,
        current_sr,
        current_dlss,
        temporal_jitter,
        mip_bias,
        mut solari,
        input_debug_active,
        post_process_target,
    )) = cameras.get_mut(camera)
    else {
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::CameraRebuilt),
            RollbackDisposition::RasterReady,
        ); // 目标实体不再是三维相机时先完整撤销旧所有权。
        return;
    };

    if current_sr.is_some() {
        runtime.dlss_rr_active = false; // SR仍存在的帧禁止提交RR组件，先等待设置模块完成移除。
        runtime.dlss_rr_cpu_ms = None; // 交接帧没有RR执行时间。
        runtime.dlss_rr_gpu_ms = None; // 交接帧没有RR GPU节点。
        runtime.dlss_rr_unavailable_reason =
            Some(DlssRayReconstructionUnavailableReason::TransitioningFromSuperResolution); // 结构化区分安全交接与能力失败。
        return;
    }

    let desired_mode = match request.dlss_rr_mode {
        DlssRayReconstructionMode::Dlaa => DlssPerfQualityMode::Dlaa, // 原生内部尺寸优先清晰度。
        DlssRayReconstructionMode::Quality => DlssPerfQualityMode::Quality, // 显式替代1080p Auto暗中选择的Quality。
        DlssRayReconstructionMode::Balanced => DlssPerfQualityMode::Balanced, // 平衡档一一映射。
        DlssRayReconstructionMode::Performance => DlssPerfQualityMode::Performance, // 性能档一一映射。
        DlssRayReconstructionMode::UltraPerformance => DlssPerfQualityMode::UltraPerformance, // 4K超性能档一一映射。
        DlssRayReconstructionMode::Off => unreachable!("关闭模式已在相机查询前返回"), // 防御未来分支调整遗漏。
    };
    let output_resolution = camera_view.physical_target_size().unwrap_or(UVec2::ZERO);
    let context_failed = rr_telemetry.output_resolution == output_resolution
        && rr_telemetry.perf_quality_mode == Some(desired_mode)
        && rr_telemetry.failure.is_some(); // 同模式与尺寸失败保持锁定，窗口缩放或切档会重新尝试。
    if context_failed {
        if runtime.dlss_rr_active
            && let Some(solari) = solari.as_deref_mut()
        {
            request_denoising_history_reset(solari, &mut runtime); // 从失败的低分辨率上下文恢复完整历史代次。
        }
        restore_dlss_camera_snapshot(&mut commands, &mut runtime); // 移除本模块RR并归还作者抖动与Mip。
        runtime.dlss_rr_active = false;
        runtime.dlss_rr_cpu_ms = None;
        runtime.dlss_rr_gpu_ms = None;
        runtime.dlss_rr_unavailable_reason =
            Some(DlssRayReconstructionUnavailableReason::ContextCreationFailed);
        return; // 后续租约同步会解除SR暂停，形成Solari+SR安全回退。
    }

    let snapshot_camera = runtime
        .dlss_rr_camera_snapshot
        .as_ref()
        .map(|snapshot| snapshot.camera); // 不长期借用runtime，后续才能提交回退请求或写入快照。
    if snapshot_camera.is_some_and(|snapshot_camera| snapshot_camera != camera) {
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::CameraRebuilt),
            RollbackDisposition::RasterReady,
        ); // 旧快照绝不能被挪用到同一帧新建的相机实体。
        return;
    }
    if runtime.dlss_rr_camera_snapshot.is_none() {
        runtime.dlss_rr_camera_snapshot = Some(DlssRayReconstructionCameraSnapshot {
            camera,                                    // 保存生命周期已经锁定的唯一玩法相机。
            authored_dlss: current_dlss.cloned(), // 外部作者RR存在时完整保存质量模式与reset值。
            temporal_jitter: temporal_jitter.cloned(), // DLSS required插入前保存完整抖动偏移。
            mip_bias: mip_bias.cloned(),          // DLSS required插入前保存完整Mip偏移。
        }); // HDR、深度和运动矢量的作者状态已由上一帧图形租约快照拥有。
    }
    let context_must_change =
        current_dlss.is_none_or(|current| current.perf_quality_mode != desired_mode); // 只有首次挂接或用户真实切档才重建昂贵NGX上下文。
    let initial_warmup_attach =
        current_dlss.is_none() && runtime.stage == RaytracingPrototypeStage::WarmingSolari; // 首次预热由PostUpdate统一记录InitialHistory，不能被RR原因抢占。
    if context_must_change {
        commands.entity(camera).insert((
            Dlss::<DlssRayReconstructionFeature> {
                perf_quality_mode: desired_mode, // 禁止Auto在1080p阈值边界静默选择低分辨率档。
                reset: true, // 首个RR帧没有可复用历史，必须与Solari首帧共同清空。
                _phantom_data: Default::default(), // 通过特征类型固定为Ray Reconstruction而非Super Resolution。
            },
            DlssPostProcessTarget, // RR真实挂接后允许专用DLSS后处理锐化接管该相机。
        )); // required组件只在首次缺失时补齐，稳定帧不重复写入相机。
        if let Some(solari) = solari.as_deref_mut() {
            if initial_warmup_attach {
                solari.reset = true; // 初次挂接只保证两套首帧清空，根因留给统一首次历史诊断。
            } else {
                request_denoising_history_reset(solari, &mut runtime); // 切换DLAA/Quality会改变内部尺寸，Solari与RR必须同帧换代。
            }
        }
    }
    sync_input_color_debug_view(
        &mut commands,
        camera,
        input_debug_active,
        request.dlss_rr_debug_view,
    ); // 只切输出呈现，不改变RR上下文、抖动、guide或任一历史代次。
    if !post_process_target {
        commands.entity(camera).insert(DlssPostProcessTarget); // 外部移除标记时恢复一次，稳定帧零写。
    }
    runtime.dlss_rr_active = true; // 从本帧Render提取开始目标相机使用RR重建。
    runtime.dlss_rr_unavailable_reason = None; // 活动态F1明确显示无不可用原因。
}

/// 仅在 F1 呈现请求真实变化时增删调试标记，稳定帧保持相机组件零写。
#[cfg(feature = "dlss_rr")] // 标记由本地 Bevy DLSS 补丁提供，普通 Solari 构建不引用。
fn sync_input_color_debug_view(
    commands: &mut Commands,                   // 把请求同步到唯一目标相机。
    camera: Entity,                            // 生命周期已经锁定的玩法相机。
    input_debug_active: bool,                  // 本帧命令执行前的真实组件存在位。
    requested: DlssRayReconstructionDebugView, // F1 非持久呈现请求。
) {
    let should_show_input = requested == DlssRayReconstructionDebugView::InputColor; // 正常输出路径不携带调试标记。
    if should_show_input == input_debug_active {
        return; // 稳定帧不重复触发提取或RenderApp组件change tick。
    }
    if should_show_input {
        commands
            .entity(camera)
            .insert(DlssRayReconstructionInputColorDebug); // RR继续执行，随后用同帧pInColor覆盖其可见输出。
    } else {
        commands
            .entity(camera)
            .remove::<DlssRayReconstructionInputColorDebug>(); // 立即恢复正常RR结果呈现。
    }
}

/// 把RR模式变化并入既有Solari历史诊断；同帧已有更早根因时不覆盖首因。
#[cfg(feature = "dlss_rr")] // 普通Solari构建没有厂商模式变化。
fn request_denoising_history_reset(
    solari: &mut SolariLighting,     // 目标相机当前Solari历史标志。
    runtime: &mut RaytracingRuntime, // 记录同帧首个可审计重置原因。
) {
    if !solari.reset {
        solari.reset = true; // 内部分辨率或重建器变化后不得复用上一模式的光照历史。
    }
    if !runtime.has_pending_solari_history_reset() {
        runtime.note_solari_history_reset(SolariHistoryResetReason::DenoisingModeChanged); // 初次挂接已有InitialHistory时保持原首因。
    }
}

/// 恢复本原型接管前的RR、TemporalJitter与MipBias；可由独立关闭或统一回退复用。
#[cfg(feature = "dlss_rr")] // 只有真实DLSS构建持有厂商相机快照。
fn restore_dlss_camera_snapshot(
    commands: &mut Commands,         // 向快照绑定的明确相机提交恢复命令。
    runtime: &mut RaytracingRuntime, // 一次取走快照并清除所有权。
) {
    let snapshot = runtime.dlss_rr_camera_snapshot.take(); // 快照只允许被关闭或回退事务消费一次。
    if let Some(snapshot) = snapshot
        && let Ok(mut camera_commands) = commands.get_entity(snapshot.camera)
    {
        camera_commands.remove::<DlssRayReconstructionInputColorDebug>(); // 调试呈现完全由本会话拥有，回退不得泄漏到作者相机。
        camera_commands.remove::<DlssPostProcessTarget>(); // RR退出后普通CAS立即恢复最新玩家选择。
        match snapshot.authored_dlss {
            Some(authored) => {
                camera_commands.insert(authored); // 恢复外部作者已有的RR质量模式与历史标志。
            }
            None => {
                camera_commands.remove::<Dlss<DlssRayReconstructionFeature>>(); // 移除仅由本crate引入的厂商组件。
            }
        }
        match snapshot.temporal_jitter {
            Some(authored) => {
                camera_commands.insert(authored); // 恢复DLSS挂接前的完整投影抖动值。
            }
            None => {
                camera_commands.remove::<TemporalJitter>(); // 移除仅由DLSS required关系引入的抖动组件。
            }
        }
        match snapshot.mip_bias {
            Some(authored) => {
                camera_commands.insert(authored); // 恢复DLSS挂接前的完整Mip偏移。
            }
            None => {
                camera_commands.remove::<MipBias>(); // 移除仅由DLSS required关系引入的Mip组件。
            }
        }
    } // HDR、DepthPrepass与MotionVectorPrepass继续由图形租约统一拥有。
}

/// 在同一Update链末把RR最终抗锯齿所有权写回既有Solari租约，禁止FXAA二次平滑。
#[cfg(feature = "dlss_rr")] // 普通test_panel没有RR活动态需要覆盖FXAA。
pub(crate) fn sync_dlss_antialiasing_requirement(
    request: Res<RaytracingPrototypeRequest>, // 主动关闭时必须立即恢复基础Solari FXAA。
    mut leases: ResMut<GraphicsRuntimeLeaseRegistry>, // 只更新本crate持有的稳定租约票据。
    runtime: Res<RaytracingRuntime>,          // 读取目标相机、阶段与实际RR活动位。
) {
    if runtime.pending_rollback.is_some() {
        return; // 统一回退同帧由生命周期释放租约，不能再提交迟到更新。
    }
    let (Some(lease), Some(camera)) = (runtime.lease, runtime.camera) else {
        return; // 候选发现阶段尚未取得任何相机画质所有权。
    };
    let rr_requested = runtime.dlss_rr_supported
        && request.dlss_rr_mode != DlssRayReconstructionMode::Off
        && runtime.dlss_rr_unavailable_reason
            != Some(DlssRayReconstructionUnavailableReason::ContextCreationFailed)
        && runtime.solari_inserted
        && matches!(
            runtime.stage,
            RaytracingPrototypeStage::WarmingSolari | RaytracingPrototypeStage::Active
        ); // 只有能力和Solari目标均就绪才暂停SR，能力不足继续使用Solari+SR回退。
    let rr_active = runtime.dlss_rr_active && rr_requested; // 真实RR组件活动后才暂停FXAA。
    let requirements = GraphicsRuntimeRequirements::solari_warmup(camera)
        .with_fxaa_suspended(rr_active)
        .with_dlss_super_resolution_suspended(rr_requested); // 先要求SR退出，确认缺失后下一帧RR才挂接。
    leases.update(lease, requirements); // 票据失效时安全返回false，由统一回退继续收口。
}

/// 回退事务释放图形租约前，恢复DLSS自身、TemporalJitter与MipBias的接管前状态。
#[cfg(feature = "dlss_rr")] // 普通Solari构建没有厂商相机组件需要恢复。
pub(crate) fn restore_dlss_ray_reconstruction_on_rollback(
    mut commands: Commands, // 只在生命周期已经提出统一回退时写目标相机。
    mut runtime: ResMut<RaytracingRuntime>, // 取走一次性相机快照并清空厂商诊断。
) {
    if runtime.pending_rollback.is_none() {
        return; // 稳定活动帧不查询或改写任何相机附件。
    }
    restore_dlss_camera_snapshot(&mut commands, &mut runtime); // 与独立RR关闭共用同一份精确附件恢复事务。
    runtime.dlss_rr_active = false; // 回退帧不得继续宣称目标相机正在重建降噪。
    runtime.dlss_rr_cpu_ms = None; // 旧视图Render线程CPU诊断立即失效。
    runtime.dlss_rr_gpu_ms = None; // 旧视图GPU诊断立即失效。
    runtime.dlss_rr_unavailable_reason = Some(if runtime.dlss_rr_supported {
        DlssRayReconstructionUnavailableReason::WaitingForSolariCamera // 设备仍支持，只是本会话相机正在释放。
    } else {
        DlssRayReconstructionUnavailableReason::DeviceOrBackendUnsupported // 防御启动能力资源不存在的路径。
    });
}

/// Solari因切镜、切换场景、尺寸或光照突变请求屏幕历史失效时，同帧重置RR避免错代重建。
#[cfg(feature = "dlss_rr")] // 只有真实DLSS相机组件存在时注册同步逻辑。
pub(crate) fn sync_dlss_ray_reconstruction_reset(
    runtime: Res<RaytracingRuntime>, // 只访问当前会话锁定的唯一相机和活动位。
    mut cameras: Query<(
        &SolariLighting, // Solari历史系统已经在本PostUpdate写入最终reset值。
        &mut Dlss<DlssRayReconstructionFeature>, // RR必须消费同一帧的历史失效信号。
    )>,
) {
    if !runtime.dlss_rr_active {
        return; // 无厂商降噪的AMD路径保持零查询写入。
    }
    let Some(camera) = runtime.camera else {
        return; // 相机销毁帧由Update统一回退，不扫描其他视图。
    };
    let Ok((solari, mut dlss)) = cameras.get_mut(camera) else {
        return; // Commands尚未落地或实体换代时安全等待统一生命周期处理。
    };
    if solari.reset && !dlss.reset {
        dlss.reset = true; // 只在Solari本帧真正reset且RR尚未reset时写一次，稳定帧保持零修改。
    }
}

/// 普通test_panel保留基础Solari且不链接NVIDIA SDK；空系统维持统一调度形状。
#[cfg(not(feature = "dlss_rr"))]
pub(crate) fn maintain_dlss_ray_reconstruction() {} // 构建禁用原因由运行时默认值直接发布给F1。

/// 普通test_panel没有RR输出需要暂停FXAA。
#[cfg(not(feature = "dlss_rr"))]
pub(crate) fn sync_dlss_antialiasing_requirement() {} // 基础Solari继续使用租约原有单帧FXAA。

/// 普通test_panel没有厂商相机附件需要加入回退事务。
#[cfg(not(feature = "dlss_rr"))]
pub(crate) fn restore_dlss_ray_reconstruction_on_rollback() {} // 基础Solari回退路径保持原状。

/// 普通test_panel没有第二份厂商时间历史需要同步。
#[cfg(not(feature = "dlss_rr"))]
pub(crate) fn sync_dlss_ray_reconstruction_reset() {} // 稳定帧不产生资源或组件访问。
