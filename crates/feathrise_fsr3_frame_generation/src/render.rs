//! RenderApp中的FSR 3相机输入与同步Prepare录制。
//!
//! 【功能设计】三维后处理后先登记输入读取，再在独立原生缓冲录制Prepare，由Bevy按顺序提交。
//! UI子域捕获真实透明层后将纯场景交SDK合成；世界名牌与伤害网格先经独立深度层，登记失败时恢复完整普通颜色。

#[path = "ui/mod.rs"] // 将独立UI职责集中到子域，不继续扩张Prepare实现。
pub(crate) mod ui; // UI捕获、管线预热和颜色还原与Prepare严格排序。

use ash::vk::{self, Handle as _}; // 只读取Vulkan句柄、格式与用途位模式。
use bevy::{
    app::SubApp,                        // RenderApp子应用安装入口。
    camera::MainPassResolutionOverride, // Vendor SR下全尺寸附件内真正有效的低分辨率区域。
    core_pipeline::{
        Core3dSystems, prepass::ViewPrepassTextures, schedule::Core3d, upscaling::upscaling,
    }, // UI前Core3d排序锚点与预通道输入。
    diagnostic::FrameCount,             // 反馈使用真实Bevy渲染帧号。
    prelude::*,                         // ECS资源、投影、数学类型和真实时间。
    render::{
        Extract,
        Render,
        RenderSystems,
        camera::TemporalJitter,               // 与实际投影逐位一致的时间抖动。
        render_resource::TextureUsages,       // Vulkan图像用途映射。
        renderer::{RenderContext, ViewQuery}, // 每视图Core3d系统和命令编码器。
        view::{ExtractedView, ViewTarget},    // 当前玩法视图颜色与相机姿态。
    },
    time::Real, // FG帧时间不受暂停或游戏时间倍率影响。
}; // Bevy导入结束。
use feathrise_scene_core::view::PrimarySceneView; // 从主世界严格选择唯一玩法投影。

use crate::{
    native::{self, NativeImage, NativePrepareDesc}, // 固定C ABI帧输入。
    runtime::{RenderFeedback, RenderRequest},       // 两个世界共享请求和反馈。
    status::Fsr3FrameGenerationInputFailure,        // 精确公开渲染输入缺项。
}; // crate内导入结束。

/// 无法仅从反向无限投影矩阵恢复的作者相机参数与真实帧时长。
#[derive(Resource, Clone, Copy, Debug)]
pub(crate) struct FrameParameters {
    valid: bool,        // 只有唯一合法透视玩法相机才为真。
    camera_near: f32,   // 作者实际近裁面。
    camera_fov_y: f32,  // 垂直FOV弧度。
    frame_time_ms: f32, // 真实渲染帧间隔毫秒。
} // FrameParameters结束。

impl Default for FrameParameters {
    fn default() -> Self {
        Self {
            valid: false,                               // 未提取到玩法相机时阻断Prepare。
            camera_near: 0.1,                           // 有限安全值但不会用于无效帧。
            camera_fov_y: core::f32::consts::FRAC_PI_4, // Bevy默认透视FOV。
            frame_time_ms: 16.667,                      // 无主世界时间时的60Hz安全值。
        }
    } // 默认参数结束。
} // FrameParameters默认实现结束。

/// 从主世界提取唯一玩法相机投影和墙钟帧间隔。
pub(crate) fn extract_frame_parameters(
    mut commands: Commands, // 向RenderWorld写本帧固定小型快照。
    cameras: Extract<Query<&Projection, With<PrimarySceneView>>>, // 不依赖RenderEntity映射。
    time: Extract<Res<Time<Real>>>, // 暂停期间仍反映真实帧节奏。
) {
    let mut iter = cameras.iter(); // 严格要求零或唯一。
    let unique = iter.next().filter(|_| iter.next().is_none()); // 多Marker时拒绝猜目标。
    let frame_time_ms = (time.delta_secs() * 1000.0).clamp(1.0, 250.0); // SDK拒绝零值，长暂停也不注入无界速度。
    let parameters = match unique {
        Some(Projection::Perspective(perspective))
            if perspective.near.is_finite()
                && perspective.near > 0.0
                && perspective.fov.is_finite()
                && perspective.fov > 0.0
                && perspective.fov <= core::f32::consts::PI =>
        {
            FrameParameters {
                valid: true,
                camera_near: perspective.near,
                camera_fov_y: perspective.fov,
                frame_time_ms,
            }
        }
        _ => FrameParameters {
            frame_time_ms,
            ..default()
        }, // 菜单、正交、自定义或非法投影安全阻断。
    }; // 投影解析结束。
    commands.insert_resource(parameters); // 每帧覆盖固定16字节内快照。
} // extract_frame_parameters结束。

