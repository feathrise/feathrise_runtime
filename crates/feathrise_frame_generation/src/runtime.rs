//! 主世界请求、图形附件租约、Reflex Simulation时序与设备附着。

use std::sync::{
    // RenderApp只通过固定原子反馈向主世界发布输入事实。
    Arc,                                       // 两个世界共享反馈所有权。
    atomic::{AtomicBool, AtomicU32, Ordering}, // 每帧固定小型无锁状态。
}; // 同步类型导入结束。

use ash::vk::Handle as _; // 只把VkPhysicalDevice转换成稳定u64位模式。
use bevy::{
    // Bevy插件、帧号、提取和Vulkan适配器资源。
    diagnostic::FrameCount,
    prelude::*,
    render::{
        ExtractSchedule, RenderApp,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        renderer::RenderAdapter,
    },
    window::{PresentMode, PrimaryWindow}, // FG只允许已完成非VSync切换且尺寸稳定的主交换链。
}; // Bevy导入结束。
use feathrise_scene_core::view::PrimarySceneView; // 只租赁唯一玩法主相机。
use feathrise_frame_generation_ui::{
    FrameGenerationUiBackend, FrameGenerationUiPlugin, FrameGenerationUiRequest,
    FrameGenerationUiRequestSystems,
}; // 两后端共享一次UI绘制与资格所有权。
use feathrise_settings::{
    // 唯一持久设置、DLSS真实状态和可组合附件所有权。
    DlssRuntimeStatus,
    FrameGenerationMode,
    Fsr3RuntimeStatus, // 仅跟踪实际输入换代，不作为帧生成启用依赖。
    RenderSettings,
    GraphicsRuntimeLeaseId,
    GraphicsRuntimeLeaseRegistry,
    GraphicsRuntimeLeaseSystems,
    GraphicsRuntimeRequirements,
}; // 设置域导入结束。

use crate::{
    // 本crate启动、原生和公开状态模块。
    bootstrap::BootstrapState,
    native,
    status::{FrameGenerationBlocker, FrameGenerationPhase, FrameGenerationRuntimeStatus},
}; // crate内导入结束。

const PCL_SIMULATION_START: u32 = 0; // `sl::PCLMarker::eSimulationStart`稳定值。
const PCL_SIMULATION_END: u32 = 1; // `sl::PCLMarker::eSimulationEnd`稳定值。

/// 提取到RenderApp的最终运行请求；玩家偏好与资格阻断已经在主世界解析。
#[derive(Resource, Clone, Copy, Debug, Default, ExtractResource)]
#[extract_app(RenderApp)]
#[extract_app(bevy::render::RenderApp)]
pub(crate) struct RenderRequest {
    pub(crate) enabled: bool, // 设备、窗口和唯一玩法相机就绪后独立准备帧生成输入。
    pub(crate) camera: Option<Entity>, // 精确主世界相机身份，避免辅助视图覆盖当前帧标签。
    pub(crate) input_identity: crate::types::InputIdentity, // 同尺寸超分/原生切换仍清历史。
} // RenderRequest结束。

/// RenderApp向主世界发布最近输入提交结果的共享无锁槽。
#[derive(Resource, Clone, Debug)]
pub(crate) struct RenderFeedback {
    tagged_frame: Arc<AtomicU32>, // 最近一次常量与场景、深度、运动、透明UI全部提交成功的帧号。
    has_tagged_frame: Arc<AtomicBool>, // 零号帧也必须能与“从未提交”区分。
    failed_frame: Arc<AtomicU32>, // 最近一次输入构建或SDK标签失败的帧号。
    has_failed_frame: Arc<AtomicBool>, // 与零号帧区分。
} // RenderFeedback结束。

impl Default for RenderFeedback {
    fn default() -> Self {
        Self {
            tagged_frame: Arc::new(AtomicU32::new(0)), // 初始数值无意义，由布尔位隔离。
            has_tagged_frame: Arc::new(AtomicBool::new(false)), // 尚无有效输入。
            failed_frame: Arc::new(AtomicU32::new(0)), // 初始数值无意义。
            has_failed_frame: Arc::new(AtomicBool::new(false)), // 尚无失败帧。
        }
    } // 默认反馈结束。
} // RenderFeedback默认实现结束。

impl RenderFeedback {
    pub(crate) fn mark_tagged(&self, frame: u32) {
        // RenderApp成功提交完整帧输入后调用。
        self.tagged_frame.store(frame, Ordering::Release); // 先发布帧号。
        self.has_failed_frame.store(false, Ordering::Release); // 新成功帧覆盖此前输入失败。
        self.has_tagged_frame.store(true, Ordering::Release); // 再发布可见性事实。
    } // mark_tagged结束。

