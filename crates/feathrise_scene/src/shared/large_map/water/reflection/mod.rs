//! 水面SSR视图接线：复用标准视图颜色绑定，每视图只复制一次已完成场景颜色。
use bevy::core_pipeline::{
    core_3d::main_transparent_pass_3d,
    schedule::{Core3d, Core3dSystems},
}; // Bevy0.19按视图渲染调度。
use bevy::pbr::{main_transmissive_pass_3d, prepare_mesh_view_bind_groups}; // 颜色捕获位于折射结束后、透明水面开始前。
use bevy::prelude::*; // 主世界插件装配。
use bevy::render::{Render, RenderApp, RenderSystems}; // GPU资源准备与逐视图绘制调度。

mod camera; // 主世界相机资格与HDR/DepthPrepass租约。
mod components; // 只记录本功能创建的GPU纹理所有权。
mod render; // 准备颜色纹理、撤销租约和一次GPU复制。
mod types; // 纯纹理规格与所有权决策类型。

pub(super) fn install(app: &mut App) {
    // 水插件在全局渲染插件就绪后调用一次。
    camera::install(app); // 主世界维护资格并提取到RenderApp。
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    }; // 无窗口回归可以不创建渲染世界。
    render_app.add_systems(
        Render,
        render::prepare_reflection_textures
            .in_set(RenderSystems::PrepareBindGroups)
            .before(prepare_mesh_view_bind_groups),
    ); // 标准PrepareResources完成后补图，绑定组准备读取本帧完整组件。
    render_app.add_systems(
        Core3d,
        render::capture_reflection_color
            .after(main_transmissive_pass_3d)
            .before(main_transparent_pass_3d)
            .in_set(Core3dSystems::MainPass),
    ); // 已解析场景颜色先复制，透明水绘制再采样，避免读写同纹理。
} // 结束水面SSR基础设施安装。
