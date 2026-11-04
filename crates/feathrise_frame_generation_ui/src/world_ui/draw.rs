//! 在时间超分前按原主视图深度、输出分辨率和无抖动投影绘制世界 UI；独立绘制失败时回绘普通主颜色并阻断本帧 FG。
use super::{components::WorldUiLayer, types::WorldUi3d}; // 仅消费本帧准备的唯一世界 UI 事务。
use bevy::render::render_resource::RenderPassDepthStencilAttachment;
use bevy::{
    core_pipeline::core_3d::CORE_3D_DEPTH_FORMAT,
    diagnostic::FrameCount,
    prelude::*,
    render::{
        render_phase::ViewSortedRenderPhases,
        render_resource::{
            LoadOp, Operations, RenderPassColorAttachment, RenderPassDescriptor, StoreOp,
        },
        renderer::{RenderContext, ViewQuery},
        view::{ExtractedView, ViewDepthTexture, ViewTarget},
    },
}; // 公开 DrawMaterial phase 保持材质、实例、深度与相机绑定不变。
use std::sync::atomic::Ordering; // 同队列完成事实只有固定原子写入，无线程等待。 // 名牌颜色使用已映射输出深度。

pub(super) fn draw_world_ui(
    // Core3d MainPass 内运行，早于时间超分和场景色调映射。
    world: &World, // Bevy phase DrawFunctions 的公开绘制接口需要只读 RenderWorld。
    view: ViewQuery<(
        &ExtractedView,
        &ViewTarget,
        &ViewDepthTexture,
        Option<&WorldUiLayer>,
    )>, // 主深度用于失败回退，独立颜色使用本帧映射深度。
    phases: Res<ViewSortedRenderPhases<WorldUi3d>>, // 已完成独立排序和 GPU batching 的 UI 项。
    frame: Res<FrameCount>, // 旧相机或旧纹理内容不能参与当前输出。
    mut traced: Local<bool>, // 首次真实非空绘制只输出一条诊断，稳定帧不增加日志开销。
    mut context: RenderContext, // 普通 wgpu 渲染命令，维持完整资源布局和生命周期跟踪。
) {
    // 世界 UI 绘制入口。
    let view_entity = view.entity(); // DrawMaterial 必须绑定原主视图实例数据。
    let (extracted, target, depth, layer) = view.into_inner(); // 不创建替代相机或猜测私有绑定布局。
    let Some(layer) = layer.filter(|layer| {
        layer.frame == frame.0 && layer.camera == extracted.retained_view_entity.main_entity.id()
    }) else {
        return;
    }; // 只有本帧实际分流过的视图才执行。
    let Some(phase) = phases.get(&extracted.retained_view_entity) else {
        return;
    }; // 正常关闭路径保留原 Transparent3d。
    if phase.items.is_empty() {
        return;
    } // 不提交空透明 pass。
    let depth_compatible = depth.texture.format() == CORE_3D_DEPTH_FORMAT
        && depth.texture.sample_count() == 1
        && depth.texture.width() == layer.output_size.x
        && depth.texture.height() == layer.output_size.y; // attachment 必须与原始单采样 HDR 主 pass 完全兼容。
    let mut complete = false; // 没有完整独立绘制证据时不得交给 UI 合成。
    if depth_compatible && layer.depth_drawn.load(Ordering::Acquire) {
        // 仅消费本帧完整重映射深度，失败仍回绘普通场景。
        let mut pass = context.begin_tracked_render_pass(RenderPassDescriptor {
            // 复用主视图所有材质 pipeline，只更换颜色附件。
            label: Some("frame_generation_world_ui_transparent"), // 显示世界 UI 被实际剥离的 GPU 证据。
            color_attachments: &[Some(RenderPassColorAttachment {
                // 透明层采用与原材质一致的线性 RGBA16F。
                view: &layer.texture.default_view, // 当前视图唯一捕获纹理。
                depth_slice: None,                 // 二维纹理无体积切片。
                resolve_target: None,              // 分流门已经强制单采样。
                ops: Operations {
                    load: LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: StoreOp::Store,
                }, // 从透明零开始累积，得到真实预乘 alpha。
            })], // 独立颜色附件结束。
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: &layer.mapped_depth.default_view,
                depth_ops: Some(Operations {
                    load: LoadOp::Load,
                    store: StoreOp::Store,
                }),
                stencil_ops: None,
            }), // 使用同坐标输出深度；原场景深度保持不变。
            timestamp_writes: None,                               // 不引入等待式 GPU 计时查询。
            occlusion_query_set: None, // UI 使用已有 CPU/GPU 可见性数据。
            multiview_mask: None,      // 唯一普通主视图。
        }); // 独立 pass 记录开始。
        pass.set_viewport(
            0.0,
            0.0,
            layer.render_size.x as f32,
            layer.render_size.y as f32,
            0.0,
            1.0,
        ); // 独立颜色层使用完整输出viewport，避免头顶字形先低分辨率光栅化再放大。
        pass.set_scissor_rect(0, 0, layer.render_size.x, layer.render_size.y); // 输出层全幅有效，空间采样不再读取未写入的低分辨率区域。
        match phase.render(&mut pass, world, view_entity) {
            // 复用公开 DrawMaterial，不复制 PBR shader、layout 或顶点准备。
            Ok(()) => complete = true, // 所有可绘制世界 UI 命令已按原材质录制。
            Err(error) => warn!("帧生成世界 UI 独立绘制失败，恢复普通输出：{error:?}"), // 明确失败而不假报完成。
        } // phase 绘制结果结束。
    } // 深度兼容检查结束。
    if complete {
        // 只有原材质 phase 实际成功记录过非空绘制才发布证据。
        layer.drawn.store(true, Ordering::Release); // 后续 UI 系统才能空间采样此层。
        if !*traced {
            info!(
                "Frame generation independent world UI: items={} render={}x{} internal={}x{} output={}x{}",
                phase.items.len(),
                layer.render_size.x,
                layer.render_size.y,
                layer.internal_size.x,
                layer.internal_size.y,
                layer.output_size.x,
                layer.output_size.y
            );
            *traced = true;
        } // 实机验证可区分真正剥离标签和空纹理/仅编译通过。
        return; // 成功时绝不再向原场景绘制第二遍 UI。
    } // 独立绘制成功结束。
    layer.failed.store(true, Ordering::Release); // 即使恢复了普通标签，本帧也不能重新启用无 UI 的插帧声明。
    let mut pass = context.begin_tracked_render_pass(RenderPassDescriptor {
        // 独立路径失败时让当前主颜色恢复完整世界 UI。
        label: Some("frame_generation_world_ui_ordinary_fallback"), // RenderDoc 与日志可区分恢复 pass。
        color_attachments: &[Some(target.get_color_attachment())], // 原场景 HDR 颜色仍在时间超分之前。
        depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)), // 完全沿用原主透明 pass 的深度附件契约。
        timestamp_writes: None,    // 不等待任何 GPU 结果。
        occlusion_query_set: None, // 不新建查询池。
        multiview_mask: None,      // 保持普通单视图。
    }); // 原主颜色恢复开始。
    pass.set_viewport(
        0.0,
        0.0,
        layer.internal_size.x as f32,
        layer.internal_size.y as f32,
        0.0,
        1.0,
    ); // 恢复与主透明相同的内部区域。
    pass.set_scissor_rect(0, 0, layer.internal_size.x, layer.internal_size.y); // 普通回退同样不写附件无效区域。
    if let Err(error) = phase.render(&mut pass, world, view_entity) {
        warn!("帧生成世界 UI 普通回退也遇到原材质绘制错误：{error:?}");
    } // 保留明确材质错误，不能把异常宣称为有效分离。
} // draw_world_ui 结束。
