//! RenderApp中的无HUD颜色捕获、公共常量、资源标签与固定2x切换。
//!
//! 【功能设计】捕获系统位于三维后处理之后、Bevy UI之前。每个Render系统拥有独立命令缓冲，
//! 场景、深度和运动先提交；共享UI完成后才提交独立Alpha标签并开启插值，真实帧始终恢复完整UI。

mod constants;
pub(crate) mod hudless; // GPU编码为交换链色彩空间。
pub(crate) mod prepass; // GPU转存预通道，使SDK立即复制符合Vulkan用途要求。 // 公共常量及历史数学，独立于SDK资源标签。
use constants::{build_constants, constants_are_finite, next_history}; // 保持原有数学契约。
mod ui; // 后端薄适配只登记共享透明层，不重复维护二维或世界UI绘制。

use ash::vk::{self, Handle as _}; // 只读取原生句柄、格式、布局和用途位模式。
use bevy::{
    // RenderApp调度、视图、预通道、纹理缓存与主世界提取。
    app::SubApp,
    camera::{MainPassResolutionOverride, NormalizedRenderTarget}, // 内部尺寸与实际窗口交换链规格分别验证。
    core_pipeline::{
        Core3dSystems,
        prepass::ViewPrepassTextures,
        schedule::Core3d,
        upscaling::{ViewUpscalingPipeline, upscaling}, // 最终输出管线也属于完整帧资格。
    },
    diagnostic::FrameCount,
    prelude::*,
    render::{
        Extract,
        Render,
        RenderSystems,
        camera::{ExtractedCamera, TemporalJitter}, // 当前输出窗口与深度采样偏移。
        render_resource::{PipelineCache, TextureUsages},
        renderer::{RenderContext, ViewQuery},
        sync_world::MainEntity,
        view::{ExtractedView, ExtractedWindow, ViewTarget}, // 读取实际交换链图像格式，不能误用sRGB视图格式。
    },
    window::PrimaryWindow,
}; // Bevy导入结束。
use feathrise_scene_core::view::PrimarySceneView; // 从主世界提取唯一玩法投影参数。
use feathrise_frame_generation_ui::{
    FrameGenerationUiSystems, UiFramePermission, WorldUiFrameGate, WorldUiLayer,
}; // 同帧世界UI分离门及独立UI事务排序。

use crate::{
    // 原生POD与两个世界共享请求/反馈。
    native::{self, NativeTexture},
    runtime::{RenderFeedback, RenderRequest},
}; // crate内导入结束。

/// 主世界玩法投影中无法仅从无限反向Z矩阵恢复的作者参数。
#[derive(Resource, Clone, Copy, Debug)]
pub(crate) struct ProjectionParameters {
    valid: bool,       // 只有唯一透视PrimarySceneView才允许FG。
    near: f32,         // 作者近裁面。
    far: f32,          // 作者可见性远裁面。
    fov: f32,          // 垂直FOV弧度。
    aspect_ratio: f32, // Camera系统解析后的宽高比。
} // ProjectionParameters结束。

impl Default for ProjectionParameters {
    fn default() -> Self {
        Self {
            valid: false,                      // 未提取到玩法透视相机时阻断输入。
            near: 0.1,                         // 有限安全值但不会用于无效帧。
            far: 1000.0,                       // Bevy默认可见性远裁面。
            fov: core::f32::consts::FRAC_PI_4, // Bevy默认垂直FOV。
            aspect_ratio: 1.0,                 // 尺寸未解析时的有限值。
        }
    } // 默认投影结束。
} // ProjectionParameters默认实现结束。

/// 从主世界唯一玩法相机提取作者投影参数到RenderApp。
pub(crate) fn extract_projection_parameters(
    mut commands: Commands, // 向RenderWorld写本帧小型值快照。
    cameras: Extract<Query<&Projection, With<PrimarySceneView>>>, // 不依赖RenderEntity映射。
) {
    let mut iter = cameras.iter(); // 严格要求零或唯一。
    let first = iter.next(); // 读取候选。
    let unique = first.filter(|_| iter.next().is_none()); // 多Marker时拒绝猜目标。
    let parameters = match unique {
        Some(Projection::Perspective(perspective))
            if perspective.near.is_finite()
                && perspective.near > 0.0
                && perspective.far.is_finite()
                && perspective.far > perspective.near
                && perspective.fov.is_finite()
                && perspective.fov > 0.0
                && perspective.aspect_ratio.is_finite()
                && perspective.aspect_ratio > 0.0 =>
        {
            ProjectionParameters {
                valid: true, // 全部作者参数满足Streamline有限值契约。
                near: perspective.near,
                far: perspective.far,
                fov: perspective.fov,
                aspect_ratio: perspective.aspect_ratio,
            }
        }
        _ => ProjectionParameters::default(), // 菜单、正交、自定义或非法投影安全阻断。
    }; // 投影解析结束。
    commands.insert_resource(parameters); // 每帧覆盖固定32字节以内快照。
} // extract_projection_parameters结束。

