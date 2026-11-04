//! 将独立世界UI的当前帧透明层空间重采样到窗口；没有历史、TAA或插帧。
mod helpers;
mod resources; // 固定管线、32字节参数和单个纹理绑定。
mod systems; // 非阻塞预热与主视图合成。
mod types; // CPU/GPU共享的空间采样参数。 // 有效范围与输出尺寸的纯数学映射。

use bevy::core_pipeline::schedule::Core3d; // 与现有UI分离使用同一个主视图调度。
use bevy::{
    app::SubApp,
    prelude::*,
    render::{Render, RenderStartup, RenderSystems},
}; // 启动期资产和RenderApp调度。
pub(crate) use resources::WorldUiCompositeReady; // 世界UI分流必须先确认存在可用合成管线。

pub(super) fn load_shader(app: &mut App) {
    // 编译期内嵌空间采样Shader。
    bevy::asset::load_internal_asset!(
        app,
        resources::WORLD_UI_SHADER,
        "sample.wgsl",
        bevy::shader::Shader::from_wgsl
    ); // 不在运行期读取作者文件。
} // Shader注册结束。

pub(super) fn install(app: &mut SubApp) {
    // 一条固定GPU管线和一个主视图采样系统。
    app.init_resource::<WorldUiCompositeReady>() // 初始false，管线未就绪时世界标签继续普通3D路径。
        .add_systems(RenderStartup, systems::prepare_pipeline) // 固定创建UniformBuffer并排队GPU编译。
        .add_systems(
            Render,
            systems::publish_ready
                .before(RenderSystems::Queue)
                .before(RenderSystems::PrepareResources),
        ) // 在世界UI分流前发布非阻塞完成事实，首帧未就绪仍走普通3D。
        .add_systems(
            Core3d,
            systems::composite
                .after(crate::FrameGenerationUiSystems::Begin)
                .before(bevy::ui_render::ui_pass),
        ); // begin成功则写透明UI，否则写普通scene，始终保留世界标签。
} // 世界UI空间合成安装结束。
