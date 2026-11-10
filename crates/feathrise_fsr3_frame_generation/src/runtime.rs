//! 主世界FSR 3帧生成资格、图形附件租约、设备附着与真实Present状态汇总。

use std::sync::{
    Arc,                                       // 主世界与RenderApp共享固定原子反馈所有权。
    atomic::{AtomicBool, AtomicU32, Ordering}, // 每帧只读写小型无锁事实。
}; // 同步类型导入结束。

use ash::vk::Handle as _; // Vulkan句柄只转换成稳定u64位模式。
use bevy::{
    camera::RenderTarget,   // 资格阶段必须排除小场景和预览RTT相机。
    diagnostic::FrameCount, // 判断RenderApp输入是否仍属于最近帧。
    prelude::*,             // ECS资源、查询、插件安装和日志。
    render::{
        ExtractSchedule, RenderApp,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        renderer::RenderDevice,
    }, // RenderWorld请求提取与wgpu设备访问。
    window::{PresentMode, PrimaryWindow, WindowRef}, // 跟踪主窗口与相机输出身份。
}; // Bevy导入结束。
use feathrise_scene_core::view::PrimarySceneView; // 只租赁唯一玩法主相机。
use feathrise_settings::{
    FrameGenerationMode, RenderSettings, GraphicsRuntimeLeaseId, GraphicsRuntimeLeaseRegistry,
    GraphicsRuntimeLeaseSystems, GraphicsRuntimeRequirements,
}; // 唯一持久设置和可组合图形附件所有权。

use crate::{
    bootstrap::BootstrapState, // 只有完整前置引导才读取Vulkan设备。
    native,                    // 设备附着和低频状态快照。
    status::{
        Fsr3FrameGenerationBlocker, Fsr3FrameGenerationInputFailure, Fsr3FrameGenerationPhase,
        Fsr3FrameGenerationRuntimeStatus,
    },
}; // crate内导入结束。

const REQUIRED_QUEUE_COUNT: u32 = 4; // 官方Vulkan代理交换链的硬要求。

/// 提取到RenderApp的最终运行请求；资格阻断已在主世界解析。
#[derive(Resource, Clone, Copy, Debug, Default, ExtractResource)]
#[extract_app(RenderApp)]
#[extract_app(bevy::render::RenderApp)]
pub(crate) struct RenderRequest {
    pub(crate) enabled: bool, // 只有设备、唯一窗口/相机和稳定交换链同时满足才为真。
    pub(crate) camera: Option<Entity>, // RenderApp必须精确匹配同一主世界玩法相机。
} // RenderRequest结束。

/// RenderApp向主世界发布最近Prepare结果的共享无锁槽。
#[derive(Resource, Clone, Debug)]
pub(crate) struct RenderFeedback {
    prepared_frame: Arc<AtomicU32>,      // 最近完整FSR Prepare帧号。
    has_prepared_frame: Arc<AtomicBool>, // 零号帧与“从未成功”区分。
    failed_frame: Arc<AtomicU32>,        // 最近缺输入或SDK失败帧号。
    has_failed_frame: Arc<AtomicBool>,   // 与零号帧区分。
    input_failure: Arc<AtomicU32>,       // 精确RenderApp缺项的稳定枚举编码。
} // RenderFeedback结束。

impl Default for RenderFeedback {
    fn default() -> Self {
        Self {
            prepared_frame: Arc::new(AtomicU32::new(0)), // 初始数值由存在位隔离。
            has_prepared_frame: Arc::new(AtomicBool::new(false)), // 尚无成功输入。
            failed_frame: Arc::new(AtomicU32::new(0)),   // 初始数值无意义。
            has_failed_frame: Arc::new(AtomicBool::new(false)), // 尚无失败输入。
            input_failure: Arc::new(AtomicU32::new(
                Fsr3FrameGenerationInputFailure::Pending.code(),
            )), // 首个渲染帧到达前显示预热而非猜测缺项。
        }
    } // 默认反馈结束。
} // RenderFeedback默认实现结束。