    pub(crate) fn mark_failed(&self, frame: u32) {
        // 缺资源或SDK拒绝本帧时调用。
        self.failed_frame.store(frame, Ordering::Release); // 发布精确失败帧。
        self.has_failed_frame.store(true, Ordering::Release); // 发布失败事实。
    } // mark_failed结束。

    fn inputs_ready_for(&self, current_frame: u32) -> bool {
        // 主世界读取上一轮RenderApp结果。
        if !self.has_tagged_frame.load(Ordering::Acquire) {
            // 从未成功时明确未就绪。
            return false;
        } // 有效帧存在性判断结束。
        let tagged = self.tagged_frame.load(Ordering::Acquire); // 读取成功帧。
        let recently_tagged = current_frame.wrapping_sub(tagged) <= 2; // 主世界通常比RenderApp最多领先一帧，允许一个恢复边界。
        let failed_after_tag = self.has_failed_frame.load(Ordering::Acquire); // 任一更新失败都由下一成功帧显式清除。
        recently_tagged && !failed_after_tag // 只接受最近且未被后续失败覆盖的输入。
    } // inputs_ready_for结束。
} // RenderFeedback实现结束。

/// DefaultPlugins完成后建立的物理设备附着事实。
#[derive(Resource, Clone, Copy, Debug, Default)]
struct AttachmentState {
    attached: bool, // Reflex和DLSS-G能力、函数及初始关闭选项是否全部成功。
    result: i32,    // 失败时的官方结果码。
} // AttachmentState结束。

/// 当前玩法相机的FG附件租约；目标换代和资格失效都必须精确释放。
#[derive(Resource, Debug, Default)]
struct LeaseState {
    active: Option<(Entity, GraphicsRuntimeLeaseId)>, // 同一时刻最多租赁唯一玩法主相机。
} // LeaseState结束。

/// 主世界本帧资格解析结果，供状态发布保留单一主因。
#[derive(Resource, Clone, Debug, Default)]
struct EligibilityState {
    blocker: Option<FrameGenerationBlocker>, // 无阻断才允许RenderApp请求开启。
} // EligibilityState结束。

/// 只跟踪会重建主交换链的事实；光标、IME等普通Window变更不能误停FG。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WindowPresentationSignature {
    width: u32,                // 主窗口物理像素宽度。
    height: u32,               // 主窗口物理像素高度。
    present_mode: PresentMode, // VSync切换会触发交换链重建。
} // WindowPresentationSignature结束。

/// Streamline要求交换链操作期间关闭FG；变化后一整个稳定主世界帧才允许重新请求。
#[derive(Resource, Clone, Copy, Debug, Default)]
struct WindowStabilityState {
    last: Option<WindowPresentationSignature>, // 最近观察到的交换链相关窗口状态。
    unchanged_frames: u8,                      // 连续未变化帧数，饱和计数避免长期运行溢出。
} // WindowStabilityState结束。

impl WindowStabilityState {
    fn observe(&mut self, window: &Window) -> bool {
        let current = WindowPresentationSignature {
            width: window.physical_width(),
            height: window.physical_height(),
            present_mode: window.present_mode,
        }; // 只采集实际影响交换链的最小字段。
        if self.last == Some(current) {
            self.unchanged_frames = self.unchanged_frames.saturating_add(1); // 已稳定至少一帧。
        } else {
            self.last = Some(current); // 本帧先让Bevy完成交换链重建。
            self.unchanged_frames = 0;
        }
        self.unchanged_frames >= 1 // 下一帧观察到未变化时才恢复FG。
    }

    fn reset(&mut self) {
        self.last = None; // 主窗口消失后不能沿用旧交换链稳定事实。
        self.unchanged_frames = 0;
    }
} // WindowStabilityState实现结束。

fn present_mode_supports_frame_generation(mode: PresentMode) -> bool {
    // Streamline Vulkan只接受不等待垂直回扫的呈现意图。
    matches!(
        mode,
        PresentMode::AutoNoVsync | PresentMode::Immediate | PresentMode::Mailbox
    )
} // present_mode_supports_frame_generation结束。

/// DefaultPlugins之后安装的帧生成运行插件。
pub struct FrameGenerationPlugin; // 配置由RenderSettings唯一持久字段驱动。

