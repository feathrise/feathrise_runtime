//! DLSS独立UI薄适配：共享捕获层完成后提交Streamline标签，完整成功才允许固定2x。
use super::{CameraHistory, RuntimeState, fail_frame, native_texture, record_native}; // 复用唯一场景事务、原生编码和失败回退。
use crate::{
    native,
    runtime::{RenderFeedback, RenderRequest},
};
use ash::vk; // 只读取当前图像的原生布局与子资源身份。
use bevy::{
    app::SubApp,
    core_pipeline::schedule::Core3d,
    diagnostic::FrameCount,
    prelude::*,
    render::{
        renderer::{RenderContext, ViewQuery},
        view::ExtractedView,
    },
}; // 同帧最终提交运行在RenderApp。
use feathrise_frame_generation_ui::{FrameGenerationUiSystems, UiCaptureResult}; // 不重复实现原生二维UI或世界标签绘制。 // SDK窄桥与主世界的明确资格。

pub(super) fn install(app: &mut SubApp) {
    // 只有共享捕获完成后才执行SDK最终提交。
    app.add_systems(
        Core3d,
        finalize.in_set(FrameGenerationUiSystems::BackendFinalize),
    ); // 公共Restore随后始终恢复完整真实帧。
} // 安装结束。

fn finalize(
    // 输入失败同样执行，以关闭上一帧可能仍开启的SDK。
    view: ViewQuery<&ExtractedView>,    // 精确匹配本帧主世界相机。
    request: Res<RenderRequest>,        // 辅助相机不能消费当前主视图事务。
    frame: Res<FrameCount>,             // UI、场景和SDK共享同一帧号。
    capture: Res<UiCaptureResult>,      // 共享层只发布完整图像，不暴露半成品。
    feedback: Res<RenderFeedback>,      // 只有完整提交后才发布成功。
    mut state: ResMut<RuntimeState>,    // 全部输入原子采用的唯一状态机。
    mut history: ResMut<CameraHistory>, // UI失败时清空时间连续性。
    mut context: RenderContext,         // 易失资源复制必须追加到当前队列命令缓冲。
) {
    // 当前主视图最终提交。
    let owner = view.into_inner().retained_view_entity.main_entity.id(); // 取得主世界稳定实体。
    if !request.enabled || request.camera != Some(owner) {
        return;
    } // 非本次后端拥有的视图零SDK调用。
    let Some(ui) = capture
        .current(frame.0, owner)
        .filter(|_| state.prepared == Some((frame.0, owner)))
    else {
        // 必须是同帧完整场景与UI。
        fail_frame(frame.0, &feedback, &mut state, &mut history);
        return; // 未完成、失败和迟到UI都安全关闭FG。
    }; // UI身份门结束。
    let Some(signature) = state.signature else {
        fail_frame(frame.0, &feedback, &mut state, &mut history);
        return;
    }; // 无选项规格不能标签。
    let Some(ui_native) = native_texture(
        &ui.texture.texture,
        &ui.texture.default_view,
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        vk::ImageAspectFlags::COLOR,
    ) else {
        // 只借用共享层强引用拥有的真实VkImage。
        fail_frame(frame.0, &feedback, &mut state, &mut history);
        return; // 不支持原生互操作时保留普通UI。
    }; // 原生描述门结束。
    if ui_native.width != signature.output_width
        || ui_native.height != signature.output_height
        || ui_native.format != signature.ui_format
    {
        // 拒绝尺寸变化与格式不匹配的快照。
        fail_frame(frame.0, &feedback, &mut state, &mut history);
        return; // 当前帧完整回退，下次重新预热。
    } // 规格门结束。
    context.command_encoder().transition_resources(
        core::iter::empty(),
        core::iter::once(wgpu::TextureTransition {
            texture: &*ui.texture.texture,
            selector: None,
            state: wgpu::TextureUses::RESOURCE,
        }),
    ); // wgpu记录读用途及布局后原生只读复制。
    let result = record_native(&mut context, "dlss_fg_ui_tag", |command_buffer| {
        native::tag_ui(frame.0, command_buffer, &ui_native)
    }); // 独立原生编码器接在UI转换屏障之后，避免wgpu29混合编码断言。
    if result != Some(native::RESULT_OK) {
        // SDK复制真实预乘UI，不持有跨帧裸借用。
        fail_frame(frame.0, &feedback, &mut state, &mut history);
        return; // SDK拒绝时公共Restore仍会显示完整真实UI。
    } // 最后一项标签门结束。
    if state.warmup_complete && !state.plugin_enabled {
        // 首帧只积累完整输入，后续才开启固定2x。
        if signature.set_enabled(true) != native::RESULT_OK {
            fail_frame(frame.0, &feedback, &mut state, &mut history);
            return;
        } // 失败不能发布假成功。
        state.plugin_enabled = true; // 实际2x仍以Present反馈为准。
    } // 插件开启结束。
    state.warmup_complete = true; // 本帧深度、运动、场景和UI全部就绪。
    state.prepared = None; // 完整结果只提交一次。
    state.failure_site = None; // 完整恢复后允许后续独立故障重新报告。
    state.failure_frame = None; // 不把本次成功误当失败帧。
    feedback.mark_tagged(frame.0); // 原子发布完整输入成功事实。
} // 默认不申请SceneOnly，Streamline接收的真实Backbuffer仍包含UI。