impl RenderFeedback {
    pub(crate) fn mark_prepared(&self, frame: u32) {
        self.prepared_frame.store(frame, Ordering::Release); // 先发布精确帧号。
        self.input_failure.store(
            Fsr3FrameGenerationInputFailure::Pending.code(),
            Ordering::Release,
        ); // 成功帧清除旧缺项。
        self.has_failed_frame.store(false, Ordering::Release); // 新成功覆盖旧失败。
        self.has_prepared_frame.store(true, Ordering::Release); // 最后发布可见性。
    } // mark_prepared结束。

    pub(crate) fn mark_failed(&self, frame: u32, reason: Fsr3FrameGenerationInputFailure) {
        self.input_failure.store(reason.code(), Ordering::Release); // 先写精确缺项。
        self.failed_frame.store(frame, Ordering::Release); // 保存精确失败帧。
        self.has_failed_frame.store(true, Ordering::Release); // 发布失败事实。
    } // mark_failed结束。

    fn input_failure(&self) -> Fsr3FrameGenerationInputFailure {
        Fsr3FrameGenerationInputFailure::from_code(self.input_failure.load(Ordering::Acquire))
    } // input_failure结束。

    fn inputs_ready_for(&self, current_frame: u32) -> bool {
        if !self.has_prepared_frame.load(Ordering::Acquire)
            || self.has_failed_frame.load(Ordering::Acquire)
        {
            return false; // 从未成功或后续失败均不可复用旧输入。
        }
        current_frame.wrapping_sub(self.prepared_frame.load(Ordering::Acquire)) <= 2 // 主世界与RenderApp最多容忍一个恢复边界。
    } // inputs_ready_for结束。
} // RenderFeedback实现结束。

/// DefaultPlugins完成后建立的Vulkan四队列附着事实。
#[derive(Resource, Clone, Debug, Default)]
struct AttachmentState {
    attached: bool,                              // 原生桥是否接受设备和四条不同队列。
    result: i32,                                 // 最近附着结果。
    blocker: Option<Fsr3FrameGenerationBlocker>, // 启动阶段更精确的主因。
} // AttachmentState结束。

/// 当前玩法相机的FG附件租约。
#[derive(Resource, Debug, Default)]
struct LeaseState {
    active: Option<(Entity, GraphicsRuntimeLeaseId)>, // 同时最多租赁唯一玩法相机。
} // LeaseState结束。

/// 主世界本帧资格结论，状态汇总复用同一主因。
#[derive(Resource, Clone, Debug, Default)]
struct EligibilityState {
    blocker: Option<Fsr3FrameGenerationBlocker>, // 无阻断才允许RenderApp Prepare。
} // EligibilityState结束。

/// 只跟踪真正导致交换链重建的窗口字段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WindowPresentationSignature {
    width: u32,                // 主窗口物理宽度。
    height: u32,               // 主窗口物理高度。
    present_mode: PresentMode, // VSync/低延迟Present模式变化。
} // WindowPresentationSignature结束。

/// 窗口规格变化后一整个稳定主世界帧才恢复Prepare。
#[derive(Resource, Clone, Copy, Debug, Default)]
struct WindowStabilityState {
    last: Option<WindowPresentationSignature>, // 最近交换链相关快照。
    unchanged_frames: u8,                      // 饱和计数防止长期运行溢出。
} // WindowStabilityState结束。

impl WindowStabilityState {
    fn observe(&mut self, window: &Window) -> bool {
        let current = WindowPresentationSignature {
            width: window.physical_width(),
            height: window.physical_height(),
            present_mode: window.present_mode,
        }; // 固定小型值快照。
        if self.last == Some(current) {
            self.unchanged_frames = self.unchanged_frames.saturating_add(1); // 稳定帧推进。
        } else {
            self.last = Some(current); // 本帧留给Bevy/FFX完成交换链重建。
            self.unchanged_frames = 0;
        }
        self.unchanged_frames >= 1 && current.width > 0 && current.height > 0 // 最小化窗口不得Prepare。
    } // observe结束。

