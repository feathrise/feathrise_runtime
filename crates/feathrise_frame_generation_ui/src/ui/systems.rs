//! 主视图UI分离事务：全部资源就绪才切换目标，任何登记失败均恢复普通完整颜色。
use super::{
    helpers,
    resources::{UI_SHADER, UiCapture, UiPipelines},
    types::ActiveUi,
};
use crate::{
    FrameGenerationUiRequest as RenderRequest, UiCaptureResult, UiFrame, UiFramePermission,
}; // SDK无关的请求和完整捕获结果。
use bevy::{
    core_pipeline::FullscreenShader,
    diagnostic::FrameCount,
    prelude::*,
    render::{
        camera::ExtractedCamera,
        render_resource::{binding_types::texture_2d, *},
        renderer::{RenderContext, ViewQuery},
        texture::TextureCache,
        view::{ExtractedView, ResolvedCompositingSpace, ViewTarget},
    },
}; // Bevy既有视图与GPU管线接口。 // 有限资源与无等待GPU辅助。

pub(super) fn prepare_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    fullscreen: Res<FullscreenShader>,
) {
    // 设备初始化阶段仅把三个固定管线排队编译。
    let layout = BindGroupLayoutDescriptor::new(
        "frame_generation_ui_texture",
        &BindGroupLayoutEntries::single(
            ShaderStages::FRAGMENT,
            texture_2d(TextureSampleType::Float { filterable: false }),
        ),
    ); // textureLoad无需过滤器和采样器。
    let pipeline = |format, blend| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            // 不使用block_on_render_pipeline或同步GPU等待。
            label: Some("frame_generation_ui_encode_or_restore".into()), // GPU调试名称明确用途。
            layout: vec![layout.clone()], // 三条管线共享一个单纹理布局。
            vertex: fullscreen.to_vertex_state(), // 复用Bevy全屏三角形。
            fragment: Some(FragmentState {
                shader: UI_SHADER,
                entry_point: Some("fragment".into()),
                targets: vec![Some(ColorTargetState {
                    format,
                    blend,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }), // sRGB格式自行编码；预乘还原保留真实Alpha。
            ..default()                   // 深度、MSAA与其他渲染状态均保持单次颜色绘制。
        })
    }; // 固定管线构造结束。
    let bgra = pipeline(TextureFormat::Bgra8UnormSrgb, None); // 对应BGRA输出窗口。
    let rgba = pipeline(TextureFormat::Rgba8UnormSrgb, None); // 对应RGBA输出窗口。
    let restore = pipeline(
        TextureFormat::Rgba16Float,
        Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
    ); // 线性主颜色上恢复已有透明覆盖层。
    commands.insert_resource(UiPipelines {
        layout,
        bgra,
        rgba,
        restore,
    }); // 发布排队身份，begin仍须非阻塞检查实际编译结果。
} // 管线预热结束。