/// 一组尺寸和格式完全一致的DLSS-G选项身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OptionSignature {
    camera: Entity,                              // 同尺寸相机换代也必须清空时间历史。
    input_identity: crate::types::InputIdentity, // DLAA、原生及其他超分同尺寸切换也重新预热。
    jittered: bool,      // 实际抖动来源出现/消失使旧历史失效，不纳入逐帧offset。
    output_width: u32,   // 交换链宽度。
    output_height: u32,  // 交换链高度。
    output_format: u32,  // 交换链VkFormat。
    input_width: u32,    // 深度/运动矢量宽度。
    input_height: u32,   // 深度/运动矢量高度。
    depth_format: u32,   // 深度VkFormat。
    motion_format: u32,  // 运动矢量VkFormat。
    hudless_format: u32, // UI前颜色VkFormat。
    ui_format: u32,      // 共享透明层与窗口一致的sRGB格式。
} // OptionSignature结束。

impl OptionSignature {
    fn set_enabled(self, enabled: bool) -> i32 {
        // 保证开启和关闭提交同一完整规格。
        native::set_enabled(
            enabled,
            self.output_width,
            self.output_height,
            self.output_format,
            self.input_width,
            self.input_height,
            self.depth_format,
            self.motion_format,
            self.hudless_format,
            self.ui_format, // SDK选项包含真实UI格式。
        )
    } // set_enabled结束。
} // OptionSignature实现结束。