impl Plugin for FrameGenerationPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<FrameGenerationUiPlugin>() {
            app.add_plugins(FrameGenerationUiPlugin);
        } // 双后端构建只安装一次公共UI phase和资源。
        crate::render::hudless::load_shader(app); // 内嵌GPU颜色转换，无外部资源加载。
        crate::render::prepass::load_shader(app); // 内嵌深度/运动副本着色器，原Bevy预通道不需要COPY_SRC用途。
        let feedback = RenderFeedback::default(); // 两个世界必须持有同一组Arc原子。
        app.init_resource::<AttachmentState>()
            .init_resource::<LeaseState>()
            .init_resource::<EligibilityState>()
            .init_resource::<WindowStabilityState>()
            .init_resource::<RenderRequest>()
            .insert_resource(feedback.clone())
            .add_plugins(ExtractResourcePlugin::<RenderRequest>::default()) // 每帧把最终资格请求复制到RenderApp。
            .add_systems(First, reflex_simulation_start) // sleep必须位于SimulationStart之前。
            .add_systems(
                Last,
                reflex_simulation_end.after(bevy::diagnostic::update_frame_count),
            ) // 使用递增后即将提取的帧号，与Render/Present一致。
            .add_systems(
                PostUpdate,
                maintain_request_and_lease.before(GraphicsRuntimeLeaseSystems::RestoreReleased), // 先声明或释放，再由统一画质所有权恢复/应用。
            )
            .add_systems(
                PostUpdate,
                publish_runtime_status.after(maintain_request_and_lease),
            ) // 同一调度内汇总上一轮Present证据。
            .add_systems(
                PostUpdate,
                publish_ui_request
                    .in_set(FrameGenerationUiRequestSystems::Publish)
                    .after(GraphicsRuntimeLeaseSystems::ApplyActive),
            ); // 图形租约完成后发布同一主相机的UI资格。
        app.add_systems(
            PostUpdate,
            publish_input_identity.after(GraphicsRuntimeLeaseSystems::ApplyActive),
        ); // 在相机租约收敛后提取请求及实际输入来源。

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            // DefaultPlugins存在时RenderApp已经建立。
            render_app.insert_resource(feedback); // Render系统只写共享原子，不跨世界借用。
            render_app.add_systems(
                ExtractSchedule,
                crate::render::extract_projection_parameters,
            ); // 同步唯一玩法相机近远裁面。
            crate::render::install(render_app); // 安装UI前颜色捕获、常量与资源标签系统。
        } // RenderApp安装结束。
    } // build结束。

    fn finish(&self, app: &mut App) {
        let runtime_dir = match app.world().get_resource::<BootstrapState>() {
            Some(BootstrapState::Ready { runtime_dir }) => runtime_dir.clone(), // 保留实际加载来源供诊断。
            _ => return, // 前置引导失败时不得触碰Vulkan原生适配器。
        }; // 引导资格判断结束。
        debug!("Streamline 运行库目录：{}", runtime_dir.display()); // 只在设备附着期记录一次，不进入稳定帧。
        let Some(render_app) = app.get_sub_app(RenderApp) else {
            // 无渲染后端时不能附着。
            block_attachment(app, FrameGenerationBlocker::RenderAdapterUnavailable, -1); // 公开明确原因。
            return;
        }; // RenderApp存在性结束。
        let Some(adapter) = render_app.world().get_resource::<RenderAdapter>() else {
            // RenderPlugin.finish应已解包适配器。
            block_attachment(app, FrameGenerationBlocker::RenderAdapterUnavailable, -1); // 不猜异步资源状态。
            return;
        }; // RenderAdapter存在性结束。
        let physical_device = unsafe {
            // 只读取wgpu拥有的Vulkan适配器句柄。
            adapter
                .as_hal::<wgpu_hal::vulkan::Api>()
                .map(|raw| raw.raw_physical_device().as_raw())
        }; // 原生适配器守卫在表达式末释放。
        let Some(physical_device) = physical_device else {
            // 实际后端必须与构建Vulkan约束一致。
            block_attachment(app, FrameGenerationBlocker::NonVulkanAdapter, -1); // 禁止静默走其他API。
            return;
        }; // Vulkan适配器判断结束。
        let result = native::attach_vulkan(physical_device); // 官方能力查询同时覆盖GPU、驱动、OS和HAGS。
        if result != native::RESULT_OK {
            // 不支持时不影响普通渲染。
            block_attachment(
                app,
                FrameGenerationBlocker::DeviceUnsupported { result },
                result,
            ); // 发布官方结果。
            return;
        } // 设备附着结果结束。
        // SAFETY: SL已成功附着同一Vulkan设备；bootstrap已登记设备销毁前的SL关闭回调，先释放全部FG再关闭共享NGX。
        unsafe {
            dlss_wgpu::register_external_ngx_shutdown_owner();
        } // SR/RR只释放自己的功能与参数，不能提前关闭仍被FG使用的NGX。
        app.insert_resource(AttachmentState {
            attached: true, // 五个功能入口、Reflex选项和DLSS-G关闭态已完成。
            result,         // 成功结果。
        }); // 发布主世界设备事实。
        let mut status = app
            .world_mut()
            .resource_mut::<FrameGenerationRuntimeStatus>();
        status.phase = FrameGenerationPhase::Ready; // 尚未收到玩家开启和Present证据。
        status.initialized = true; // 前置slInit保持有效。
        status.device_supported = true; // 官方适配器查询通过。
        status.streamline_result = result; // 成功结果。
        status.blocker = None; // 清理设备检查占位阻断。
        status.detail.clear(); // 清理旧SDK错误文本。
        info!("DLSS Frame Generation 设备能力检查通过，当前保持关闭"); // 文案明确不冒充2x已生效。
    } // finish结束。
} // FrameGenerationPlugin实现结束。