#[allow(clippy::too_many_arguments)] // 每项资源对应一个需要显式审计的帧内所有权。
pub(super) fn begin_ui(
    // 在成功Prepare后和所有2D UI绘制前开始原子分离。
    view: ViewQuery<(
        &ViewTarget,
        &ExtractedView,
        &ExtractedCamera,
        Option<&ResolvedCompositingSpace>,
    )>, // 当前Core3d视图的输出、相机身份和实际合成空间。
    cameras: Query<&ExtractedCamera>, // 拒绝同窗口多相机输出，避免共享主纹理被其他相机改写。
    request: Res<RenderRequest>,      // 主世界唯一玩法主相机资格。
    frame: Res<FrameCount>,           // 同帧Prepare和UI事务身份。
    pipelines: Res<UiPipelines>,      // 固定管线排队身份。
    cache: Res<PipelineCache>,        // 非阻塞检查真实GPU管线。
    mut textures: ResMut<TextureCache>, // 尺寸稳定时复用唯一UI捕获纹理。
    mut capture: ResMut<UiCapture>,   // 有界绑定、目标与当前事务。
    permission: Res<UiFramePermission>, // 后端同帧完整准备成功才允许切换目标。
    mut context: RenderContext,       // 普通wgpu命令保持完整资源使用跟踪。
) {
    // 分离入口。
    let (target, extracted, camera, compositing_space) = view.into_inner(); // 读取当前视图而非猜第一个相机。
    let owner = extracted.retained_view_entity.main_entity.id(); // 与主世界请求使用同一实体身份。
    if !request.enabled
        || request.camera != Some(owner)
        || !request.backend_ready
        || !permission.allows(frame.0, owner)
    {
        return;
    } // 辅助相机及未成功Prepare帧不触碰主纹理。
    let size = camera.physical_target_size.unwrap_or(UVec2::ZERO); // 必须是完整物理窗口范围。
    let output_format = target
        .out_texture_view_format()
        .map(|format| format.add_srgb_suffix()); // 独立UI固定sRGB编码；UNORM窗口由最终pass显式编码相同字节。
    let valid = size.x > 0 && size.y > 0 && camera.physical_viewport_size == Some(size) && camera.viewport.as_ref().is_none_or(|viewport| viewport.physical_position == UVec2::ZERO && viewport.physical_size == size) // 只接受完整窗口视口。
        && target.main_texture_format() == TextureFormat::Rgba16Float && ResolvedCompositingSpace::space(compositing_space).is_none_or(|space| space.is_linear()) // 真实透明UI必须在线性RGBA16F中累积。
        && target.main_texture().size().width == size.x && target.main_texture().size().height == size.y && target.main_texture().sample_count() == 1 // 不把低分辨率载体或MSAA当作最终覆盖层。
        && matches!(output_format, Some(TextureFormat::Bgra8UnormSrgb | TextureFormat::Rgba8UnormSrgb)) // 首期SDR路径不隐式处理HDR交换链。
        && camera.target.is_some() && cameras.iter().filter(|candidate| candidate.target == camera.target).take(2).count() == 1 // 同窗口多输出视图无法原子共享捕获，安全退回普通完整UI。
        && capture.active.is_none(); // 前一事务未恢复时禁止覆盖其身份。
    if !valid {
        return;
    } // 拒绝前保持原主纹理未切换，普通UI继续绘制。
    let format = output_format.unwrap(); // 上面的格式门已经证明存在。
    let encode_id = if format == TextureFormat::Bgra8UnormSrgb {
        pipelines.bgra
    } else {
        pipelines.rgba
    }; // 精确选择当前窗口编码管线。
    let (Some(encode), Some(restore)) = (
        cache.get_render_pipeline(encode_id).cloned(),
        cache.get_render_pipeline(pipelines.restore).cloned(),
    ) else {
        // 两条管线都完成才有恢复保障。
        return; // 异步预热帧保持普通UI，不等待编译。
    }; // GPU管线就绪门结束。
    let Some(targets) =
        helpers::capture_targets(&mut textures, context.render_device(), size, format)
    else {
        // 先准备完整可互操作UI输出再切换。
        return; // GPU图像缺失时不分离，原生转换由后端完成。
    }; // 捕获目标准备结束。
    let ui_binding = helpers::binding(
        &mut capture,
        target.main_texture_other_view(),
        &pipelines.layout,
        context.render_device(),
        &cache,
    ); // 另一主纹理将成为真正透明UI目标。
    capture.targets = Some(targets); // 持久强引用延续到交换链quiesce之后，避免SDK借用悬空。
    capture.active = Some(ActiveUi {
        camera: owner,
        frame: frame.0,
        ui_binding,
        encode,
        restore,
    }); // 场景留在另一主纹理，恢复资源齐全后才发布事务。
    let write = target.post_process_write(); // 只在最后一步翻到另一主颜色，保留source作为场景快照。
    helpers::clear(&mut context, write.destination); // RGBA全零确保后续两种UI的Alpha是实际绘制结果。
} // UI分离入口结束。