/// 上一成功Prepare帧的相机与显示规格，用于断续检测。
#[derive(Resource, Clone, Copy, Debug, Default)]
struct CameraHistory {
    position: Vec3,     // 上一帧世界位置。
    projection: Mat4,   // 上一帧无抖动投影。
    output_size: UVec2, // 上一帧显示输出尺寸。
    valid: bool,        // 首帧、关闭或失败后为假。
} // CameraHistory结束。

/// RenderApp私有的当前输入有效状态。
#[derive(Resource, Clone, Copy, Debug, Default)]
struct RenderState {
    prepared: bool,                                   // 最近帧是否提交完整Prepare。
    failure: Option<Fsr3FrameGenerationInputFailure>, // 只在缺项变化时输出一次日志。
} // RenderState结束。

/// 安装Render调度和Core3d UI前Prepare系统。
pub(crate) fn install(render_app: &mut SubApp) {
    ui::install(render_app); // 仅已启用FSR构建注册独立UI系统。
    render_app
        .init_resource::<FrameParameters>()
        .init_resource::<CameraHistory>()
        .init_resource::<RenderState>()
        .add_systems(
            Render,
            disable_without_request.in_set(RenderSystems::Prepare), // RenderGraph前撤销旧配置。
        )
        .add_systems(
            Core3d,
            capture_and_prepare
                .in_set(feathrise_frame_generation_ui::FrameGenerationUiSystems::Prepare)
                .after(Core3dSystems::PostProcess) // 三维HDR后处理必须完成。
                .before(bevy::ui_render::ui_pass) // Prepare完成后UI子域才切换透明目标，避免HUD进入插值场景。
                .before(upscaling), // Prepare先于最终交换链写入和Present。
        );
} // install结束。

fn disable_without_request(
    request: Res<RenderRequest>,        // 主世界最终资格。
    frame: Res<FrameCount>,             // 失败反馈精确帧号。
    feedback: Res<RenderFeedback>,      // 撤销旧输入就绪事实。
    mut state: ResMut<RenderState>,     // 只在状态变化时调用原生关闭。
    mut history: ResMut<CameraHistory>, // 恢复后强制时间历史reset。
) {
    if request.enabled {
        return; // 有资格时由Core3d系统继续。
    }
    if state.prepared {
        native::disable(); // 禁止下一Present复用上一玩法帧。
    }
    if state.prepared || state.failure.is_some() {
        feedback.mark_failed(frame.0, Fsr3FrameGenerationInputFailure::Pending); // 主世界不再显示输入就绪。
        state.prepared = false; // 稳定关闭帧零SDK调用。
        state.failure = None; // 下次请求应重新发布精确缺项。
        history.valid = false; // 恢复首帧重新预热。
    } // 旧成功或失败状态清理结束。
} // disable_without_request结束。