    fn reset(&mut self) {
        self.last = None; // 新窗口必须重新经历稳定边界。
        self.unchanged_frames = 0;
    } // reset结束。
} // WindowStabilityState实现结束。

/// 由lib.rs中的运行插件调用，安装主世界与RenderApp系统。
pub(crate) fn install(app: &mut App) {
    if !app.is_plugin_added::<feathrise_frame_generation_ui::FrameGenerationUiPlugin>() {
        app.add_plugins(feathrise_frame_generation_ui::FrameGenerationUiPlugin);
    } // 两个后端共用唯一UI所有者。
    let feedback = RenderFeedback::default(); // 两个世界必须共享同一组Arc原子。
    app.init_resource::<AttachmentState>()
        .init_resource::<LeaseState>()
        .init_resource::<EligibilityState>()
        .init_resource::<WindowStabilityState>()
        .init_resource::<RenderRequest>()
        .insert_resource(feedback.clone())
        .add_plugins(ExtractResourcePlugin::<RenderRequest>::default()) // 每帧复制最终资格与唯一主相机身份。
        .add_systems(
            PostUpdate,
            maintain_request_and_lease.before(GraphicsRuntimeLeaseSystems::RestoreReleased), // 先声明或释放，再由统一画质所有权恢复/应用。
        )
        .add_systems(
            PostUpdate,
            publish_runtime_status.after(maintain_request_and_lease),
        ); // 同帧汇总上一轮Present证据。

    app.add_systems(
        PostUpdate,
        publish_ui_request
            .in_set(feathrise_frame_generation_ui::FrameGenerationUiRequestSystems::Publish)
            .after(GraphicsRuntimeLeaseSystems::ApplyActive),
    ); // 图形租约落地后发布精确UI资格。
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.insert_resource(feedback); // Render系统只写共享原子。
        render_app.add_systems(ExtractSchedule, crate::render::extract_frame_parameters); // 同步相机和真实帧间隔。
        crate::render::install(render_app); // 安装UI前深度/运动Prepare节点。
    } // RenderApp安装结束。
} // install结束。

