//! 将色调映射后的线性场景编码为交换链 sRGB，保证 DLSS-G 的 HUDless 与真实输出一致。
use bevy::{
    app::SubApp,
    asset::uuid_handle,
    core_pipeline::FullscreenShader,
    prelude::*,
    render::{
        RenderStartup,
        render_resource::{binding_types::texture_2d, *},
        renderer::RenderContext,
        texture::CachedTexture,
        view::ViewTarget,
    },
}; // 只使用异步管线缓存和 GPU 全屏绘制。

const SHADER: Handle<bevy::shader::Shader> = uuid_handle!("8993cc12-ec70-4fde-a3ef-e2e113c5f00c"); // 本模块独立内嵌 Shader 身份。

#[derive(Resource)] // 两条固定格式管线不随尺寸增加。
pub(super) struct HudlessPipelines {
    // 调用者只需要把资源传给 encode。
    layout: BindGroupLayoutDescriptor, // 无采样器单纹理绑定。
    bgra: CachedRenderPipelineId,      // BGRA sRGB 窗口输出。
    rgba: CachedRenderPipelineId,      // RGBA sRGB 窗口输出。
} // 管线资源结束。

#[derive(Resource, Default)] // 未请求帧生成时不分配图像。
pub(super) struct HudlessCapture {
    // 固定两绑定和一输出，避免逐帧资源增长。
    bindings: [Option<(TextureViewId, BindGroup)>; 2], // ViewTarget 的 A/B 主纹理绑定。
    image: Option<CachedTexture>,                      // 仅保存最近规格的强引用。
} // 捕获资源结束。

pub(crate) fn load_shader(app: &mut App) {
    // 在主 App 资产系统建立后调用。
    bevy::asset::load_internal_asset!(app, SHADER, "hudless.wgsl", bevy::shader::Shader::from_wgsl); // 正式运行不访问作者源码路径。
} // Shader 注册结束。

pub(super) fn install(app: &mut SubApp) {
    // 不自行安装视图系统，调用者决定场景捕获时机。
    app.init_resource::<HudlessCapture>()
        .add_systems(RenderStartup, prepare); // GPU 编译排队不等待结果。
} // RenderApp 安装结束。

fn prepare(mut commands: Commands, cache: Res<PipelineCache>, fullscreen: Res<FullscreenShader>) {
    // 固定启动期准备。
    let layout = BindGroupLayoutDescriptor::new(
        "dlss_hudless_source",
        &BindGroupLayoutEntries::single(
            ShaderStages::FRAGMENT,
            texture_2d(TextureSampleType::Float { filterable: false }),
        ),
    ); // textureLoad 保持逐像素无滤波。
    let pipeline = |format| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            // 只排队，不阻塞渲染线程。
            label: Some("dlss_hudless_srgb_encode".into()), // GPU 捕获可辨认颜色编码。
            layout: vec![layout.clone()],                   // 两格式共用布局。
            vertex: fullscreen.to_vertex_state(),           // 全屏三角形无需顶点缓冲。
            fragment: Some(FragmentState {
                shader: SHADER,
                entry_point: Some("fragment".into()),
                targets: vec![Some(ColorTargetState {
                    format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }), // 硬件 sRGB 编码，禁用混合。
            ..default()                                     // 不使用深度或多重采样。
        })
    }; // 管线描述结束。
    commands.insert_resource(HudlessPipelines {
        layout: layout.clone(),
        bgra: pipeline(TextureFormat::Bgra8UnormSrgb),
        rgba: pipeline(TextureFormat::Rgba8UnormSrgb),
    }); // 固定两个异步管线身份。
} // 启动准备结束。

