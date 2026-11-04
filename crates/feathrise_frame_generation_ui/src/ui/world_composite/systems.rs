//! 世界UI透明层的预热与空间合成；分离失败时仍把标签画回普通场景，避免丢失UI。
//! 世界标签与屏幕UI共用作者线性显示色，保留原shader fog和深度遮挡，但明确绕过场景tonemap、bloom及时间域处理。
use super::{
    helpers::sampling_parameters,
    resources::{WORLD_UI_SHADER, WorldUiCompositeReady, WorldUiCompositor},
    types::WorldUiSample,
};
use crate::world_ui::WorldUiLayer; // 世界3D阶段负责实际绘制、深度遮挡与本帧图像身份。
use bevy::{
    core_pipeline::FullscreenShader,
    diagnostic::FrameCount,
    prelude::*,
    render::{
        render_resource::{
            binding_types::{texture_2d, uniform_buffer},
            *,
        },
        renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
        view::{ExtractedView, ViewTarget},
    },
}; // 仅记录有界GPU工作。 // 固定管线与空间采样状态。

pub(super) fn prepare_pipeline(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
    fullscreen: Res<FullscreenShader>,
) {
    // 设备建立后预先分配参数缓冲和编译固定管线。
    let layout = BindGroupLayoutDescriptor::new(
        "frame_generation_world_ui_sample",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: false }),
                uniform_buffer::<WorldUiSample>(false),
            ),
        ),
    ); // 非过滤textureLoad实现透明越界的四点采样。
    let queued = cache.queue_render_pipeline(RenderPipelineDescriptor {
        // 管线编译只排队，不阻塞等待。
        label: Some("frame_generation_world_ui_spatial_composite".into()), // GPU抓帧明确区分3D标签重采样。
        layout: vec![layout.clone()],                                      // 固定两个绑定项。
        vertex: fullscreen.to_vertex_state(),                              // 共用Bevy全屏三角形。
        fragment: Some(FragmentState {
            shader: WORLD_UI_SHADER,
            entry_point: Some("fragment".into()),
            targets: vec![Some(ColorTargetState {
                format: TextureFormat::Rgba16Float,
                blend: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }), // 线性UI层保持真实预乘颜色。
        ..default() // 此步骤不接管深度，遮挡已由世界UI3D阶段完成。
    }); // 管线描述结束。
    let mut parameters = UniformBuffer::from(WorldUiSample::default()); // 固定32字节，不保留逐帧数组。
    parameters.write_buffer(&device, &queue); // 创建缓冲后才允许发布ready，防止分流后缺绑定。
    commands.insert_resource(WorldUiCompositor {
        layout,
        queued,
        pipeline: None,
        parameters,
        binding: None,
    }); // 初始管线未完成，世界标签仍走普通主pass。
} // 世界UI管线创建结束。

pub(super) fn publish_ready(
    cache: Res<PipelineCache>,
    mut compositor: ResMut<WorldUiCompositor>,
    mut ready: ResMut<WorldUiCompositeReady>,
) {
    // 固定一次非阻塞查询，不检查或等待任何GPU fence。
    if compositor.pipeline.is_none() {
        compositor.pipeline = cache.get_render_pipeline(compositor.queued).cloned();
    } // 保留最后可用管线，避免已经分流的标签因缓存刷新丢失。
    ready.0 = compositor.pipeline.is_some() && compositor.parameters.binding().is_some(); // 只有完整GPU合成能力存在才允许世界3D阶段分流。
} // 合成资格发布结束。

pub(super) fn composite(
    view: ViewQuery<(&ViewTarget, &ExtractedView, Option<&WorldUiLayer>)>,
    frame: Res<FrameCount>,
    cache: Res<PipelineCache>,
    queue: Res<RenderQueue>,
    mut compositor: ResMut<WorldUiCompositor>,
    mut context: RenderContext,
) {
    // begin之后不区分FG开关，任何已经分流的当前帧标签都必须显示。
    let (target, extracted, layer) = view.into_inner(); // 读取当前主视图身份。
    let Some(layer) = layer else {
        return;
    }; // 仅没有高清分流时跳过；FG关闭但时间重建开启仍有独立层。
    if !layer.is_drawn()
        || layer.has_failed()
        || layer.frame != frame.0
        || layer.camera != extracted.retained_view_entity.main_entity.id()
    {
        return;
    } // 不复用旧帧、辅助视图或已经普通回绘的图像。
    let Some(pipeline) = compositor.pipeline.clone() else {
        return;
    }; // 世界3D分流门保证当前分支只在完整管线可用时发生。
    let output = UVec2::new(
        target.main_texture().width(),
        target.main_texture().height(),
    ); // begin成功时是透明层，否则是普通scene，尺寸契约完全相同。
    if output != layer.output_size
        || layer.render_size.x == 0
        || layer.render_size.y == 0
        || layer.render_size.x > layer.texture.texture.width()
        || layer.render_size.y > layer.texture.texture.height()
        || target.main_texture_format() != TextureFormat::Rgba16Float
    {
        return;
    } // 世界3D阶段采用相同资格门，拒绝不一致资源身份。
    compositor
        .parameters
        .set(sampling_parameters(layer.render_size, layer.output_size)); // 只写当前帧的输出分辨率有效区，绝不再做时间抖动位移。
    compositor
        .parameters
        .write_buffer(context.render_device(), &queue); // 单队列32字节更新，后续GPU采样按正式提交顺序使用。
    if compositor
        .binding
        .as_ref()
        .is_none_or(|(view, _)| *view != layer.texture.default_view.id())
    {
        // 只有resize导致实际纹理视图更换时才建立新绑定。
        let Some(parameters) = compositor.parameters.binding() else {
            return;
        }; // 初始化已保证存在，此处防御设备资源异常。
        let group = context.render_device().create_bind_group(
            Some("frame_generation_world_ui_source"),
            &cache.get_bind_group_layout(&compositor.layout),
            &BindGroupEntries::sequential((&layer.texture.default_view, parameters)),
        ); // 同时绑定本帧标签与固定采样参数。
        compositor.binding = Some((layer.texture.default_view.id(), group)); // 始终只保留当前主视图一个绑定。
    } // 绑定更新结束。
    let group = &compositor.binding.as_ref().unwrap().1; // 本次输入和参数均已验证存在。
    super::super::helpers::draw(
        &mut context,
        &pipeline,
        group,
        target.main_texture_view(),
        LoadOp::Load,
    ); // begin成功合入透明UI；begin失败直接恢复普通scene上的世界标签。
} // 世界UI当前帧空间合成结束。