fn block_attachment(app: &mut App, blocker: FrameGenerationBlocker, result: i32) {
    // 统一附着失败回退。
    app.insert_resource(AttachmentState {
        attached: false, // RenderApp永不发送SDK帧输入。
        result,          // 保留官方或本地哨兵码。
    });
    let mut status = app
        .world_mut()
        .resource_mut::<FrameGenerationRuntimeStatus>();
    status.phase = FrameGenerationPhase::Blocked; // 普通渲染安全继续。
    status.device_supported = false; // 设备契约未建立。
    status.streamline_result = result; // 公开结果。
    status.blocker = Some(blocker); // 公开主因。
    status.detail = native::last_error(); // 同步复制一次原生诊断。
    warn!("DLSS Frame Generation 设备附着失败：{}", status.detail); // 一次性日志。
} // block_attachment结束。

fn reflex_simulation_start(frame: Res<FrameCount>, attachment: Res<AttachmentState>) {
    // 每个模拟帧最早边界。
    if !attachment.attached {
        // 未附着时零SDK调用。
        return;
    } // 设备资格判断结束。
    let render_frame = frame.0.wrapping_add(1); // Bevy在Last递增后才提取到RenderApp，First必须使用同一即将渲染的帧号。
    let _ = native::sleep(render_frame); // 官方要求sleep先于SimulationStart，错误由状态轮询统一显示。
    let _ = native::mark(render_frame, PCL_SIMULATION_START); // 与Last、Render提交和Present使用同一帧号。
} // reflex_simulation_start结束。

fn reflex_simulation_end(frame: Res<FrameCount>, attachment: Res<AttachmentState>) {
    // 每个模拟帧最晚主世界边界。
    if attachment.attached {
        // 未附着时保持普通运行零开销。
        let _ = native::mark(frame.0, PCL_SIMULATION_END); // Render提交标记随后由wgpu底层回调发送。
    } // 设备资格判断结束。
} // reflex_simulation_end结束。