/// RenderApp拥有的切换与预热状态；尺寸变化先关FG再重新积累一帧输入。
#[derive(Resource, Debug, Default)]
struct RuntimeState {
    signature: Option<OptionSignature>, // 最近成功解析的完整资源规格。
    plugin_enabled: bool,               // 最近一次`slDLSSGSetOptions(On)`是否成功。
    warmup_complete: bool,              // 同规格至少已有一帧完整标签。
    tags_valid: bool,                   // 是否需要在停用时发送空标签撤销资源。
    prepared: Option<(u32, Entity)>,    // 场景标签只构成本帧中间结果，UI完成前不发布成功。
    failure_site: Option<(&'static str, u32)>, // 相同输入缺项只记录一次，避免稳定失败逐帧刷日志。
    failure_frame: Option<u32>,         // 同帧UI收尾不能覆盖场景阶段更早的实际失败点。
} // RuntimeState结束。

/// 上一帧无抖动相机状态，用于构造裁剪空间时间变换和断续重置。
#[derive(Resource, Clone, Copy, Debug, Default)]
struct CameraHistory {
    clip_from_world: Mat4, // 上一帧无抖动世界到裁剪矩阵。
    position: Vec3,        // 上一帧世界位置。
    projection: Mat4,      // 上一帧无抖动投影。
    output_size: UVec2,    // 上一帧最终输出尺寸。
    valid: bool,           // 首帧或停用后为假。
} // CameraHistory结束。

/// 安装Render调度和Core3d UI前捕获系统。
pub(crate) fn install(render_app: &mut SubApp) {
    hudless::install(render_app); // 固定两条异步颜色转换管线。
    prepass::install(render_app); // 固定单条深度/运动MRT管线。
    ui::install(render_app); // UI完成后登记SDK标签，最后由公共层恢复真实帧。
    render_app
        .init_resource::<ProjectionParameters>()
        .init_resource::<RuntimeState>()
        .init_resource::<CameraHistory>()
        .add_systems(
            Render,
            (
                publish_driver_frame,    // 所有wgpu提交回调必须先看到本帧编号。
                disable_without_request, // 菜单、加载、关闭或资格失效时在RenderGraph前关FG并清标签。
            )
                .chain()
                .after(RenderSystems::ExtractCommands) // 提取命令已应用，才能读取本帧真实窗口与请求。
                .before(RenderSystems::PrepareAssets) // 上传资源的队列提交也必须看到新帧号。
                .before(RenderSystems::PrepareMeshes) // 必须早于PrepareViews；Prepare已经晚于交换链配置。
                .before(bevy::render::view::window::create_surfaces) // 先关闭旧FG，再创建或恢复Surface。
                .before(bevy::render::view::window::prepare_windows), // 在configure/Acquire之前完成关闭和标签撤销。
        )
        .add_systems(
            Core3d,
            capture_and_tag
                .in_set(FrameGenerationUiSystems::Prepare) // 完整场景准备必须先于共享UI分离。
                .after(Core3dSystems::PostProcess) // 色调映射与三维后处理必须已经完成。
                .before(bevy::ui_render::ui_pass) // Bevy HUD不得进入插值颜色。
                .before(upscaling), // 标签命令必须先于最终交换链写入与Present。
        );
} // install结束。

fn publish_driver_frame(frame: Res<FrameCount>) {
    // RenderGraph开始前固定小型原子写。
    native::set_driver_frame(frame.0); // 队列提交和Present回调使用同一显式编号。
} // publish_driver_frame结束。

fn disable_without_request(
    request: Res<RenderRequest>,   // 主世界已经解析设备、DLSS和玩法相机资格。
    frame: Res<FrameCount>,        // 空标签也绑定明确帧号。
    feedback: Res<RenderFeedback>, // 关闭后当前帧不再被视为输入就绪。
    windows: Query<&ExtractedWindow, With<PrimaryWindow>>, // 直接读取本次提取的变化，覆盖主世界资格系统之后才发生的resize。
    mut state: ResMut<RuntimeState>, // 保存最近规格以完整关闭。
    mut history: ResMut<CameraHistory>, // 停用后下一玩法帧必须重置历史。
) {
    let window_transition = windows
        .single()
        .ok()
        .is_none_or(|window| window.size_changed || window.present_mode_changed); // 缺窗、尺寸与呈现模式变化都不能沿用旧交换链FG。
    if request.enabled
        && !window_transition
        && !(state.plugin_enabled && native::status().requested == 0)
    {
        // 原生Present安全门未关闭SDK时，有资格请求由Core3d继续。
        return;
    } // 请求资格判断结束。
    if state.plugin_enabled {
        if let Some(signature) = state.signature {
            let _ = signature.set_enabled(false); // 先关插件，再允许菜单/窗口改变交换链。
        } // 有完整规格时提交正式关闭选项。
    } // 插件关闭结束。
    if state.tags_valid {
        let _ = native::clear_tags(frame.0); // 撤销Vulkan资源引用，禁止Present复用玩法旧帧。
    } // 标签清理结束。
    feedback.mark_failed(frame.0); // 主世界状态不再把上一玩法帧视为当前输入。
    *state = RuntimeState::default(); // 下一次重新预热并提交完整规格。
    *history = CameraHistory::default(); // 下一帧公共常量显式reset。
} // disable_without_request结束。

#[allow(clippy::too_many_arguments)] // View系统逐项读取公开渲染资源，拆成隐藏查询会削弱输入契约可审计性。
fn capture_and_tag(
    view: ViewQuery<(
        &ViewTarget,
        &ExtractedView,
        Option<&ViewPrepassTextures>,
        Option<&WorldUiLayer>,
        Option<&MainPassResolutionOverride>,
        Option<&TemporalJitter>,
        &ExtractedCamera,
        Option<&ViewUpscalingPipeline>,
    )>, // 输入和最终输出必须同时就绪。
    windows: Query<(MainEntity, &ExtractedWindow)>, // 已配置的真实交换链图像，和渲染用sRGB视图区别对待。
    world_ui_gate: Res<WorldUiFrameGate>, // 世界标签没有完整分离时禁止插值带UI场景。
    mut ui_permission: ResMut<UiFramePermission>, // 场景标签成功才允许本帧透明UI事务。
    request: Res<RenderRequest>,    // 最终资格请求。
    projection: Res<ProjectionParameters>, // 主世界作者近远裁面与FOV。
    frame: Res<FrameCount>,         // 常量、标签和Reflex共用帧号。
    feedback: Res<RenderFeedback>,  // 向主世界发布成功/失败事实。
    pipelines: Res<hudless::HudlessPipelines>, // 固定格式管线异步准备。
    pipeline_cache: Res<PipelineCache>, // 未就绪时非阻塞回退。
    mut capture: ResMut<hudless::HudlessCapture>, // 单张持久颜色目标。
    prepass_pipelines: Res<prepass::PrepassPipelines>, // 两项预通道共用单次GPU遍历。
    mut prepass_capture: ResMut<prepass::PrepassCapture>, // 最近内部尺寸的唯一副本组。
    mut state: ResMut<RuntimeState>, // 尺寸切换和一帧预热状态。
    mut history: ResMut<CameraHistory>, // 当前到上一裁剪空间变换。
    mut context: RenderContext,     // 复制和SDK易失资源命令写入同一命令缓冲。
) {
    if !request.enabled {
        // Render调度关闭系统已经清理，不在每个三维辅助视图重复调用。
        return;
    } // 请求判断结束。
    let (
        target,
        extracted_view,
        prepass,
        world_ui_layer,
        resolution_override,
        jitter,
        camera,
        output_pipeline,
    ) = view.into_inner(); // 当前视图固定读取一次。
    let owner = extracted_view.retained_view_entity.main_entity.id(); // 主世界与渲染世界使用明确映射。
    if request.camera != Some(owner) {
        return;
    } // 辅助相机不得取消或覆盖主相机事务。
    state.prepared = None; // 本帧场景尚未完成，不能沿用上次成功。
    ui_permission.deny(); // 当前主视图重新建立完整许可。
    if world_ui_gate.required
        && (world_ui_gate.frame != frame.0
            || world_ui_gate.camera != Some(owner)
            || !world_ui_gate.split
            || world_ui_layer.is_none_or(|layer| {
                layer.frame != frame.0
                    || layer.camera != owner
                    || !layer.is_drawn()
                    || layer.has_failed()
            }))
    {
        // 遵守既有世界UI原子分离门。
        fail_frame(frame.0, &feedback, &mut state, &mut history);
        return; // 普通绘制回退继续保留世界标签。
    } // 世界UI完整性门结束。
    if !projection.valid
        || output_pipeline.is_none_or(|pipeline| !pipeline.is_ready(&pipeline_cache))
    {
        // 最终管线未编译时只有清帧输出，不能用完整场景标签冒充可插值的真实帧。
        // 投影无效时撤销旧帧；窗口输出资格由下方out_texture和精确相机身份验证。
        fail_frame(frame.0, &feedback, &mut state, &mut history);
        return;
    } // 玩法窗口视图判断结束。
    let Some(prepass) = prepass else {
        // 租约尚未完成提取时不得复用上一帧标签。
        fail_frame(frame.0, &feedback, &mut state, &mut history);
        return;
    }; // 预通道组件存在性结束。
    let Some(output_view) = target.out_texture() else {
        // 只有真实交换链视图可启用FG。
        fail_frame(frame.0, &feedback, &mut state, &mut history);
        return;
    }; // 输出视图存在性结束。

    let source = target.main_texture(); // 三维后处理完成且UI尚未绘制的当前主颜色。
    let source_size = source.size(); // 通常等于最终输出物理尺寸。
    if source_size.width == 0 || source_size.height == 0 || source_size.depth_or_array_layers != 1 {
        fail_frame(frame.0, &feedback, &mut state, &mut history); // 最小化窗口或非二维目标不得开启。
        return;
    } // 主颜色尺寸判断结束。
    let Some(hudless) = hudless::encode(
        &mut context,
        target,
        &pipelines,
        &pipeline_cache,
        &mut capture,
    ) else {
        fail_frame(frame.0, &feedback, &mut state, &mut history);
        return; // 转换管线或格式未就绪时保留完整普通渲染。
    };

    let render_size = resolution_override
        .map_or(UVec2::new(source_size.width, source_size.height), |size| {
            size.0
        }); // 原始预通道整窗分配，SDK只消费内部有效区域。
    let Some(inputs) = prepass::encode(
        &mut context,
        prepass,
        render_size,
        &prepass_pipelines,
        &pipeline_cache,
        &mut prepass_capture,
    ) else {
        fail_frame(frame.0, &feedback, &mut state, &mut history); // 缺输入、无效尺寸或异步管线未就绪时保留普通帧。
        return;
    }; // 两项副本完成且已转换为只读布局。
    let Some(depth) = native_texture(
        &inputs.depth.texture,      // R32Float独立颜色载体保留原始反向Z值。
        &inputs.depth.default_view, // SDK可合法复制的同格式视图。
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, // GPU转存后已显式转换。
        vk::ImageAspectFlags::COLOR, // 载体是颜色图像，不能误标原深度aspect。
    ) else {
        fail_frame(frame.0, &feedback, &mut state, &mut history);
        return;
    }; // 深度原生描述结束。
    let Some(motion) = native_texture(
        &inputs.motion.texture,      // 精确内部尺寸的运动副本。
        &inputs.motion.default_view, // 与R32深度同时录制完成。
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        vk::ImageAspectFlags::COLOR,
    ) else {
        fail_frame(frame.0, &feedback, &mut state, &mut history);
        return;
    }; // 运动矢量原生描述结束。
    let Some(hudless_native) = native_texture(
        &hudless.texture,
        &hudless.default_view,
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        vk::ImageAspectFlags::COLOR,
    ) else {
        fail_frame(frame.0, &feedback, &mut state, &mut history);
        return;
    }; // 无HUD颜色原生描述结束。
    let Some(output_view_format) = raw_view_format(output_view) else {
        // 验证代理图像只使用自身合法UNORM视图，独立UI与HUDless仍用自有sRGB纹理。
        fail_frame(frame.0, &feedback, &mut state, &mut history);
        return;
    }; // 输出格式结束。
    let output_format = match camera.target.as_ref() {
        // 仅接受SDK支持的真实UNORM窗口图像，不改写原生格式身份。
        Some(NormalizedRenderTarget::Window(target)) => windows
            .iter()
            .find(|(main_entity, _)| *main_entity == target.entity())
            .and_then(|(_, window)| window.swap_chain_texture_format), // 相机目标保留主世界窗口身份。
        _ => None, // 离屏纹理不能作为DLSS-G交换链。
    }; // 当前已配置图像规格。
    let output_format = match output_format {
        // 最终pass显式编码sRGB，SDK接收真实UNORM图像。
        Some(wgpu::TextureFormat::Bgra8Unorm) => vk::Format::B8G8R8A8_UNORM.as_raw() as u32, // BGRA窗口。
        Some(wgpu::TextureFormat::Rgba8Unorm) => vk::Format::R8G8B8A8_UNORM.as_raw() as u32, // RGBA窗口。
        _ => {
            fail_frame(frame.0, &feedback, &mut state, &mut history);
            return;
        } // 未通过中间件格式协商时保持普通渲染。
    }; // 交换链图像格式门结束。
    if output_view_format != output_format {
        fail_frame(frame.0, &feedback, &mut state, &mut history);
        return;
    } // SL代理图像没有MUTABLE_FORMAT能力，禁止重解释为sRGB视图。
    let ui_format = hudless_native.format; // 共享UI和HUDless均从窗口通道选择自有sRGB纹理，最终UI标签仍独立验证真实格式。
    if depth.width != motion.width || depth.height != motion.height {
        fail_frame(frame.0, &feedback, &mut state, &mut history); // 深度与运动矢量必须逐像素对应。
        return;
    } // 预通道尺寸一致性结束。
    let signature = OptionSignature {
        camera: owner,                          // 实体换代与资源规格一起使旧历史失效。
        input_identity: request.input_identity, // 复用原有规格失效事务关闭、清标签并重新预热。
        jittered: jitter.is_some(),             // 无超分原生路径使用零抖动；不自行制造时间采样。
        output_width: source_size.width,
        output_height: source_size.height,
        output_format,
        input_width: depth.width,
        input_height: depth.height,
        depth_format: depth.format,
        motion_format: motion.format,
        hudless_format: hudless_native.format,
        ui_format, // 独立UI的真实sRGB纹理格式，不与UNORM交换链视图混淆。
    }; // 当前完整规格结束。

    if state.signature != Some(signature) {
        // 尺寸、DLSS档位、HDR或格式改变都先关闭并重新预热。
        if state.plugin_enabled {
            if let Some(previous) = state.signature {
                let _ = previous.set_enabled(false); // 旧交换链规格仍有效时先关。
            } // 旧规格存在性结束。
        } // 旧插件关闭结束。
        if state.tags_valid {
            let _ = native::clear_tags(frame.0);
        } // 撤销旧身份全部标签。
        let _ = signature.set_enabled(false); // 新规格先建立明确关闭态。
        state.signature = Some(signature); // 保存新身份。
        state.plugin_enabled = false; // 不在换规格首帧插值。
        state.warmup_complete = false; // 至少积累一帧完整输入。
        state.tags_valid = false; // 新标签尚未提交。
        history.valid = false; // 公共常量必须reset。
    } // 规格切换结束。

    let constants = build_constants(
        extracted_view,
        *projection,
        source_size,
        *history,
        jitter.map_or(Vec2::ZERO, |jitter| -jitter.offset),
    ); // 与SR共享真实像素抖动约定，运动矢量无抖动不代表深度无抖动。
    if !constants_are_finite(&constants)
        || native::set_constants(frame.0, &constants) != native::RESULT_OK
    {
        fail_frame(frame.0, &feedback, &mut state, &mut history); // 缺公共常量时绝不进入Present插值。
        return;
    } // 公共常量提交结束。

    context.command_encoder().transition_resources(
        core::iter::empty(),
        core::iter::once(wgpu::TextureTransition {
            texture: &*hudless.texture,
            selector: None,
            state: wgpu::TextureUses::RESOURCE,
        }),
    ); // 显式从颜色写入转换为SDK只读布局。
    state.tags_valid = true; // SDK可能部分提交后失败，失败路径必须撤销全部标签。
    let result = record_native(&mut context, "dlss_fg_scene_tags", |command_buffer| {
        // 原生标签使用独立编码器，保留 wgpu 的资源跟踪和命令顺序。
        native::tag_inputs(
            frame.0,
            command_buffer,
            &depth,
            &motion,
            &hudless_native,
            signature.output_width,
            signature.output_height,
        ) // SDK复制紧随GPU转换和布局屏障，仍在同一渲染队列有序提交。
    }); // 原生缓冲通过RenderContext追加，不自行提交队列。
    if result != Some(native::RESULT_OK) {
        // 原生后端缺失或SDK失败均撤销事务。
        fail_frame(frame.0, &feedback, &mut state, &mut history); // SDK标签失败立即关闭。
        return;
    } // 三项输入标签结束。
    state.tags_valid = true; // 场景标签已经存在，后续UI失败必须连同它们一起撤销。
    state.prepared = Some((frame.0, owner)); // 最终成功等待同帧UI标签，不提前开启SDK。
    ui_permission.allow(frame.0, owner); // 公共捕获层只接受本次完整场景准备。
    *history = next_history(extracted_view, source_size); // 只有完整标签成功才推进相机历史。
} // capture_and_tag结束。

fn record_native(
    context: &mut RenderContext,
    label: &'static str,
    record: impl FnOnce(u64) -> i32,
) -> Option<i32> {
    // 单独编码原生命令，沿用Bevy队列和帧内提交顺序。
    let mut encoder = context
        .render_device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) }); // 该编码器只使用HAL接口。
    let result = unsafe {
        encoder.as_hal_mut::<wgpu_hal::vulkan::Api, _, _>(|encoder| {
            encoder.map(|encoder| record(encoder.raw_command_buffer().as_raw()))
        })
    }; // SDK录制只在有效原生借用内发生。
    context.add_command_buffer(encoder.finish()); // 先结束前序wgpu编码器，再追加原生缓冲；后续UI继续使用新的wgpu编码器。
    result // 不等待GPU，不创建独立队列或跨帧命令流。
} // 原生命令桥结束。