/// 由lib.rs插件finish阶段调用，在Surface首次配置前附着真实wgpu Vulkan设备。
pub(crate) fn finish(app: &mut App) {
    let runtime_directory = match app.world().get_resource::<BootstrapState>() {
        Some(BootstrapState::Ready { runtime_directory }) => runtime_directory.clone(), // 保留准确来源供一次性日志。
        _ => return, // 前置失败已经发布更具体阻断。
    }; // 启动资格结束。
    debug!("FidelityFX 帧生成运行目录：{}", runtime_directory.display()); // 不进入稳定帧。
    let Some(render_app) = app.get_sub_app(RenderApp) else {
        block_attachment(app, Fsr3FrameGenerationBlocker::RenderDeviceUnavailable, -1);
        return;
    }; // RenderApp存在性结束。
    let Some(render_device) = render_app.world().get_resource::<RenderDevice>() else {
        block_attachment(app, Fsr3FrameGenerationBlocker::RenderDeviceUnavailable, -1);
        return;
    }; // RenderDevice存在性结束。

    let handles = unsafe {
        render_device
            .wgpu_device()
            .as_hal::<wgpu_hal::vulkan::Api>()
            .map(|raw| {
                let queues = raw
                    .raw_queues()
                    .iter()
                    .map(|queue| queue.queue.as_raw())
                    .collect::<Vec<_>>(); // finish期一次有界小分配，稳定帧零分配。
                let queue_families = raw
                    .raw_queues()
                    .iter()
                    .map(|queue| queue.family_index)
                    .collect::<Vec<_>>(); // 角色可跨族，避免要求AMD在通用族暴露四条队列。
                (
                    raw.raw_physical_device().as_raw(),
                    raw.raw_device().handle().as_raw(),
                    queues,
                    queue_families,
                )
            })
    }; // wgpu HAL守卫在表达式末释放。
    let Some((physical_device, device, queues, queue_families)) = handles else {
        block_attachment(app, Fsr3FrameGenerationBlocker::NonVulkanDevice, -1);
        return;
    }; // Vulkan后端判断结束。
    if queues.len() < REQUIRED_QUEUE_COUNT as usize {
        block_attachment(
            app,
            Fsr3FrameGenerationBlocker::QueueUnavailable {
                available: queues.len() as u32,
                required: REQUIRED_QUEUE_COUNT,
            },
            3,
        ); // provider is_available保持假，wgpu创建普通交换链。
        return;
    } // 队列数量判断结束。
    let result = native::attach_vulkan(physical_device, device, &queues, &queue_families); // 原生层再次验证四个角色、族号和句柄两两一致。
    if result != native::RESULT_OK {
        block_attachment(
            app,
            Fsr3FrameGenerationBlocker::DeviceAttachFailed { result },
            result,
        ); // 普通交换链继续。
        return;
    } // 原生附着结果结束。

    app.insert_resource(AttachmentState {
        attached: true, // Surface配置时provider查询将返回真。
        result,
        blocker: None,
    }); // 发布主世界资格。
    let mut status = app
        .world_mut()
        .resource_mut::<Fsr3FrameGenerationRuntimeStatus>();
    status.phase = Fsr3FrameGenerationPhase::Ready; // 尚未收到完整玩法Prepare和Present证据。
    status.runtime_loaded = true;
    status.device_attached = true;
    status.available_queues = queues.len() as u32;
    status.fidelityfx_result = result;
    status.blocker = None;
    status.detail.clear();
    info!(
        "FSR 3.1 帧生成 Vulkan 设备附着完成：game/compute/present/acquire 队列族 {:?}，独立队列 {} 条，当前保持普通 Present",
        queue_families,
        queues.len(),
    ); // 文案明确未宣称2x生效。
} // finish结束。

fn block_attachment(app: &mut App, blocker: Fsr3FrameGenerationBlocker, result: i32) {
    app.insert_resource(AttachmentState {
        attached: false,
        result,
        blocker: Some(blocker.clone()),
    }); // RenderApp永不请求Prepare。
    let detail = native::last_error(); // finish边界同步复制一次诊断。
    let mut status = app
        .world_mut()
        .resource_mut::<Fsr3FrameGenerationRuntimeStatus>();
    status.phase = Fsr3FrameGenerationPhase::Blocked;
    status.device_attached = false;
    status.fidelityfx_result = result;
    status.blocker = Some(blocker);
    status.detail = detail.clone();
    warn!("FSR 3.1 帧生成设备附着失败：{detail}"); // 一次性日志。
} // block_attachment结束。