fn maintain_request_and_lease(
    settings: Res<RenderSettings>,                  // 唯一持久玩家请求源。
    attachment: Res<AttachmentState>,             // 物理设备能力事实。
    cameras: Query<Entity, With<PrimarySceneView>>,   // 唯一玩法主相机身份。
    windows: Query<&Window, With<PrimaryWindow>>, // 必须以实际交换链状态而非仅设置草稿判定资格。
    mut request: ResMut<RenderRequest>,           // 提取到RenderApp的最终请求。
    mut eligibility: ResMut<EligibilityState>,    // 状态页单一主因。
    mut window_stability: ResMut<WindowStabilityState>, // 尺寸/VSync变化期间先关闭FG一个渲染帧。
    mut lease_state: ResMut<LeaseState>,          // 精确释放旧目标租约。
    mut leases: ResMut<GraphicsRuntimeLeaseRegistry>, // 统一图形附件所有权。
) {
    let requested = settings.graphics.frame_generation_mode == FrameGenerationMode::Dlss2x; // 只有启动选择DLSS-G时请求当前后端。
    let mut camera_iter = cameras.iter(); // 主相机集合应为零或一。
    let camera = camera_iter.next().filter(|_| camera_iter.next().is_none()); // 多个Marker同样视为身份不明确。
    let window = windows.single().ok(); // 无窗口或多主窗口都不能猜测Present目标。
    let window_stable = window.is_some_and(|window| window_stability.observe(window));
    if window.is_none() {
        window_stability.reset(); // 下次主窗口出现必须重新经过稳定边界。
    }
    let blocker = if !requested {
        None // 玩家关闭时不是错误。
    } else if !attachment.attached {
        Some(FrameGenerationBlocker::DeviceUnsupported {
            result: attachment.result,
        }) // 前置状态中仍保留更具体启动主因。
    } else if camera.is_none() {
        Some(FrameGenerationBlocker::NoGameplayCamera) // 菜单和加载期必须关闭FG并清标签。
    } else if window.is_none() {
        Some(FrameGenerationBlocker::NoPrimaryWindow) // Present目标不唯一时不得向SDK提交交换链输入。
    } else if settings.graphics.vsync
        || !window.is_some_and(|window| present_mode_supports_frame_generation(window.present_mode))
    {
        Some(FrameGenerationBlocker::RequiresVsyncOff) // Streamline 2.12 Vulkan路径明确不支持VSync。
    } else if !window_stable {
        Some(FrameGenerationBlocker::WindowTransition) // 本帧让Bevy先完成交换链尺寸/模式切换。
    } else {
        None // 允许RenderApp在完整输入就绪后开启2x。
    }; // 本帧资格解析结束。
    eligibility.blocker = blocker.clone(); // 状态发布读取同一结论。
    request.enabled = requested && blocker.is_none(); // 不把不可执行偏好发送给SDK。
    request.camera = request.enabled.then_some(camera).flatten(); // 清空停用或非唯一相机身份。

    let desired_camera = request.enabled.then_some(camera).flatten(); // 只有最终资格成立才持有附件。
    if lease_state.active.map(|(entity, _)| entity) != desired_camera {
        // 目标换代或资格改变时更新所有权。
        if let Some((_, lease)) = lease_state.active.take() {
            leases.release(lease); // 先释放旧相机，统一画质系统同帧恢复作者状态。
        } // 旧租约释放结束。
        if let Some(camera) = desired_camera {
            let requirements = GraphicsRuntimeRequirements::frame_generation(camera); // 与FSR-G复用HDR、单采样、FXAA托底及深度/MV要求，不依赖超分租约。
            let lease = leases.acquire(requirements); // 与SR/Solari要求按或合并。
            lease_state.active = Some((camera, lease)); // 保存稳定票据供切换和退出释放。
        } // 新租约登记结束。
    } // 租约目标同步结束。
} // maintain_request_and_lease结束。

fn publish_input_identity(
    settings: Res<RenderSettings>,  // 请求用于提前识别同尺寸渲染方式切换。
    dlss: Res<DlssRuntimeStatus>, // 真实回退会在后续帧再次撤销旧来源历史。
    fsr: Res<Fsr3RuntimeStatus>,  // FSR无需成为DLSS帧生成的启动依赖。
    mut request: ResMut<RenderRequest>, // 输入来源随既有请求一次提取。
) {
    let graphics = &settings.graphics; // 只读取唯一持久设置。
    let identity = crate::types::InputIdentity {
        dlss: graphics.dlss_mode,                // DLAA与原生即使同尺寸也区分。
        fsr: graphics.fsr_mode,                  // 厂商切换不能混用历史。
        antialiasing: graphics.anti_aliasing,    // 原生AA选择换代。
        raytracing: graphics.raytracing_enabled, // 光栅/光追选择换代。
        reconstruction: graphics.ray_reconstruction_enabled, // SR/RR选择换代。
        effective_path: dlss.path,               // 输入延迟生效或失败回退仍重置。
        effective_fsr: fsr.effective_mode,       // FSR实际上下文换代。
    };
    if request.input_identity != identity {
        request.input_identity = identity;
    } // 稳定帧不制造变更。
}

fn publish_ui_request(
    request: Res<RenderRequest>,
    attachment: Res<AttachmentState>,
    mut ui: ResMut<FrameGenerationUiRequest>,
) {
    // UI仅消费后端已证明的主相机资格。
    ui.update_for(
        FrameGenerationUiBackend::Dlss,
        request.enabled,
        request.camera,
        attachment.attached,
    ); // 关闭DLSS时只释放自身所有权，不覆盖FSR请求。
} // 公共UI资格发布结束。