#[track_caller] // 保留最先失败的准确契约位置，不把统一回退函数当作根因。
fn fail_frame(
    frame: u32,                  // 当前明确帧号。
    feedback: &RenderFeedback,   // 发布失败事实。
    state: &mut RuntimeState,    // 关闭当前插件与标签。
    history: &mut CameraHistory, // 下一帧强制reset。
) {
    if state.failure_frame != Some(frame) {
        // 每帧只接受第一次失败，后续UI收尾保留原始原因。
        let caller = std::panic::Location::caller(); // 静态源码身份，无分配或堆栈扫描。
        let site = (caller.file(), caller.line()); // 只保存固定小型身份。
        if state.failure_site != Some(site) {
            warn!(
                "DLSS帧生成输入未就绪：{}:{}，SDK结果={}，状态位=0x{:08X}",
                site.0,
                site.1,
                native::status().last_result,
                native::status().dlssg_status
            );
        } // 契约失败点变化时才格式化日志。
        state.failure_site = Some(site); // 稳定缺项不重复输出。
        state.failure_frame = Some(frame); // UI收尾不得覆盖本帧较早错误。
    } // 输入失败诊断结束。
    if state.plugin_enabled {
        if let Some(signature) = state.signature {
            let _ = signature.set_enabled(false); // 任何输入缺失都先退回普通Present。
        } // 规格存在性结束。
    } // 插件关闭结束。
    if state.tags_valid {
        let _ = native::clear_tags(frame); // 禁止复用上一成功帧资源。
    } // 标签撤销结束。
    feedback.mark_failed(frame); // 主世界公开输入未就绪。
    state.plugin_enabled = false; // 清理运行事实。
    state.warmup_complete = false; // 恢复后重新预热一帧。
    state.tags_valid = false; // 当前无有效标签。
    state.prepared = None; // 失败不能遗留可被UI收尾采用的中间结果。
    history.valid = false; // 当前到上一帧关系断裂。
} // fail_frame结束。

