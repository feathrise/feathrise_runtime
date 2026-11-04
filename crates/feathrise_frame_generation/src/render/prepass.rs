//! 把无 COPY_SRC 用途的 Bevy 预通道采样为独立可复制图像，并裁切到真实内部渲染尺寸。
use bevy::{
    app::SubApp,
    asset::uuid_handle,
    core_pipeline::{FullscreenShader, prepass::ViewPrepassTextures},
    prelude::*,
    render::{
        RenderStartup,
        render_resource::{
            binding_types::{texture_2d, texture_depth_2d},
            *,
        },
        renderer::RenderContext,
        texture::CachedTexture,
    },
}; // 单次 GPU MRT 转存，不改变原始预通道用途。
const SHADER: Handle<bevy::shader::Shader> = uuid_handle!("c90967b8-f3fc-44c6-862e-3f8b06868ea1"); // 固定内嵌着色器身份。

#[derive(Resource)] // 一条固定 MRT 管线。
pub(super) struct PrepassPipelines {
    // 编译未完成时立即返回 None。
    layout: BindGroupLayoutDescriptor, // 深度与运动两个只读纹理。
    pipeline: CachedRenderPipelineId,  // R32Float 与 Rg16Float 双输出。
} // 管线记录结束。

#[derive(Clone)] // 返回廉价强引用保持当前帧所有权。
pub(super) struct PrepassImages {
    // SDK 读取的是独立精确内部尺寸图像。
    pub depth: CachedTexture,  // R32Float 颜色附件承载原始反向 Z 数值。
    pub motion: CachedTexture, // Rg16Float 原始归一化运动。
} // 图像记录结束。

#[derive(Resource, Default)] // 保持最近规格资源，不使用增长集合。
pub(super) struct PrepassCapture {
    // 深度双缓冲最多需要两个绑定。
    bindings: [Option<((TextureViewId, TextureViewId), BindGroup)>; 2], // 源身份精确匹配。
    images: Option<PrepassImages>,                                      // 唯一持久输出组。
} // 捕获资源结束。

pub(crate) fn load_shader(app: &mut App) {
    // 主 App 的资产已初始化。
    bevy::asset::load_internal_asset!(app, SHADER, "prepass.wgsl", bevy::shader::Shader::from_wgsl); // 正式包内嵌源码。
} // Shader 注册结束。

pub(super) fn install(app: &mut SubApp) {
    // 调用者控制当前视图捕获顺序。
    app.init_resource::<PrepassCapture>()
        .add_systems(RenderStartup, prepare); // 不同步等待 GPU 编译。
} // 安装结束。

fn prepare(mut commands: Commands, cache: Res<PipelineCache>, fullscreen: Res<FullscreenShader>) {
    // 只排队固定一条 MRT 管线。
    let layout = BindGroupLayoutDescriptor::new(
        "dlss_prepass_sources",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_depth_2d(),
                texture_2d(TextureSampleType::Float { filterable: false }),
            ),
        ),
    ); // 使用原始深度纹理和无滤波运动数据。
    let pipeline = cache.queue_render_pipeline(RenderPipelineDescriptor {
        // 异步编译，不执行帧内等待。
        label: Some("dlss_prepass_copy_mrt".into()), // GPU 调试中标识用途修复。
        layout: vec![layout.clone()],                // 两输入的固定布局。
        vertex: fullscreen.to_vertex_state(),        // 全屏三角形。
        fragment: Some(FragmentState {
            shader: SHADER,
            entry_point: Some("fragment".into()),
            targets: vec![
                Some(ColorTargetState {
                    format: TextureFormat::R32Float,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                }),
                Some(ColorTargetState {
                    format: TextureFormat::Rg16Float,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                }),
            ],
            ..default()
        }), // 单次遍历写出原始深度和运动。
        ..default()                                  // 不使用深度附件或 MSAA。
    }); // 固定管线描述结束。
    commands.insert_resource(PrepassPipelines { layout, pipeline }); // 发布异步管线身份。
} // 准备结束。