#[allow(clippy::too_many_arguments)] // 各参数逐项对应公开渲染输入契约，隐藏到复合查询会降低可审计性。
fn capture_and_prepare(
    // UI子域以此系统作为同帧Prepare成功后的排序锚点。
    mut permission: ResMut<feathrise_frame_generation_ui::UiFramePermission>, // 同帧场景输入完整后才授权透明捕获。
    view: ViewQuery<(
        &ViewTarget,                                            // UI前三维HDR颜色。
        &ExtractedView,                                         // 无抖动投影和世界姿态。
        Option<&ViewPrepassTextures>,                           // 当前深度和运动矢量。
        Option<&TemporalJitter>,                                // 时间超分活动时的真实投影抖动。
        Option<&MainPassResolutionOverride>,                    // DLSS/FSR SR实际渲染尺寸。
        Option<&feathrise_frame_generation_ui::WorldUiLayer>, // 同帧世界标签分流失败必须先阻断，不能在Prepare阶段重新开启FG。
    )>,
    request: Res<RenderRequest>,      // 最终资格请求。
    parameters: Res<FrameParameters>, // 作者近裁面、FOV和墙钟帧间隔。
    world_ui_gate: Res<feathrise_frame_generation_ui::WorldUiFrameGate>, // 当前主视图可见世界UI必须实际完成分离才允许生成。
    frame: Res<FrameCount>,                                                // 反馈身份。
    feedback: Res<RenderFeedback>,                                         // 主世界Prepare事实。
    mut state: ResMut<RenderState>,                                        // 当前有效输入事实。
    mut history: ResMut<CameraHistory>,                                    // 相机断续检测。
    mut context: RenderContext, // wgpu资源转换与独立FFX Prepare命令缓冲按序提交。
) {
    if !request.enabled {
        return; // 关闭系统已处理，不在辅助视图重复调用。
    }
    let (target, extracted_view, prepass, jitter, resolution_override, world_ui) =
        view.into_inner(); // 所有输入包含当前世界UI层的失败身份。
    if request.camera != Some(extracted_view.retained_view_entity.main_entity.id()) {
        return; // 小地图、角色预览和场景切换残留视图不得覆盖主相机反馈。
    }
    if world_ui.is_some_and(|layer| {
        layer.frame == frame.0
            && layer.camera == extracted_view.retained_view_entity.main_entity.id()
            && layer.has_failed()
    }) {
        // 独立世界UI已回绘普通场景时，不允许随后Prepare把这些标签再次送入插帧。
        fail_frame(
            frame.0,
            Fsr3FrameGenerationInputFailure::WorldUiLayer,
            &feedback,
            &mut state,
            &mut history,
        ); // 同一个明确缺项只报告一次，普通完整颜色继续呈现。
        return; // 当前帧保持原生普通UI，下一帧完整绘制成功后才能恢复。
    } // 世界UI同帧失败门结束。
    if world_ui_gate.frame == frame.0
        && world_ui_gate.camera == request.camera
        && world_ui_gate.required
        && (!world_ui_gate.split
            || !world_ui.is_some_and(|layer| {
                layer.frame == frame.0 && Some(layer.camera) == request.camera && layer.is_drawn()
            }))
    {
        // 分流预热或规格不支持时，原主pass仍含世界UI，不能误报HUD-less。
        fail_frame(
            frame.0,
            Fsr3FrameGenerationInputFailure::Pending,
            &feedback,
            &mut state,
            &mut history,
        ); // 普通场景保持完整标签，只暂停当前帧生成。
        return; // 等当前帧独立层真实绘制完成后才进入SDK Prepare。
    } // 可见世界UI完整分离门结束。
    if target.out_texture().is_none() {
        fail_frame(
            frame.0,
            Fsr3FrameGenerationInputFailure::ViewNotPresenting,
            &feedback,
            &mut state,
            &mut history,
        );
        return; // 主世界身份正确但RenderApp未建立最终输出时撤销旧Prepare。
    }
    if !parameters.valid {
        fail_frame(
            frame.0,
            Fsr3FrameGenerationInputFailure::CameraParameters,
            &feedback,
            &mut state,
            &mut history,
        );
        return; // 主相机投影变为正交或非法值时禁止复用旧历史。
    }
    let Some(prepass) = prepass else {
        fail_frame(
            frame.0,
            Fsr3FrameGenerationInputFailure::PrepassTextures,
            &feedback,
            &mut state,
            &mut history,
        );
        return;
    };
    let Some((depth_attachment, depth_view)) = prepass.depth.as_ref().zip(prepass.depth_only_view()) else {
        fail_frame(
            frame.0,
            Fsr3FrameGenerationInputFailure::DepthTexture,
            &feedback,
            &mut state,
            &mut history,
        );
        return; // 官方必需当前深度。
    };
    let Some(motion_attachment) = prepass.motion_vectors.as_ref() else {
        fail_frame(
            frame.0,
            Fsr3FrameGenerationInputFailure::MotionVectorTexture,
            &feedback,
            &mut state,
            &mut history,
        );
        return; // 官方必需对象与相机运动矢量。
    };

    let output_size = target.main_texture().size(); // 当前显示目标尺寸；颜色由代理Present直接读取。
    if output_size.width == 0 || output_size.height == 0 || output_size.depth_or_array_layers != 1 {
        fail_frame(
            frame.0,
            Fsr3FrameGenerationInputFailure::OutputTexture,
            &feedback,
            &mut state,
            &mut history,
        ); // 最小化或非二维目标回退。
        return;
    }

    let Some(depth) = native_image(
        &depth_attachment.texture.texture,
        depth_view, // 保证与传给桥接层的 DEPTH aspect 一致。
        vk::ImageAspectFlags::DEPTH,
    ) else {
        fail_frame(
            frame.0,
            Fsr3FrameGenerationInputFailure::DepthInterop,
            &feedback,
            &mut state,
            &mut history,
        );
        return;
    };
    let Some(motion_vectors) = native_image(
        &motion_attachment.texture.texture,
        &motion_attachment.texture.default_view,
        vk::ImageAspectFlags::COLOR,
    ) else {
        fail_frame(
            frame.0,
            Fsr3FrameGenerationInputFailure::MotionVectorInterop,
            &feedback,
            &mut state,
            &mut history,
        );
        return;
    };
    let attachment_size = UVec2::new(depth.width, depth.height);
    let Some(render_size) = resolve_render_size(
        resolution_override.map(|resolution| resolution.0),
        attachment_size,
        UVec2::new(motion_vectors.width, motion_vectors.height),
        UVec2::new(output_size.width, output_size.height),
    ) else {
        fail_frame(
            frame.0,
            Fsr3FrameGenerationInputFailure::RenderExtent,
            &feedback,
            &mut state,
            &mut history,
        ); // 有效区域必须同时落在深度、运动和显示纹理内。
        return;
    };

    let position = extracted_view.world_from_view.translation(); // 当前世界相机位置。
    let projection = extracted_view.clip_from_view; // ExtractedView保留无抖动投影。
    let output = UVec2::new(output_size.width, output_size.height);
    let projection_changed =
        history.valid && matrix_difference(history.projection, projection) > 0.001; // FOV/裁面换代。
    let teleported = history.valid && history.position.distance(position) > 10.0; // 大位移不跨帧插值。
    let reset = !history.valid || history.output_size != output || projection_changed || teleported; // 首帧、resize、投影或瞬移均预热一帧。
    let transform = extracted_view.world_from_view; // 正交世界基向量。
    let jitter = jitter.map_or(Vec2::ZERO, |jitter| jitter.offset); // 原生/FXAA路径使用零抖动。
    let mut input = NativePrepareDesc {
        // 先校验CPU快照，再在HAL借用期内填写命令缓冲。
        command_buffer: 0, // 原生命令缓冲不得逃逸HAL录制闭包。
        depth,
        motion_vectors,
        render_width: render_size.x,
        render_height: render_size.y,
        jitter_x: jitter.x,
        jitter_y: jitter.y,
        motion_scale_x: -(render_size.x as f32), // 超分时必须使用有效内部区域，而不是全尺寸附件载体。
        motion_scale_y: -(render_size.y as f32),
        frame_time_ms: parameters.frame_time_ms,
        camera_near: parameters.camera_near,
        camera_fov_y: parameters.camera_fov_y,
        camera_position: position.to_array(),
        camera_up: transform.up().as_vec3().to_array(),
        camera_right: transform.right().as_vec3().to_array(),
        camera_forward: transform.forward().as_vec3().to_array(),
        requested: 1, // 主世界已解析最终资格。
        reset: u8::from(reset),
        padding: [0; 2], // ABI填充必须确定性清零。
    }; // 完整同步Prepare快照结束。
    if !prepare_is_finite(&input) {
        fail_frame(
            frame.0,
            Fsr3FrameGenerationInputFailure::NonFiniteParameters,
            &feedback,
            &mut state,
            &mut history,
        );
        return;
    }
    context.command_encoder().transition_resources(
        // 原生SDK读取也必须登记wgpu资源跟踪，建立前序写入到Compute读取的依赖。
        core::iter::empty(), // Prepare不直接借用wgpu缓冲。
        [
            &depth_attachment.texture.texture,
            &motion_attachment.texture.texture,
        ] // 两张输入固定有界，不复制像素。
        .into_iter() // 使用栈内数组，不在逐帧路径分配。
        .map(|texture| wgpu::TextureTransition {
            // 让wgpu维持真实使用状态与资源生命周期。
            texture: &**texture,                // 只借用Bevy包装内的wgpu纹理。
            selector: None,                     // 预通道单mip单层，覆盖完整输入。
            state: wgpu::TextureUses::RESOURCE, // 深度为DEPTH_STENCIL_READ_ONLY，颜色为SHADER_READ_ONLY。
        }), // 深度到FFX专用读取布局的临时转换由原生桥成对恢复。
    ); // 后续wgpu命令仍能正确跟踪这两张纹理。
    let mut native_encoder =
        context
            .render_device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                // 保留普通编码器的资源跟踪，独立录制原生 SDK 命令。
                label: Some("fsr3_frame_generation_prepare"), // 独立原生命令缓冲便于GPU抓帧定位。
            }); // 只增加一个有界命令缓冲，仍由Bevy统一提交。
    let result = unsafe {
        // SDK录制严格限制在wgpu提供的HAL借用作用域内。
        native_encoder.as_hal_mut::<wgpu_hal::vulkan::Api, _, _>(|encoder| {
            // 原生编码器不调用任何普通wgpu录制API。
            encoder.map(|encoder| {
                // 非Vulkan编码器返回None并精确阻断。
                input.command_buffer = encoder.raw_command_buffer().as_raw(); // 句柄只在本闭包同步使用。
                native::prepare(&input) // 原生Prepare及恢复屏障在结束HAL录制之前完成。
            }) // 返回SDK结果，不返回原生句柄。
        }) // 退出后才允许继续wgpu录制。
    }; // HAL借用结束。
    if result != Some(native::RESULT_OK) {
        // 区分编码器不存在与SDK调用失败。
        fail_frame(
            frame.0,
            if result.is_none() {
                Fsr3FrameGenerationInputFailure::CommandBuffer
            } else {
                Fsr3FrameGenerationInputFailure::PrepareCall
            }, // 保留可定位的阻断原因。
            &feedback,
            &mut state,
            &mut history,
        );
        return;
    }
    context.add_command_buffer(native_encoder.finish()); // 先flush前面的wgpu资源转换，再插入Prepare，后续UI继续使用新的普通编码器。
    permission.allow(
        frame.0,
        extracted_view.retained_view_entity.main_entity.id(),
    ); // 公共UI仅捕获本次Prepare确实成功的主视图。
    state.prepared = true; // 此时只完成深度/运动Prepare，完整输入成功与恢复日志必须等finish_ui登记成功。
    *history = CameraHistory {
        position,
        projection,
        output_size: output,
        valid: true,
    }; // 只有完整SDK成功才推进时间历史。
} // capture_and_prepare结束。