pub(super) fn capture_ui(
    // UI 完成后翻回场景，并发布真实透明层给后端同帧消费。
    view: ViewQuery<(&ViewTarget, &ExtractedView)>, // 当前唯一视图身份。
    frame: Res<FrameCount>,                         // 不接受跨帧捕获。
    capture: Res<UiCapture>,                        // 持有恢复所需管线和绑定，直到 Restore。
    mut result: ResMut<UiCaptureResult>,            // 唯一完整结果槽。
    mut context: RenderContext,                     // 编码命令先于后端标签系统提交。
) {
    // GPU 捕获入口。
    let (target, extracted) = view.into_inner(); // 当前视图不能消费另一相机事务。
    let owner = extracted.retained_view_entity.main_entity.id(); // 保留主世界相机身份。
    let Some(active) = capture
        .active
        .as_ref()
        .filter(|active| active.camera == owner)
    else {
        return;
    }; // 无活动事务不翻纹理。
    let write = target.post_process_write(); // 无条件翻回保留场景，UI 来源仍由绑定持有。
    result.scene_only = None; // 默认完整真实帧；每次必须重新取得后端成功证明。
    let Some(targets) = capture.targets.as_ref() else {
        return;
    }; // 缺失图像时 Restore 仍用原主纹理绑定恢复。
    if active.frame != frame.0
        || targets.size
            != UVec2::new(
                write.destination_texture.width(),
                write.destination_texture.height(),
            )
        || Some(targets.format)
            != target
                .out_texture_view_format()
                .map(|format| format.add_srgb_suffix())
    {
        return;
    } // 比较相同输出色彩编码与通道，不能把合法UNORM窗口误判成UI格式换代。
    helpers::draw(
        &mut context,
        &active.encode,
        &active.ui_binding,
        &targets.ui.default_view,
        LoadOp::Clear(wgpu::Color::TRANSPARENT),
    ); // 从真实线性透明层编码，不从颜色差推测 Alpha。
    context.command_encoder().transition_resources(
        core::iter::empty(),
        core::iter::once(wgpu::TextureTransition {
            texture: &*targets.ui.texture,
            selector: None,
            state: wgpu::TextureUses::RESOURCE,
        }),
    ); // wgpu 跟踪原生后端后续只读布局。
    result.image = Some(UiFrame {
        frame: frame.0,
        camera: owner,
        size: targets.size,
        format: targets.format,
        texture: targets.ui.clone(),
    }); // 只在完整录制后原子发布强引用。
} // GPU 捕获结束。

pub(super) fn restore_ui(
    // 后端失败、DLSS 或未接管真实帧时，默认恢复完整普通 UI。
    view: ViewQuery<(&ViewTarget, &ExtractedView)>, // 只能由开始事务的相机收尾。
    frame: Res<FrameCount>,                         // SceneOnly 也必须同帧授权。
    mut capture: ResMut<UiCapture>, // 最后消费活动事务，释放两个主纹理的临时所有权。
    result: Res<UiCaptureResult>,   // 后端成功声明之外所有情况都恢复。
    mut context: RenderContext,     // 恢复在 upscaling/Present 前提交。
) {
    // 完整普通帧恢复入口。
    let (target, extracted) = view.into_inner(); // 当前 ViewTarget 已被 Capture 翻回场景。
    let owner = extracted.retained_view_entity.main_entity.id(); // 拒绝其他视图。
    if capture
        .active
        .as_ref()
        .is_none_or(|active| active.camera != owner)
    {
        return;
    } // 不消耗另一个视图的事务。
    let active = capture.active.take().unwrap(); // 每次 begin 最多收尾一次。
    if active.frame == frame.0 && result.scene_only == Some((frame.0, owner)) {
        return;
    } // FSR 本帧已接受图像并负责真实帧合成。
    helpers::draw(
        &mut context,
        &active.restore,
        &active.ui_binding,
        target.main_texture_view(),
        LoadOp::Load,
    ); // DLSS 真实帧和所有失败帧均保留完整 UI。
} // 普通帧恢复结束。