fn publish_runtime_status(
    settings: Res<RenderSettings>,        // 显示原始玩家请求，即使当前受阻。
    attachment: Res<AttachmentState>,   // 设备附着事实。
    eligibility: Res<EligibilityState>, // 主世界资格主因。
    feedback: Res<RenderFeedback>,      // 上一轮RenderApp输入证据。
    frame: Res<FrameCount>,             // 判断输入是否仍属于最近渲染帧。
    mut status: ResMut<FrameGenerationRuntimeStatus>, // 只在快照真实变化时写入。
) {
    if !status.initialized || !attachment.attached {
        // 前置或设备阻断由对应阶段保留更具体原因。
        status.requested = settings.graphics.frame_generation_mode == FrameGenerationMode::Dlss2x; // 仍显示当前后端偏好。
        return;
    } // 生命周期资格判断结束。
    let native = native::status(); // 固定小型原子快照，不调用昂贵VRAM估算。
    let requested = settings.graphics.frame_generation_mode == FrameGenerationMode::Dlss2x; // 当前后端的原始持久请求。
    let inputs_ready = feedback.inputs_ready_for(frame.0); // RenderApp上一轮完整输入事实。
    let blocker = if !requested {
        None // 玩家关闭是正常Ready状态。
    } else if let Some(blocker) = eligibility.blocker.clone() {
        Some(blocker) // 设备、窗口或主相机主因优先，超分不再是资格条件。
    } else if native.dlssg_status & (1 << 0) != 0 {
        Some(FrameGenerationBlocker::ResolutionTooLow {
            minimum: native.min_dimension,
        }) // 官方分辨率下限。
    } else if native.dlssg_status & (1 << 1) != 0 {
        Some(FrameGenerationBlocker::ReflexNotDetected) // Present未观察到匹配标记。
    } else if native.dlssg_status & (1 << 3) != 0 {
        Some(FrameGenerationBlocker::CommonConstantsInvalid) // 矩阵/深度/MV常量错误。
    } else if !inputs_ready {
        Some(FrameGenerationBlocker::InputsUnavailable) // 首帧或资源切换等待完整标签。
    } else if native.last_result != native::RESULT_OK {
        Some(FrameGenerationBlocker::RuntimeFailure {
            result: native.last_result,
            status_bits: native.dlssg_status,
        }) // 其他SDK调用失败。
    } else if native.dlssg_status != 0 {
        Some(FrameGenerationBlocker::RuntimeFailure {
            result: native.last_result,
            status_bits: native.dlssg_status,
        }) // HDR或未来状态位同样不得忽略。
    } else {
        None // 运行契约完整。
    }; // 运行阻断解析结束。
    let phase = if !requested {
        FrameGenerationPhase::Ready // 设备可用但玩家关闭。
    } else if native.effective != 0 && native.frames_presented >= 2 && blocker.is_none() {
        FrameGenerationPhase::Active2x // 只有Present真实证据成立。
    } else if eligibility.blocker.is_some() {
        FrameGenerationPhase::Blocked // 稳定资格阻断。
    } else {
        FrameGenerationPhase::Requested // 输入预热或等待下一次Present确认。
    }; // 阶段解析结束。
    let detail = if blocker.is_some() {
        native::last_error()
    } else {
        String::new()
    }; // 仅阻断时复制文本。
    let next = FrameGenerationRuntimeStatus {
        phase,
        requested,
        initialized: native.initialized != 0,
        device_supported: native.attached != 0,
        inputs_ready,
        frames_presented: native.frames_presented, // SDK零计数意味着没有呈现证据，不能在主世界再次伪造成一帧。
        total_present_count: native.total_present_count, // 只转发Present回调已经累计的事实。
        presentation_epoch: native.presentation_epoch, // 保留完整采样代次。
        presentation_ready: native.presentation_ready != 0, // 不增加SDK查询或主线程等待。
        maximum_generated_frames: native.max_generated_frames,
        minimum_dimension: native.min_dimension,
        estimated_vram_bytes: native.estimated_vram,
        streamline_result: native.last_result,
        dlssg_status_bits: native.dlssg_status,
        blocker,
        detail,
    }; // 完整公开快照结束。
    if *status != next {
        // 稳定帧不触发UI变更检测。
        *status = next; // 原子替换主世界值快照。
    } // 状态变化判断结束。
} // publish_runtime_status结束。

#[cfg(test)]
mod tests; // 资格、输入来源与独立租约回归。
