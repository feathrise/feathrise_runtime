//! 独立UI的固定纹理、采样绑定和全屏绘制辅助；不包含逐像素CPU工作。
use super::{
    resources::UiCapture,
    types::{UiBinding, UiTargets},
};
use bevy::{
    prelude::*,
    render::{
        render_resource::*,
        renderer::{RenderContext, RenderDevice},
        texture::TextureCache,
    },
}; // GPU缓存与只提交命令的绘制上下文。 // 有限缓存记录。

pub(super) fn capture_targets(
    cache: &mut TextureCache,
    device: &RenderDevice,
    size: UVec2,
    format: TextureFormat,
) -> Option<UiTargets> {
    // 只捕获SDK实际读取的UI；scene沿现有前缓冲输出，不重复分配和编码。
    let descriptor = |label| TextureDescriptor {
        // 固定双纹理规格，不复制CPU像素。
        label: Some(label), // 独立UI缓存标签保持资源身份可追踪。
        size: Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        }, // 同尺寸单层输出。
        mip_level_count: 1, // SDK合成只读取基础级。
        sample_count: 1,    // 最终输出与透明层均已解析采样。
        dimension: TextureDimension::D2, // 普通二维窗口纹理。
        format,             // 自有sRGB纹理编码成最终窗口色彩空间，不借用代理交换链的可变格式能力。
        usage: TextureUsages::RENDER_ATTACHMENT
            | TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_SRC, // GPU编码、SDK采样/内部复制的完整用途。
        view_formats: &[],  // 不混用UNORM view或隐式可变格式。
    }; // 固定描述构造结束。
    let ui = cache.get(device, descriptor("frame_generation_premultiplied_ui")); // Alpha来自透明底真实UI绘制。
    Some(UiTargets { size, format, ui }) // 持久Resource继续保留强引用到交换链安全重建。
} // 目标获取结束。

pub(super) fn binding(
    capture: &mut UiCapture,
    view: &TextureView,
    layout: &BindGroupLayoutDescriptor,
    device: &RenderDevice,
    cache: &PipelineCache,
) -> BindGroup {
    // A/B两张主纹理至多缓存两个绑定。
    if let Some(found) = capture
        .bindings
        .iter()
        .flatten()
        .find(|entry| entry.view == view.id())
    {
        return found.group.clone();
    } // 稳定帧只克隆廉价GPU句柄。
    let group = device.create_bind_group(
        Some("frame_generation_ui_source"),
        &cache.get_bind_group_layout(layout),
        &BindGroupEntries::single(view),
    ); // 只在首次或尺寸重建更换纹理身份时创建绑定。
    capture.bindings.rotate_left(1); // 固定两槽保留最近两个不同视图，resize后不会因旧槽残留逐帧重建。
    capture.bindings[1] = Some(UiBinding {
        view: view.id(),
        group: group.clone(),
    }); // 原绑定仍可由当前帧强引用安全持有。
    group // 返回给本帧事务，不借用可变缓存。
} // 绑定复用结束。

pub(super) fn draw(
    context: &mut RenderContext,
    pipeline: &RenderPipeline,
    group: &BindGroup,
    destination: &TextureView,
    load: LoadOp<wgpu::Color>,
) {
    // 单个全屏三角形复制或预乘合成。
    let attachment = RenderPassColorAttachment {
        view: destination,
        depth_slice: None,
        resolve_target: None,
        ops: Operations {
            load,
            store: StoreOp::Store,
        },
    }; // 每次完整保存目标，透明清理只在明确入口发生。
    let attachments = [Some(attachment)]; // 固定一个颜色附件。
    let descriptor = RenderPassDescriptor {
        label: Some("frame_generation_ui_capture_or_restore"),
        color_attachments: &attachments,
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }; // 不接管主场景深度或MSAA。
    let mut pass = context.command_encoder().begin_render_pass(&descriptor); // 原生层尚未借用命令缓冲，保持普通wgpu资源跟踪。
    pass.set_pipeline(pipeline); // begin前已经验证GPU管线可用。
    pass.set_bind_group(0, group, &[]); // 精确采样同帧场景或透明UI纹理。
    pass.draw(0..3, 0..1); // 固定三顶点，不执行任何CPU像素批处理。
} // 全屏绘制结束。

pub(super) fn clear(context: &mut RenderContext, destination: &TextureView) {
    // 透明底是Alpha真实性的来源，不能从最终颜色相减恢复。
    let attachment = RenderPassColorAttachment {
        view: destination,
        depth_slice: None,
        resolve_target: None,
        ops: Operations {
            load: LoadOp::Clear(wgpu::Color::TRANSPARENT),
            store: StoreOp::Store,
        },
    }; // 明确清空RGBA四通道。
    let attachments = [Some(attachment)]; // 固定单颜色附件。
    let descriptor = RenderPassDescriptor {
        label: Some("frame_generation_clear_real_ui_layer"),
        color_attachments: &attachments,
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }; // 后续原生Bevy UI会Load这张透明纹理，WorldUI沿用同一捕获目标。
    context.command_encoder().begin_render_pass(&descriptor); // 不需要额外clear shader或CPU写入。
} // 透明底清理结束。