#[allow(clippy::too_many_arguments)] // 各资源分别表达设置、窗口、相机、资格和所有权，合并会隐藏审计边界。
fn maintain_request_and_lease(
    settings: Res<RenderSettings>,      // 唯一持久玩家请求源。
    attachment: Res<AttachmentState>, // 设备和队列事实。
    cameras: Query<(Entity, &Camera, &RenderTarget), (With<PrimarySceneView>, With<Camera3d>)>, // 唯一、激活且输出到主窗口的玩法主相机。
    windows: Query<&Window, With<PrimaryWindow>>, // 唯一Present目标。
    mut request: ResMut<RenderRequest>,           // 提取到RenderApp的最终请求。
    mut eligibility: ResMut<EligibilityState>,    // 状态页主因。
    mut stability: ResMut<WindowStabilityState>,  // 交换链变化保护。
    mut lease_state: ResMut<LeaseState>,          // 精确释放旧相机租约。
    mut leases: ResMut<GraphicsRuntimeLeaseRegistry>, // 可组合附件所有权。
    mut last_logged_state: Local<Option<(bool, bool, Option<Fsr3FrameGenerationBlocker>)>>, // 只记录资格变化。
) {
    let requested = settings.graphics.frame_generation_mode == FrameGenerationMode::Fsr3_2x; // 只接受当前后端枚举。
    let mut camera_iter = cameras.iter(); // 主相机集合通常为零或一。
    let unique_camera = camera_iter.next().filter(|_| camera_iter.next().is_none()); // 多Marker时拒绝猜测。
    let (has_unique_camera, camera) = match unique_camera {
        Some((entity, camera, target)) => (
            true,
            (camera.is_active && matches!(target, RenderTarget::Window(WindowRef::Primary)))
                .then_some(entity), // 精确限定主窗口，误带Marker的RTT也不取得代理交换链。
        ),
        None => (false, None), // 零个和多个Marker均属于身份不唯一。
    }; // 玩法相机目标验证结束。
    let window = windows.single().ok(); // 多主窗口同样阻断。
    let window_stable = window.is_some_and(|window| stability.observe(window));
    if window.is_none() {
        stability.reset(); // 新窗口重新经过稳定边界。
    }
    let blocker = if !requested {
        None // 玩家关闭不是错误。
    } else if !attachment.attached {
        attachment
            .blocker
            .clone()
            .or(Some(Fsr3FrameGenerationBlocker::DeviceAttachFailed {
                result: attachment.result,
            }))
    } else if !has_unique_camera {
        Some(Fsr3FrameGenerationBlocker::NoGameplayCamera)
    } else if camera.is_none() {
        Some(Fsr3FrameGenerationBlocker::GameplayCameraNotPresenting)
    } else if window.is_none() {
        Some(Fsr3FrameGenerationBlocker::NoPrimaryWindow)
    } else if !window_stable {
        Some(Fsr3FrameGenerationBlocker::WindowTransition)
    } else {
        None // FSR FG与原生、DLSS SR/RR、FSR SR均可独立组合。
    }; // 本帧资格结束。
    eligibility.blocker = blocker.clone();
    request.enabled = requested && blocker.is_none();
    request.camera = request.enabled.then_some(camera).flatten(); // 同帧提取唯一权威相机身份。
    let log_state = (requested, request.enabled, blocker.clone()); // 固定小型状态，稳定帧不输出日志。
    if last_logged_state.as_ref() != Some(&log_state) {
        info!(
            "FSR 3.1 帧生成资格变化：requested={}，enabled={}，blocker={:?}",
            requested, request.enabled, blocker
        ); // 启动、进图、窗口重建和恢复均留下单一原因。
        *last_logged_state = Some(log_state);
    }

    let desired_camera = request.camera; // 请求和租约必须指向同一台玩法相机。
    if lease_state.active.map(|(entity, _)| entity) != desired_camera {
        if let Some((_, lease)) = lease_state.active.take() {
            leases.release(lease); // 同帧归还旧相机作者状态。
        }
        if let Some(camera) = desired_camera {
            let requirements = GraphicsRuntimeRequirements::frame_generation(camera); // 两后端共用独立输入契约，保持原有FSR-G行为与可组合租约优先级。
            let lease = leases.acquire(requirements); // 与DLSS/FSR SR、RR和Solari按或合并。
            lease_state.active = Some((camera, lease));
        } // 新租约登记结束。
    } // 租约目标同步结束。
} // maintain_request_and_lease结束。

