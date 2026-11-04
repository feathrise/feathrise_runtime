//! FSR 的 SDK 薄适配；共享层负责完整透明捕获与普通输出回退，只有登记成功才交给原生 Present 合成。
use super::{CameraHistory, RenderState, fail_frame, native_image}; // FSR自己的失败、历史与原生图像转换。
use crate::{
    native,
    runtime::{RenderFeedback, RenderRequest},
    status::Fsr3FrameGenerationInputFailure,
};
use bevy::{
    app::SubApp,
    core_pipeline::schedule::Core3d,
    diagnostic::FrameCount,
    prelude::*,
    render::{renderer::ViewQuery, view::ExtractedView},
}; // 当前视图和同帧身份。
use feathrise_frame_generation_ui::{
    FrameGenerationUiSystems, UiCaptureResult, UiFramePermission,
}; // 不复制共享GPU事务。 // 原生ABI与公开失败原因留在FSR。
pub(super) fn install(app: &mut SubApp) {
    // 公共插件已由runtime安装一次。
    app.add_systems(
        Core3d,
        finalize.in_set(FrameGenerationUiSystems::BackendFinalize),
    ); // Capture之后/普通Restore之前登记。
} // FSR薄适配安装结束。
#[allow(clippy::too_many_arguments)] // 保留每个事务身份与反馈的显式所有权。
fn finalize(
    // UI未完整就绪时必须撤销本帧FSR Prepare。
    view: ViewQuery<&ExtractedView>,      // 只消费当前唯一主相机。
    frame: Res<FrameCount>,               // 本次Prepare与捕获必须同帧。
    request: Res<RenderRequest>,          // 不活动的后端不得碰共享结果。
    permission: Res<UiFramePermission>,   // 只有本次成功Prepare才需要收尾。
    mut capture: ResMut<UiCaptureResult>, // 成功后精确声明SceneOnly。
    feedback: Res<RenderFeedback>,        // 完整成功必须等原生UI登记。
    mut state: ResMut<RenderState>,       // 失败撤销旧Prepare。
    mut history: ResMut<CameraHistory>,   // 失败使下一帧重置时间历史。
) {
    // 后端收尾入口。
    let owner = view.into_inner().retained_view_entity.main_entity.id(); // 主世界实体身份。
    if !request.enabled || request.camera != Some(owner) || !permission.allows(frame.0, owner) {
        return;
    } // 辅助相机和失败Prepare不重复登记。
    let image = capture.current(frame.0, owner).and_then(|ui| {
        native_image(
            &ui.texture.texture,
            &ui.texture.default_view,
            ash::vk::ImageAspectFlags::COLOR,
        )
    }); // 当前真实透明图像转换为FSR ABI。
    let registered = image
        .as_ref()
        .is_some_and(|image| native::set_ui_resources(image) == native::RESULT_OK); // 原生验证交换链尺寸/格式及完整输入。
    if !registered {
        // 默认共享Restore会恢复完整真实帧，不能留下纯场景。
        fail_frame(
            frame.0,
            if image.is_none() {
                Fsr3FrameGenerationInputFailure::Pending
            } else {
                Fsr3FrameGenerationInputFailure::PrepareCall
            },
            &feedback,
            &mut state,
            &mut history,
        ); // 不完整UI不得宣称Prepare成功。
        return; // 不授权SceneOnly。
    } // 原生登记成功门结束。
    if capture.keep_scene_only(frame.0, owner) {
        // FSR原生Present负责给真实帧与生成帧各合成一次。
        if let Some(previous) = state.failure.take() {
            info!(
                "FSR 3.1帧生成完整输入已恢复，上一缺项：{}",
                previous.message()
            );
        } // 完整跨过成功边界才报告恢复。
        feedback.mark_prepared(frame.0); // 共享结果已完整且原生已经接受。
    } // 当前FSR合成所有权发布结束。
} // FSR后端收尾结束。
