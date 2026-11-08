//! 缓存固定深度重映射管线；只在RenderWorld申请采样用途，不修改玩家相机作者状态。
use super::resources::WorldUiDepth;
use bevy::{
    core_pipeline::{FullscreenShader, core_3d::CORE_3D_DEPTH_FORMAT},
    prelude::*,
    render::{
        render_resource::{
            binding_types::{texture_depth_2d, uniform_buffer},
            *,
        },
        renderer::{RenderDevice, RenderQueue},
        view::{ExtractedView, ViewDepthStencilTexture},
    },
}; // 使用引擎公开深度和全屏绘制接口。 // 固定大小GPU资源。
pub(super) const SHADER: Handle<bevy::shader::Shader> =
    bevy::asset::uuid_handle!("ca049052-e906-4b54-b943-b162c3297209"); // 内嵌着色器唯一身份。
pub(super) fn prepare(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
    fullscreen: Res<FullscreenShader>,
) {
    // 启动期排队编译。
    let layout = BindGroupLayoutDescriptor::new(
        "world_ui_depth_mapping",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (texture_depth_2d(), uniform_buffer::<Vec4>(false)),
        ),
    ); // 无过滤深度采样和16字节参数。
    let pipeline = cache.queue_render_pipeline(RenderPipelineDescriptor {
        // 固定格式，resize不重新编译。
        label: Some("world_ui_depth_mapping".into()), // GPU诊断名称。
        layout: vec![layout.clone()],                 // 唯一绑定组。
        vertex: fullscreen.to_vertex_state(),         // 三顶点全屏覆盖。
        fragment: Some(FragmentState {
            shader: SHADER,
            entry_point: Some("fragment".into()),
            targets: vec![],
            ..default()
        }), // 仅写深度。
        depth_stencil: Some(DepthStencilState {
            format: CORE_3D_DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(CompareFunction::Always),
            stencil: default(),
            bias: default(),
        }), // 全幅覆盖新深度，不修改主场景。
        ..default()                                   // 单采样，与WorldUI分流要求相同。
    }); // 管线异步编译。
    let mut parameters = UniformBuffer::from(Vec4::ZERO); // 单视图复用缓冲。
    parameters.write_buffer(&device, &queue); // 创建一次，不读回GPU。
    commands.insert_resource(WorldUiDepth {
        layout,
        pipeline,
        parameters,
        binding: None,
    }); // 未就绪时分流门保持普通绘制。
} // 管线初始化结束。
pub(super) fn configure(
    request: Res<super::request::WorldUiRequest>,
    mut views: Query<(&ExtractedView, &mut Camera3d)>,
) {
    // 只更新RenderWorld提取副本，使用与分流一致的独立资格。
    if request.camera.is_none() {
        return;
    } // 没有高清绘制需求时不额外申请用途。
    for (view, mut camera) in &mut views {
        // 与当前后端唯一主相机精确匹配。
        if Some(view.retained_view_entity.main_entity.id()) == request.camera {
            // 辅助视图不参与。
            let usage =
                TextureUsages::from(camera.depth_texture_usages) | TextureUsages::TEXTURE_BINDING; // 读取主深度要求显式用途。
            camera.depth_texture_usages = usage.into(); // 在核心深度附件分配前生效。
        } // 当前主相机处理结束。
    } // 视图遍历结束。
} // 用途配置结束。

pub(super) fn draw(
    view: bevy::render::renderer::ViewQuery<(
        &ExtractedView,
        &ViewDepthStencilTexture,
        Option<&super::components::WorldUiLayer>,
        Option<&bevy::render::camera::TemporalJitter>,
    )>,
    frame: Res<bevy::diagnostic::FrameCount>,
    cache: Res<PipelineCache>,
    queue: Res<RenderQueue>,
    mut mapping: ResMut<WorldUiDepth>,
    mut context: bevy::render::renderer::RenderContext,
) {
    // 独立系统可写GPU资源，后续DrawMaterial的只读World不与ResMut冲突。
    let (extracted, depth, layer, jitter) = view.into_inner(); // 读取当前主视图与真实抖动。
    let Some(layer) = layer.filter(|layer| {
        layer.frame == frame.0 && layer.camera == extracted.retained_view_entity.main_entity.id()
    }) else {
        return;
    }; // 不操作过期或辅助相机。
    let depth_texture = depth.texture();
    if depth_texture.format() != CORE_3D_DEPTH_FORMAT
        || depth_texture.sample_count() != 1
        || !depth_texture.usage().contains(TextureUsages::TEXTURE_BINDING)
    {
        return;
    } // 不兼容源由后续颜色系统回退。
    let Some(depth_view) = depth.attachment.depth_stencil_views().depth_only_view() else {
        return; // 采样使用深度专用视图，普通回退仍使用原深度/模板附件。
    };
    let Some(pipeline) = cache.get_render_pipeline(mapping.pipeline) else {
        return;
    }; // 非阻塞读取异步编译结果。
    let scale = layer.internal_size.as_vec2() / layer.output_size.as_vec2(); // 物理分配尺寸不等于有效viewport。
    let offset = jitter.map_or(Vec2::ZERO, |jitter| -jitter.offset); // Bevy透视矩阵z轴偏移在屏幕空间等于负offset。
    mapping
        .parameters
        .set(Vec4::new(scale.x, scale.y, offset.x, offset.y)); // 输出像素映射到本帧内部抖动像素。
    mapping
        .parameters
        .write_buffer(context.render_device(), &queue); // 单队列更新，不等待GPU。
    if mapping
        .binding
        .as_ref()
        .is_none_or(|(id, _)| *id != depth_view.id())
    {
        // 仅深度源换代重建绑定。
        let group = context.render_device().create_bind_group(
            Some("world_ui_depth_source"),
            &cache.get_bind_group_layout(&mapping.layout),
            &BindGroupEntries::sequential((depth_view, mapping.parameters.binding().unwrap())),
        ); // 初始化保证参数缓冲存在。
        mapping.binding = Some((depth_view.id(), group)); // 保留当前唯一源附件。
    } // 源绑定更新结束。
    {
        // 先完整填充输出深度，再让名牌使用硬件深度测试。
        let mut pass = context.begin_tracked_render_pass(RenderPassDescriptor {
            // 深度与颜色使用不同pass，避免同时读写主深度。
            label: Some("world_ui_depth_mapping"), // GPU定位入口。
            color_attachments: &[],                // 不改场景颜色。
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: &layer.mapped_depth.default_view,
                depth_ops: Some(Operations {
                    load: LoadOp::Clear(0.0),
                    store: StoreOp::Store,
                }),
                stencil_ops: None,
            }), // 保留反向Z天空清零语义。
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None, // 无同步查询或多视图。
        }); // 全屏映射pass开始。
        pass.set_render_pipeline(pipeline); // 固定Always写入管线。
        pass.set_bind_group(0, &mapping.binding.as_ref().unwrap().1, &[]); // 当前帧源深度与坐标。
        pass.draw(0..3, 0..1); // GPU全屏一次采样，无CPU像素循环。
    } // 映射pass结束。
    layer
        .depth_drawn
        .store(true, std::sync::atomic::Ordering::Release); // 同队列深度映射完整后才允许颜色层使用。
} // 当前帧深度映射结束。