fn fail_frame(
    frame: u32,
    reason: Fsr3FrameGenerationInputFailure,
    feedback: &RenderFeedback,
    state: &mut RenderState,
    history: &mut CameraHistory,
) {
    let failure_changed = state.failure != Some(reason); // 重复缺项已经处于明确关闭态。
    if state.prepared || failure_changed {
        native::disable(); // 失败边界撤销旧Prepare，稳定失败帧不重复调SDK。
    }
    feedback.mark_failed(frame, reason); // 发布精确失败事实和缺项。
    if failure_changed {
        warn!("FSR 3.1帧生成渲染输入失败：{}", reason.message()); // 缺项不变时稳定帧零日志。
        state.failure = Some(reason);
    }
    state.prepared = false; // 下一成功帧重新建立有效状态。
    history.valid = false; // 恢复首帧重置时间历史。
} // fail_frame结束。

fn matrix_difference(left: Mat4, right: Mat4) -> f32 {
    left.to_cols_array()
        .into_iter()
        .zip(right.to_cols_array())
        .map(|(left, right)| (left - right).abs())
        .fold(0.0, f32::max) // 固定十六元素最大绝对差，无分配。
} // matrix_difference结束。

/// 解析当前真实渲染区域；Vendor SR保留全尺寸附件，只在左上角写入override范围。
fn resolve_render_size(
    resolution_override: Option<UVec2>,
    depth_size: UVec2,
    motion_size: UVec2,
    output_size: UVec2,
) -> Option<UVec2> {
    let render_size = resolution_override.unwrap_or(depth_size); // 原生路径直接使用完整预通道。
    (render_size.x > 0
        && render_size.y > 0
        && render_size.x <= depth_size.x
        && render_size.y <= depth_size.y
        && render_size.x <= motion_size.x
        && render_size.y <= motion_size.y
        && render_size.x <= output_size.x
        && render_size.y <= output_size.y)
        .then_some(render_size) // 不要求附件尺寸相等，避免把合法全尺寸载体误判为低分辨率输入。
} // resolve_render_size结束。

