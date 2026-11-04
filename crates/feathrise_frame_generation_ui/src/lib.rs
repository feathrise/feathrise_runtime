//! SDK 无关的帧生成透明 UI 事务；后端只消费同帧 GPU 图像，默认恢复完整真实帧。
mod resources; // 跨后端请求、同帧许可与捕获结果。
mod types; // 后端身份、调度锚点与帧身份。
mod ui; // 透明捕获、空间合成和普通输出恢复。
mod world_ui; // 有界世界标签分流及原深度绘制。
use bevy::{
    prelude::*,
    render::{RenderApp, extract_resource::ExtractResourcePlugin},
};
pub use resources::{FrameGenerationUiRequest, UiCaptureResult, UiFramePermission}; // 两个后端共享的明确事务接口。
pub use types::{
    FrameGenerationUiBackend, FrameGenerationUiRequestSystems, FrameGenerationUiSystems, UiFrame,
}; // 身份与调度公开，SDK ABI 留在后端。
pub use world_ui::{WorldUiFrameGate, WorldUiLayer}; // 后端场景输入必须检查本帧世界标签完整性。 // 唯一插件拥有公共渲染资源。
pub struct FrameGenerationUiPlugin; // 两个后端均先检查是否已安装，防止重复 phase 和目标翻转。
impl Plugin for FrameGenerationUiPlugin {
    // 注册一次主世界与 RenderApp 子域。
    fn build(&self, app: &mut App) {
        // 不触碰任何厂商 SDK。
        app.init_resource::<FrameGenerationUiRequest>()
            .add_plugins(ExtractResourcePlugin::<FrameGenerationUiRequest>::default()); // 每帧只提取固定大小资格。
        ui::load_shader(app); // 编译期内嵌共享 shader，运行期不读文件。
        world_ui::install(app); // 世界 phase 和字形引导标记只有一个所有者。
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            ui::install(render_app);
        } // 无 GPU 测试允许跳过渲染子域。
    } // 插件构建结束。
} // 插件实现结束。
