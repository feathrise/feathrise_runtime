//! Solari视图的标准环境贴图漫反射与镜面IBL兼容层。
//!
//! Solari会为活动相机插入`SkipDeferredLighting`，因此Bevy标准Deferred PBR不会消费
//! `EnvironmentMapLight`。本模块在不透明Forward覆盖与既有阴影兼容完成后、透明内容和RR之前，
//! 复用同一MeshView环境贴图绑定，以加法方式只补回Deferred首表面的环境项。
//! 0.20 的实时射线未命中与世界缓存不再重复累加天空，以保留场景独立的补光和反射强度。

mod node; // Core3d主颜色上的原位加法节点。
mod pipeline; // 复用MeshView环境贴图布局的全屏专用管线。
mod systems; // 只为合格Solari环境视图准备异步管线身份。

use bevy::{
    asset::embedded_asset, // WGSL随crate嵌入，避免运行时松散资源路径。
    core_pipeline::{
        core_3d::{main_opaque_pass_3d, main_transparent_pass_3d}, // 约束环境项位于Opaque之后、透明之前。
        schedule::{Core3d, Core3dSystems}, // 环境项属于每相机MainPass而不是全局Render帧任务。
    },
    ecs::schedule::IntoScheduleConfigs, // 声明精确系统前后关系。
    pbr::{MeshPipelineSystems, main_transmissive_pass_3d}, // 管线初始化等待MeshView布局，合成早于折射复制。
    prelude::*,                                            // App与调度配置。
    render::{
        GpuResourceAppExt,
        Render,
        RenderApp,
        RenderStartup,
        RenderSystems,                               // RenderApp资源与阶段注册。
        render_resource::SpecializedRenderPipelines, // 按视图布局和HDR格式异步专用化。
    },
};

use crate::raster_shadows::RasterShadowCompositeSet; // 环境光晚于有界方向光阴影，保持天空IBL不受shadow map遮挡。

/// 注册Solari环境IBL兼容层；无RenderApp的逻辑测试继续保持可运行。
pub(super) fn install_solari_environment_lighting(app: &mut App) {
    embedded_asset!(app, "solari_environment_lighting.wesl"); // 把完整环境采样Shader嵌入当前crate。
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return; // 无头逻辑App没有GPU子世界，不安装任何渲染资源。
    };
    render_app
        .init_gpu_resource::<SpecializedRenderPipelines<pipeline::SolariEnvironmentPipeline>>() // 专用化缓存只在RenderWorld持有。
        .add_systems(
            RenderStartup,
            pipeline::init_solari_environment_pipeline.after(MeshPipelineSystems), // 等待MeshView两组布局和设备能力就绪。
        )
        .add_systems(
            Render,
            systems::prepare_solari_environment_pipelines.in_set(RenderSystems::Prepare), // 稳定帧只核对小键并复用缓存ID。
        )
        .add_systems(
            Core3d,
            node::solari_environment_lighting
                .after(main_opaque_pass_3d) // GPU草叶Forward最终颜色已经覆盖主目标，Shader按bit7精确跳过。
                .after(RasterShadowCompositeSet) // 天空环境是全局IBL，不让标准方向光shadow map再次乘暗。
                .before(main_transmissive_pass_3d) // 水等透明Forward PBR稍后自行读取同一环境图，不参与本层。
                .before(main_transparent_pass_3d) // 透明粒子与界面不能被不透明GBuffer环境项二次覆盖。
                .in_set(Core3dSystems::MainPass), // RR位于更晚EarlyPostProcess并看到完整HDR环境结果。
        );
}

#[cfg(test)]
mod tests; // 源码、混合状态与调度边界回归。
