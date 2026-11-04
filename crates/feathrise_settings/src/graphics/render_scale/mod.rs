//! 窗口 3D 主通道固定渲染比例与原生分辨率空间放大。

mod node; // Core3d 后处理链中的四采样双线性空间放大。
mod pipeline; // 按 LDR/HDR 目标格式专用化的全屏渲染管线。
mod systems; // 主世界相机接管与 RenderApp 视图准备。

use bevy::{
    asset::embedded_asset, // 把 WGSL 随 crate 嵌入，避免新增外部美术资源与路径依赖。
    core_pipeline::{Core3dSystems, schedule::Core3d}, // 空间放大位于早期效果和原生后处理之间。
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        extract_component::{ExtractComponentPlugin, UniformComponentPlugin},
        render_resource::SpecializedRenderPipelines,
        view::prepare_view_targets,
    },
};

use crate::components::RenderScaleView;

pub(crate) use systems::apply_render_scale; // 保持 GraphicsSettingsPlugin 单一主世界注册入口。

/// 注册渲染比例的提取、逐视图 uniform、管线准备和空间放大阶段。
pub(crate) fn configure_render_scale(app: &mut App) {
    embedded_asset!(app, "render_scale.wgsl"); // 发布构建不依赖 assets 目录中的松散着色器文件。
    app.add_plugins((
        ExtractComponentPlugin::<RenderScaleView>::default(), // 只提取当前被画质功能接管的窗口 3D 相机。
        UniformComponentPlugin::<RenderScaleView>::default(), // 每个接管视图上传低分辨率和完整输出尺寸。
    ));

    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return; // 无渲染子应用的纯逻辑测试保持可运行。
    };

    render_app
        .init_resource::<SpecializedRenderPipelines<pipeline::RenderScalePipeline>>()
        .add_systems(RenderStartup, pipeline::init_render_scale_pipeline)
        .add_systems(
            Render,
            systems::prepare_render_scale_views
                .in_set(RenderSystems::PrepareViews)
                .before(prepare_view_targets), // 主通道附件和预通道读取尺寸前必须写入覆盖值。
        )
        .add_systems(
            Core3d,
            node::spatial_upscale
                .after(Core3dSystems::EarlyPostProcess)
                .before(Core3dSystems::PostProcess), // 先扩展到原生尺寸，再运行 Bloom、色调映射、FXAA、CAS 和 UI。
        );
}
