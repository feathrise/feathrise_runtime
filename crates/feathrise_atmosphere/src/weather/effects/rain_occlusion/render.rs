//! 局部雨向深度发布：复用 Bevy 深度预通道，在一次 GPU 绘制中原子写入矩阵和深度。

use bevy::asset::{load_internal_asset, uuid_handle}; // 内置 Shader 随 crate 部署。
use bevy::core_pipeline::{Core3dSystems, FullscreenShader, schedule::Core3d}; // 在屋顶深度完成后发布。
use bevy::prelude::*; // 资源、矩阵、资产及调度接口。
use bevy::render::render_asset::RenderAssets; // 取得目标图片的 GPU 实例。
use bevy::render::render_resource::{
    binding_types::{texture_depth_2d, uniform_buffer},
    *,
}; // 固定深度及矩阵绑定。
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery}; // 非阻塞 GPU 编码。
use bevy::render::texture::GpuImage; // 已准备的目标纹理。
use bevy::render::view::ViewDepthTexture; // 独立雨向相机预通道深度。
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems}; // 只安装 RenderApp 调度。

use super::components::RainOcclusionCamera; // 仅处理专用深度相机。
use super::resources::RainOcclusionMap; // 主世界发布的完整雨向投影需求。

const CONVERT_SHADER: Handle<Shader> = uuid_handle!("d5fd63aa-3bb6-41d8-9103-19609babb82c"); // 转换专用稳定句柄。

#[derive(Clone, Copy, ShaderType)] // 单个矩阵固定 64 字节，无内容规模相关上传。
struct CaptureUniform {
    // 元数据和深度必须在同一次 GPU pass 中采用。
    world_to_rain: Mat4, // 世界到雨向 clip 坐标，与相机投影完全一致。
} // 固定 uniform 布局结束。

#[derive(Resource)] // 所有局部深度更新复用唯一管线和固定 uniform。
struct CapturePipeline {
    // 不复制场景网格，不维护第二套材质渲染阶段。
    layout: BindGroupLayoutDescriptor, // 深度纹理和矩阵 uniform 两个绑定。
    pipeline: CachedRenderPipelineId,  // 异步编译未完成时保持上次完整图。
    uniform: UniformBuffer<CaptureUniform>, // 缓冲区初始化后保持身份稳定。
} // 管线状态结束。

#[derive(Default)] // 单相机最多缓存一个绑定组，空间固定。
struct CaptureBindGroupCache {
    // 输入输出纹理换代时才重建。
    cached: Option<(TextureViewId, TextureViewId, BindGroup)>, // 源、目标身份及绑定组。
} // 缓存结构结束。

pub(super) fn install(app: &mut App) {
    // 父插件负责资源及相机组件提取。
    load_internal_asset!(
        app,
        CONVERT_SHADER,
        "render/convert.wgsl",
        Shader::from_wgsl
    ); // 编译期嵌入转换 Shader。
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    }; // 无窗口 ECS 测试不要求 GPU。
    render_app.add_systems(RenderStartup, initialize_pipeline); // 初始化一次固定转换管线。
    render_app.add_systems(
        Render,
        prepare_capture.in_set(RenderSystems::PrepareResources),
    ); // 每次仅上传固定矩阵。
    render_app.add_systems(
        Core3d,
        publish_depth
            .after(Core3dSystems::Prepass)
            .before(Core3dSystems::MainPass),
    ); // 先发布，再供后续主相机雨丝读取。
} // 安装结束。

fn initialize_pipeline(
    // GPU 管线与持久 uniform 仅初始化一次。
    mut commands: Commands,            // 发布 RenderApp 管线资源。
    device: Res<RenderDevice>,         // 创建固定缓冲区。
    queue: Res<RenderQueue>,           // 初始 uniform 上传。
    fullscreen: Res<FullscreenShader>, // 复用 Bevy 全屏三角形。
    cache: Res<PipelineCache>,         // Shader 编译保持非阻塞。
) {
    // 初始化逻辑开始。
    let layout = BindGroupLayoutDescriptor::new(
        "rain_depth_capture_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (texture_depth_2d(), uniform_buffer::<CaptureUniform>(false)),
        ),
    ); // 深度无需采样器与滤波。
    let mut uniform = UniformBuffer::from(CaptureUniform {
        world_to_rain: Mat4::IDENTITY,
    }); // 单矩阵固定容量。
    uniform.write_buffer(&device, &queue); // 提前分配，后续更新不改变绑定身份。
    let pipeline = cache.queue_render_pipeline(RenderPipelineDescriptor {
        // 首次编译在缓存中异步完成。
        label: Some("rain_depth_capture".into()), // GPU 调试标记。
        layout: vec![layout.clone()],             // 唯一绑定组。
        vertex: fullscreen.to_vertex_state(),     // 固定三个顶点，无 CPU 网格构造。
        fragment: Some(FragmentState {
            // 同一绘制同时覆盖 metadata 与所有深度像素。
            shader: CONVERT_SHADER.clone(), // 内置转换 Shader。
            targets: vec![Some(ColorTargetState {
                format: TextureFormat::Rgba16Float,
                blend: None,
                write_mask: ColorWrites::ALL,
            })], // filterable 格式内按整数位无损保存 f32。
            ..default()                     // 使用 fragment 默认入口。
        }), // 片元阶段结束。
        ..default()                               // 不启用深度测试或多重采样。
    }); // 管线请求结束。
    commands.insert_resource(CapturePipeline {
        layout,
        pipeline,
        uniform,
    }); // 管线和缓冲区共同发布。
} // 初始化结束。