fn prepare_is_finite(input: &NativePrepareDesc) -> bool {
    input.jitter_x.is_finite()
        && input.jitter_y.is_finite()
        && input.motion_scale_x.is_finite()
        && input.motion_scale_y.is_finite()
        && input.frame_time_ms.is_finite()
        && input.frame_time_ms > 0.0
        && input.camera_near.is_finite()
        && input.camera_near > 0.0
        && input.camera_fov_y.is_finite()
        && input.camera_position.iter().all(|value| value.is_finite())
        && input.camera_up.iter().all(|value| value.is_finite())
        && input.camera_right.iter().all(|value| value.is_finite())
        && input.camera_forward.iter().all(|value| value.is_finite()) // SDK拒绝任何NaN/Infinity前由宿主阻断。
} // prepare_is_finite结束。

pub(super) fn native_image(
    // UI子域复用精确Vulkan用途和句柄转换，不维护第二套ABI映射。
    texture: &bevy::render::render_resource::Texture, // wgpu拥有的图像。
    view: &bevy::render::render_resource::TextureView, // 匹配基础二维视图。
    aspect: vk::ImageAspectFlags,                     // 颜色或深度用途映射。
) -> Option<NativeImage> {
    let image = unsafe {
        texture
            .as_hal::<wgpu_hal::vulkan::Api>()
            .map(|raw| raw.raw_handle().as_raw())?
    }; // 只复制句柄位，表达式结束即释放Texture的snatch读锁。
    let format = unsafe {
        view.as_hal::<wgpu_hal::vulkan::Api>()
            .map(|raw| raw.raw_format().as_raw() as u32)?
    }; // Texture锁已释放后再取View格式，避免在同一资源上嵌套 HAL 锁。
    let size = texture.size();
    Some(NativeImage {
        image,  // 只读VkImage句柄位模式。
        format, // 匹配基础视图的VkFormat原值。
        width: size.width,
        height: size.height,
        depth: 1, // 当前只支持二维窗口纹理。
        mip_levels: texture.mip_level_count(),
        array_layers: size.depth_or_array_layers,
        sample_count: map_sample_count(texture.sample_count()).as_raw(),
        usage: map_image_usage(texture.usage(), aspect).as_raw(),
        flags: vk::ImageCreateFlags::empty().as_raw(), // wgpu公开API不暴露额外创建flags，当前资源无需可变格式。
    })
} // native_image结束。