fn native_texture(
    texture: &bevy::render::render_resource::Texture, // wgpu拥有的图像。
    view: &bevy::render::render_resource::TextureView, // 与图像匹配的完整二维视图。
    layout: vk::ImageLayout,                          // 当前命令点已知布局。
    aspect: vk::ImageAspectFlags,                     // 颜色或深度子资源。
) -> Option<NativeTexture> {
    let image = {
        // wgpu图像和视图共用不可重入snatch锁，必须依次释放HAL守卫。
        let raw = unsafe { texture.as_hal::<wgpu_hal::vulkan::Api>()? }; // 后端不符时安全返回空。
        unsafe { raw.raw_handle().as_raw() } // 外层Texture强引用持续保证底层图像存活。
    }; // 释放图像守卫后才能借用视图。
    let (image_view, format) = {
        // 同一视图守卫一次读取两个只读值。
        let raw = unsafe { view.as_hal::<wgpu_hal::vulkan::Api>()? }; // 视图必须来自同一Vulkan设备。
        unsafe { (raw.raw_handle().as_raw(), raw.raw_format().as_raw() as u32) } // 外层TextureView强引用保证视图存活。
    }; // 不把HAL锁带入SDK调用或下一次原生借用。
    let size = texture.size(); // wgpu公开创建尺寸。
    Some(NativeTexture {
        image,                          // 已释放HAL守卫的只读图像身份。
        view: image_view,               // 已释放HAL守卫的只读视图身份。
        layout: layout.as_raw() as u32, // VkImageLayout数值。
        width: size.width,
        height: size.height,
        format, // 视图实际VkFormat。
        mip_levels: texture.mip_level_count(),
        array_layers: size.depth_or_array_layers,
        usage: map_image_usage(texture.usage(), aspect).as_raw(),
        aspect_mask: aspect.as_raw(),
    }) // 原生描述结束。
} // native_texture结束。