fn prepare_capture(
    // 提取后的最新矩阵仅固定大小上传。
    map: Option<Res<RainOcclusionMap>>, // 纯 ECS 或停用流程可以没有地图。
    pipeline: Option<ResMut<CapturePipeline>>, // RenderStartup 完成前无需操作。
    device: Res<RenderDevice>,          // 固定 uniform 写入接口。
    queue: Res<RenderQueue>,            // 只编码上传，不等待 GPU。
) {
    // 准备开始。
    let (Some(map), Some(mut pipeline)) = (map, pipeline) else {
        return;
    }; // 初始化缺失时保留安全空图。
    if !map.active || !map.is_changed() {
        return;
    } // 静态场景与停雨不重复上传。
    pipeline.uniform.set(CaptureUniform {
        world_to_rain: map.world_to_rain,
    }); // 最新矩阵随本帧深度一起发布。
    pipeline.uniform.write_buffer(&device, &queue); // 固定 64 字节上传，无回读或等待。
} // 准备结束。

fn publish_depth(
    // 每个深度相机完成后执行一个固定全屏转换。
    view: ViewQuery<(&RainOcclusionCamera, &ViewDepthTexture)>, // 主相机不匹配，不产生额外 pass。
    map: Option<Res<RainOcclusionMap>>,                         // 同次提取的目标纹理及投影。
    pipeline: Option<Res<CapturePipeline>>,                     // 异步管线可暂时缺失。
    pipelines: Res<PipelineCache>,                              // 查询就绪状态，不阻塞编译。
    images: Res<RenderAssets<GpuImage>>,                        // 目标纹理换代后等待正常资产准备。
    mut bindings: Local<CaptureBindGroupCache>,                 // 静态纹理不重建绑定组。
    mut context: RenderContext, // 使用同一提交顺序完成深度与元数据。
) {
    // 发布开始。
    let (Some(map), Some(pipeline)) = (map, pipeline) else {
        return;
    }; // 尚未初始化时不发布半结果。
    if !map.active {
        return;
    } // 停雨不会更新缓存。
    let Some(compiled) = pipelines.get_render_pipeline(pipeline.pipeline) else {
        return;
    }; // 管线未就绪时保留旧完整图。
    let Some(target) = images.get(map.image.id()) else {
        return;
    }; // 新地图图片未就绪时不复用旧目标。
    let Some(uniform) = pipeline.uniform.binding() else {
        return;
    }; // 固定缓冲初始化前安全退出。
    let (_, depth) = view.into_inner(); // 专用相机的本帧深度。
    let source_id = depth.view().id(); // 缓存必须跟踪 Bevy 深度池纹理换代。
    let target_id = target.texture_view.id(); // 目标换代同样使缓存失效。
    let bind_group = match &mut bindings.cached {
        // 最多保持一个局部视图的绑定组。
        Some((source, destination, group)) if *source == source_id && *destination == target_id => {
            group
        } // 原纹理复用绑定。
        slot => {
            // 纹理身份真实改变时重建。
            let group = context.render_device().create_bind_group(
                "rain_depth_capture_bindings",
                &pipelines.get_bind_group_layout(&pipeline.layout),
                &BindGroupEntries::sequential((depth.view(), uniform)),
            ); // 只绑定GPU深度和固定矩阵。
            &mut slot.insert((source_id, target_id, group)).2 // 保存新身份，避免每帧分配。
        } // 缓存更新结束。
    }; // 得到本次有效绑定组。
    let attachments = [Some(RenderPassColorAttachment {
        // 独立目标与源深度不存在读写冲突。
        view: &target.texture_view, // N×(N+1)，首行仅用于完整元数据。
        depth_slice: None,          // 普通二维纹理。
        resolve_target: None,       // 无 MSAA。
        ops: Operations {
            load: LoadOp::Clear(LinearRgba::NONE.into()),
            store: StoreOp::Store,
        }, // 无效背景 Alpha 为零，完整绘制后发布有效 Alpha。
    })]; // 单个颜色附件。
    let mut pass = context
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            // 一次 pass 原子替换元数据及深度。
            label: Some("rain_depth_capture_publish"), // GPU 分析器可单独测量。
            color_attachments: &attachments,           // 只输出共享遮雨图片。
            depth_stencil_attachment: None,            // 已有深度只读，无需额外深度附件。
            timestamp_writes: None,                    // 不引入等待或读回。
            occlusion_query_set: None,                 // 固定全屏绘制无需查询。
            multiview_mask: None,                      // 唯一雨向视图。
        }); // 编码转换绘制。
    pass.set_pipeline(compiled); // 已确认编译就绪的管线。
    pass.set_bind_group(0, bind_group, &[]); // 固定 uniform 不使用动态偏移。
    pass.draw(0..3, 0..1); // 三角形覆盖深度和 metadata 全图。
} // pass 结束后后续相机可采样完整结果。

#[cfg(test)] // 编码与 Shader 验证无需启动游戏。
mod tests; // 单独文件保持发布路径清晰。

#[cfg(test)] // 真实GPU烟雾显式ignored，不影响常规无窗口回归。
mod gpu_tests; // 复用生产捕获管线验证屋顶与天井。