fn publish_runtime_status(
    settings: Res<RenderSettings>,        // 显示当前持久请求。
    attachment: Res<AttachmentState>,   // 设备资格。
    eligibility: Res<EligibilityState>, // 当前主世界阻断。
    feedback: Res<RenderFeedback>,      // 最近RenderApp Prepare事实。
    frame: Res<FrameCount>,             // 反馈新鲜度。
    mut status: ResMut<Fsr3FrameGenerationRuntimeStatus>, // UI公开快照。
) {
    let requested = settings.graphics.frame_generation_mode == FrameGenerationMode::Fsr3_2x;
    status.requested = requested;
    if !attachment.attached {
        return; // 启动/设备阶段保留更具体的阻断和诊断。
    }
    let native = native::status(); // 固定POD原子复制，无GPU查询和等待。
    status.runtime_loaded = native.runtime_loaded != 0;
    status.device_attached = native.attached != 0;
    status.swapchain_ready = native.swapchain_ready != 0;
    status.context_ready = native.context_ready != 0;
    status.inputs_ready = feedback.inputs_ready_for(frame.0);
    status.frames_presented = native.last_present_delta.max(1);
    status.total_present_count = native.last_present_count;
    status.available_queues = native.available_queues;
    status.display_width = native.display_width;
    status.display_height = native.display_height;
    status.fidelityfx_result = native.last_result;

    let blocker = if !requested {
        None // 关闭时Ready且普通Present。
    } else if let Some(blocker) = eligibility.blocker.clone() {
        Some(blocker)
    } else if native.swapchain_ready == 0 || native.context_ready == 0 {
        Some(Fsr3FrameGenerationBlocker::SwapchainUnavailable)
    } else if native.last_result != native::RESULT_OK {
        Some(Fsr3FrameGenerationBlocker::RuntimeFailure {
            result: native.last_result,
        })
    } else if !status.inputs_ready {
        Some(Fsr3FrameGenerationBlocker::InputsUnavailable {
            reason: feedback.input_failure(), // 不再把十二种可操作缺项折叠成一句模糊文案。
        })
    } else {
        None
    }; // 单一主因解析结束。
    status.blocker = blocker;
    status.phase = if !requested {
        Fsr3FrameGenerationPhase::Ready
    } else if status.blocker.is_some() {
        Fsr3FrameGenerationPhase::Blocked
    } else if native.effective != 0 && status.frames_presented >= 2 {
        Fsr3FrameGenerationPhase::Active2x
    } else {
        Fsr3FrameGenerationPhase::Requested
    }; // DLL/请求不冒充真实2x。
    if status.blocker.is_some() && native.last_result != native::RESULT_OK {
        status.detail = native::last_error(); // 只在错误状态低频刷新文本。
    } else if status.blocker.is_none() {
        status.detail.clear(); // 成功恢复清除过期错误。
    }
} // publish_runtime_status结束。

#[cfg(test)] // 纯窗口稳定状态测试不启动GPU。
mod tests {
    use super::*;

    #[test]
    fn resize_requires_one_complete_unchanged_frame() {
        let mut state = WindowStabilityState::default();
        let mut window = Window::default();
        window.resolution.set_physical_resolution(1920, 1080);
        assert!(!state.observe(&window)); // 首次观察留给交换链创建。
        assert!(state.observe(&window)); // 下一帧同规格才稳定。
        window.resolution.set_physical_resolution(3840, 2160);
        assert!(!state.observe(&window)); // 4K重建帧先关闭FG。
        assert!(state.observe(&window)); // 新代理稳定后恢复。
    } // 测试结束。
} // 测试模块结束。

fn publish_ui_request(
    request: Res<RenderRequest>,
    mut shared: ResMut<feathrise_frame_generation_ui::FrameGenerationUiRequest>,
) {
    // 不活动的FSR只释放自己拥有的请求。
    shared.update_for(
        feathrise_frame_generation_ui::FrameGenerationUiBackend::Fsr3,
        request.enabled,
        request.camera,
        request.enabled && native::status().swapchain_ready != 0,
    ); // 不用上一帧Prepare成功作资格，避免预热门互等。
} // 公共UI请求发布结束。