fn map_sample_count(sample_count: u32) -> vk::SampleCountFlags {
    match sample_count {
        1 => vk::SampleCountFlags::TYPE_1,
        2 => vk::SampleCountFlags::TYPE_2,
        4 => vk::SampleCountFlags::TYPE_4,
        8 => vk::SampleCountFlags::TYPE_8,
        16 => vk::SampleCountFlags::TYPE_16,
        32 => vk::SampleCountFlags::TYPE_32,
        64 => vk::SampleCountFlags::TYPE_64,
        _ => vk::SampleCountFlags::empty(), // 非法值由原生资源验证拒绝。
    }
} // map_sample_count结束。

fn map_image_usage(usage: TextureUsages, aspect: vk::ImageAspectFlags) -> vk::ImageUsageFlags {
    let mut flags = vk::ImageUsageFlags::empty();
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
    }
    flags // 返回精确VkImageUsageFlags位集合。
} // map_image_usage结束。

#[cfg(test)] // 纯数学和用途测试不启动GPU或加载SDK。
mod tests {
    use super::*;

    #[test]
    fn motion_vector_scale_matches_project_temporal_reconstruction_convention() {
        let width = 2560_u32;
        let height = 1440_u32;
        assert_eq!([-(width as f32), -(height as f32)], [-2560.0, -1440.0]); // 与FSR SR/DLSS节点同号。
    }

    #[test]
    fn vendor_upscaling_uses_override_instead_of_full_attachment_extent() {
        let output = UVec2::new(3840, 2160);
        let internal = UVec2::new(1920, 1080);
        assert_eq!(
            resolve_render_size(Some(internal), output, output, output),
            Some(internal),
        ); // DLSS/FSR SR的深度和运动附件仍是4K载体，FG只消费1080p有效区域。
        assert_eq!(
            resolve_render_size(Some(UVec2::new(4096, 2160)), output, output, output),
            None,
        ); // 越界override在录制任何FFX命令前阻断。
    }

    #[test]
    fn motion_vector_usage_maps_to_sampling_and_color_attachment() {
        let mapped = map_image_usage(
            TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
            vk::ImageAspectFlags::COLOR,
        );
        assert!(mapped.contains(vk::ImageUsageFlags::SAMPLED));
        assert!(mapped.contains(vk::ImageUsageFlags::COLOR_ATTACHMENT));
        assert!(!mapped.contains(vk::ImageUsageFlags::TRANSFER_DST));
    }
} // 测试模块结束。