pub(super) fn encode(
    context: &mut RenderContext,
    target: &ViewTarget,
    pipelines: &HudlessPipelines,
    cache: &PipelineCache,
    capture: &mut HudlessCapture,
) -> Option<CachedTexture> {
    // None 表示当前帧必须关闭 FG，不等待管线。
    let source = target.main_texture(); // 调用者保证三维后处理完成且 UI 尚未写入。
    let format = target.out_texture_view_format()?.add_srgb_suffix(); // 独立纹理可合法使用sRGB格式编码；代理窗口本身只使用UNORM视图。
    let id = match format {
        // 只接受当前已经支持的 SDR 输出。
        TextureFormat::Bgra8UnormSrgb => pipelines.bgra, // BGRA 由硬件编码。
        TextureFormat::Rgba8UnormSrgb => pipelines.rgba, // RGBA 由硬件编码。
        _ => return None,                                // 不误把 HDR10 或线性目标当作 sRGB。
    }; // 输出格式判断结束。
    let size = source.size(); // 本接口不执行缩放，保持同尺寸像素对应。
    if source.format() != TextureFormat::Rgba16Float
        || source.sample_count() != 1
        || size.width == 0
        || size.height == 0
        || size.depth_or_array_layers != 1
        || u64::from(size.width) * u64::from(size.height) * 4 > 256 * 1024 * 1024
    {
        return None;
    } // 有限单张纹理预算，拒绝不兼容颜色载体。
    let pipeline = cache.get_render_pipeline(id)?; // 未编译时立即回退，不阻塞帧。
    if capture
        .image
        .as_ref()
        .is_none_or(|image| image.texture.size() != size || image.texture.format() != format)
    {
        // 只在规格变化时分配一次。
        let texture = context.render_device().create_texture(&TextureDescriptor {
            // GPU 输出常驻最近一组。
            label: Some("dlss_hudless_srgb"), // 区分线性主颜色与 SDK 输入。
            size,                             // 与完整主颜色相同分辨率。
            mip_level_count: 1,               // SDK 只读取基础级。
            sample_count: 1,                  // 时间超分后单采样颜色。
            dimension: TextureDimension::D2,  // 唯一窗口二维输出。
            format,                           // 真实交换链 sRGB 编码格式。
            usage: TextureUsages::RENDER_ATTACHMENT
                | TextureUsages::TEXTURE_BINDING
                | TextureUsages::COPY_SRC, // GPU 编码及 SDK 易失资源复制用途。
            view_formats: &[],                // 不允许另一套颜色解释。
        }); // 输出分配结束。
        let default_view = texture.create_view(&TextureViewDescriptor::default()); // 原生标签使用同格式默认视图。
        capture.image = Some(CachedTexture {
            texture,
            default_view,
        }); // 替换唯一持久强引用。
        capture.bindings = [None, None]; // 规格变化同时释放旧主纹理绑定。
    } // 最近规格更新结束。
    let view = target.main_texture_view(); // 当前 ping-pong 读源。
    let group = if let Some((_, group)) = capture
        .bindings
        .iter()
        .flatten()
        .find(|(id, _)| *id == view.id())
    {
        // 稳定帧复用两个主视图绑定。
        group.clone() // 仅克隆 GPU 包装句柄。
    } else {
        // 首次或主视图更换时建立绑定。
        let group = context.render_device().create_bind_group(
            Some("dlss_hudless_source"),
            &cache.get_bind_group_layout(&pipelines.layout),
            &BindGroupEntries::single(view),
        ); // 单纹理无采样器。
        capture.bindings.rotate_left(1); // 固定两槽 LRU，绝不按帧增长。
        capture.bindings[1] = Some((view.id(), group.clone())); // 新绑定保留给下次同源帧。
        group // 当前绘制拥有独立强引用。
    }; // 绑定获取结束。
    let image = capture.image.as_ref()?.clone(); // 调用者持有同帧纹理直至原生登记完成。
    let attachments = [Some(RenderPassColorAttachment {
        view: &image.default_view,
        depth_slice: None,
        resolve_target: None,
        ops: Operations {
            load: LoadOp::Clear(wgpu::Color::BLACK),
            store: StoreOp::Store,
        },
    })]; // 全图覆盖的独立颜色附件。
    let descriptor = RenderPassDescriptor {
        label: Some("dlss_hudless_srgb_encode"),
        color_attachments: &attachments,
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }; // 无深度写入或第二套提交流。
    {
        // 在返回前结束 pass，调用者随后可 transition 到 RESOURCE 并登记 SDK。
        let mut pass = context.command_encoder().begin_render_pass(&descriptor); // 与当前场景捕获命令流有序执行。
        pass.set_pipeline(pipeline); // 已证明管线就绪。
        pass.set_bind_group(0, &group, &[]); // 同尺寸线性场景读源。
        pass.draw(0..3, 0..1); // 固定全屏 GPU 编码，不执行 CPU 像素工作。
    } // GPU 编码 pass 结束。
    Some(image) // 当前布局为 COLOR_ATTACHMENT_OPTIMAL，由调用者显式跟踪只读转换。
} // 编码入口结束。