pub(super) fn encode(
    context: &mut RenderContext,
    source: &ViewPrepassTextures,
    render_size: UVec2,
    pipelines: &PrepassPipelines,
    cache: &PipelineCache,
    capture: &mut PrepassCapture,
) -> Option<PrepassImages> {
    // 内部尺寸必须由 MainPassResolutionOverride 的真实值提供。
    let depth = &source.depth.as_ref()?.texture; // 原始深度保持只读采样。
    let motion = &source.motion_vectors.as_ref()?.texture; // 不从无法合法复制的原纹理直接 vkCmdCopyImage。
    let size = Extent3d {
        width: render_size.x,
        height: render_size.y,
        depth_or_array_layers: 1,
    }; // 输出精确裁切左上内部有效区域。
    let fits = |texture: &Texture| {
        texture.width() >= size.width
            && texture.height() >= size.height
            && texture.sample_count() == 1
            && texture.depth_or_array_layers() == 1
    }; // 不采样越界或多重采样载体。
    if render_size.min_element() == 0
        || !fits(&depth.texture)
        || !fits(&motion.texture)
        || source.size.width < size.width
        || source.size.height < size.height
        || u64::from(size.width) * u64::from(size.height) * 8 > 256 * 1024 * 1024
    {
        return None;
    } // 两输出合计固定显存上限。
    let pipeline = cache.get_render_pipeline(pipelines.pipeline)?; // 预热期间安全回退。
    if capture
        .images
        .as_ref()
        .is_none_or(|images| images.depth.texture.size() != size)
    {
        // 仅规格变化时重新分配。
        let create = |format, label| {
            // 两图共用有界规格和合法复制用途。
            let texture = context.render_device().create_texture(&TextureDescriptor {
                // 不修改 Bevy 原始纹理。
                label: Some(label),              // 记录独立 SDK 输入用途。
                size,                            // 真实内部渲染尺寸。
                mip_level_count: 1,              // 数据只读取基础级。
                sample_count: 1,                 // 单采样数据图。
                dimension: TextureDimension::D2, // 全窗口左上区域二维图。
                format,                          // 原始数值不会发生 sRGB 编码。
                usage: TextureUsages::RENDER_ATTACHMENT
                    | TextureUsages::TEXTURE_BINDING
                    | TextureUsages::COPY_SRC, // Streamline volatile复制必需TRANSFER_SRC。
                view_formats: &[],               // 固定格式，不重解释深度位。
            }); // 分配结束。
            let default_view = texture.create_view(&TextureViewDescriptor::default()); // SDK采用颜色aspect视图。
            CachedTexture {
                texture,
                default_view,
            } // 输出强引用。
        }; // 单图创建结束。
        capture.images = Some(PrepassImages {
            depth: create(TextureFormat::R32Float, "dlss_depth_copy"),
            motion: create(TextureFormat::Rg16Float, "dlss_motion_copy"),
        }); // 替换唯一输出组。
        capture.bindings = [None, None]; // 规格换代释放旧输入绑定。
    } // 图像缓存更新结束。
    let key = (depth.default_view.id(), motion.default_view.id()); // 双源身份都参与缓存键。
    let group =
        if let Some((_, group)) = capture.bindings.iter().flatten().find(|(id, _)| *id == key) {
            // 稳定帧或深度双缓冲复用。
            group.clone() // 不重新建立描述符。
        } else {
            // 首帧或资源换代。
            let group = context.render_device().create_bind_group(
                Some("dlss_prepass_sources"),
                &cache.get_bind_group_layout(&pipelines.layout),
                &BindGroupEntries::sequential((&depth.default_view, &motion.default_view)),
            ); // 两原始纹理仅采样。
            capture.bindings.rotate_left(1); // 固定两槽替换。
            capture.bindings[1] = Some((key, group.clone())); // 保留最近输入组。
            group // 当前绘制独立强引用。
        }; // 绑定获取结束。
    let images = capture.images.as_ref()?.clone(); // 保持两目标直到标签完成。
    let attachment = |view| {
        Some(RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: Operations {
                load: LoadOp::Clear(wgpu::Color::BLACK),
                store: StoreOp::Store,
            },
        })
    }; // 全图数据覆盖。
    let attachments = [
        attachment(&images.depth.default_view),
        attachment(&images.motion.default_view),
    ]; // MRT输出顺序匹配WGSL位置。
    let descriptor = RenderPassDescriptor {
        label: Some("dlss_prepass_copy_mrt"),
        color_attachments: &attachments,
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }; // 无第二套命令提交。
    {
        // 完成pass之后显式转换数据读取布局。
        let mut pass = context.command_encoder().begin_render_pass(&descriptor); // 由wgpu正确跟踪源采样布局。
        pass.set_pipeline(pipeline); // GPU管线已经就绪。
        pass.set_bind_group(0, &group, &[]); // 只读深度与运动。
        pass.draw(0..3, 0..1); // 单次全屏处理，无CPU像素循环。
    } // 转存pass结束。
    context.command_encoder().transition_resources(
        core::iter::empty(),
        [&images.depth.texture, &images.motion.texture]
            .into_iter()
            .map(|texture| wgpu::TextureTransition {
                texture: &**texture,
                selector: None,
                state: wgpu::TextureUses::RESOURCE,
            }),
    ); // 两图均以SHADER_READ_ONLY_OPTIMAL及COLOR aspect交给SDK。
    Some(images) // SDK可合法执行COPY_SRC，随后恢复该只读布局。
} // 转存接口结束。