fn raw_view_format(view: &bevy::render::render_resource::TextureView) -> Option<u32> {
    // 读取真实交换链视图格式。
    let raw = unsafe { view.as_hal::<wgpu_hal::vulkan::Api>()? }; // 非Vulkan后端阻断。
    Some(unsafe { raw.raw_format().as_raw() as u32 }) // 返回VkFormat位模式。
} // raw_view_format结束。

fn map_image_usage(usage: TextureUsages, aspect: vk::ImageAspectFlags) -> vk::ImageUsageFlags {
    // WebGPU创建用途映射到Vulkan描述。
    let mut flags = vk::ImageUsageFlags::empty(); // 从无用途开始逐位追加。
    if usage.contains(TextureUsages::COPY_SRC) {
        flags |= vk::ImageUsageFlags::TRANSFER_SRC;
    }
    if usage.contains(TextureUsages::COPY_DST) {
        flags |= vk::ImageUsageFlags::TRANSFER_DST;
    }
    if usage.contains(TextureUsages::TEXTURE_BINDING) {
        flags |= vk::ImageUsageFlags::SAMPLED;
    }
    if usage.contains(TextureUsages::STORAGE_BINDING) {
        flags |= vk::ImageUsageFlags::STORAGE;
    }
    if usage.contains(TextureUsages::RENDER_ATTACHMENT) {
        flags |= if aspect.intersects(vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL) {
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
        } else {
            vk::ImageUsageFlags::COLOR_ATTACHMENT
        };
    } // 渲染附件映射结束。
    flags // 返回精确VkImageUsageFlags。
} // map_image_usage结束。

#[cfg(test)] // 纯矩阵和用途映射测试不启动GPU。
mod tests; // 保留原测试与注释，降低主渲染事务文件复杂度。
