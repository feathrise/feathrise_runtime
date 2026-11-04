//! FSR独立UI子域：捕获真实透明覆盖层供SDK合成，登记失败则还原完整普通输出，不从颜色差推测Alpha。
mod helpers; // 有限纹理缓存、绑定和单次全屏绘制工具。
mod resources; // GPU管线、持久图像与本帧分离状态。
mod systems; // 明确排在原生Bevy UI前后的捕获与恢复系统。
mod types; // 跨系统的小型GPU资源记录。
mod world_composite; // 世界UI透明层仅做当前帧空间采样，并在分离失败时回到普通场景。
pub(crate) use world_composite::WorldUiCompositeReady; // 世界3D阶段以此阻止管线预热期间提前分流标签。

use bevy::core_pipeline::{schedule::Core3d, upscaling::upscaling};
use bevy::{app::SubApp, prelude::*, render::RenderStartup}; // 主世界内嵌资产和RenderApp安装。 // 主3D调度及最终输出锚点。

pub(crate) fn load_shader(app: &mut App) {
    // 在主世界建立Shader资产，供异步PipelineCache编译。
    world_composite::load_shader(app); // 同时预热世界标签的非时间域空间采样Shader。
    bevy::asset::load_internal_asset!(
        app,
        resources::UI_SHADER,
        "composite.wgsl",
        bevy::shader::Shader::from_wgsl
    ); // 编译期内嵌，正式运行不依赖作者文件路径。
} // 内嵌着色器注册结束。

pub(super) fn install(app: &mut SubApp) {
    // 只安装有界资源和两个主视图系统。
    world_composite::install(app); // 世界UI先合到当前目标，后续原生2DUI继续保持原绘制顺序。
    use crate::FrameGenerationUiSystems as Ui; // 后端通过公共锚点挂接准备和同帧图像登记。
    app.init_resource::<resources::UiCapture>() // 只保存最近规格的一张UI纹理与两张主纹理绑定。
        .init_resource::<crate::UiFramePermission>() // 后端场景输入成功才授权当前帧。
        .init_resource::<crate::UiCaptureResult>() // 完整结果在 Capture 后才可读取。
        .configure_sets(
            Core3d,
            (
                Ui::Prepare,
                Ui::Begin,
                Ui::Capture,
                Ui::BackendFinalize,
                Ui::Restore,
            )
                .chain(),
        ) // 同一视图事务按阶段顺序执行。
        .configure_sets(
            Core3d,
            Ui::Prepare.after(bevy::core_pipeline::Core3dSystems::PostProcess),
        ) // 无 UI 场景必须等三维后处理完成。
        .configure_sets(
            Core3d,
            Ui::Begin.before(bevy::ui_render::ui_pass),
        ) // 世界标签与原生二维UI共享同一透明目标，不再注册第二套UI后端。
        .configure_sets(
            Core3d,
            Ui::Capture.after(bevy::ui_render::ui_pass),
        ) // 原生UI全部绘制完成后发布，WorldUI仍在ui_pass前合成。
        .configure_sets(Core3d, Ui::Restore.before(upscaling)) // 真实输出在最终交换链写入前完整恢复。
        .add_systems(RenderStartup, systems::prepare_pipelines) // 固定三条GPU管线排入后台编译，不等待GPU或CPU任务。
        .add_systems(Core3d, systems::begin_ui.in_set(Ui::Begin)) // 世界标签及原生UI开始前将当前主纹理切到透明层。
        .add_systems(Core3d, systems::capture_ui.in_set(Ui::Capture)) // 完整透明图像发布给本帧后端。
        .add_systems(Core3d, systems::restore_ui.in_set(Ui::Restore)); // 默认恢复完整真实帧，FSR成功可接管合成。
} // UI调度安装结束。
