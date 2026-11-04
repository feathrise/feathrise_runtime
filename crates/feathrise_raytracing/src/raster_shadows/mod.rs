//! Solari视图中的P3光栅内容方向光阴影兼容层。
//!
//! 玩家、NPC、敌人与GPU叶片按P3边界不进入TLAS；本模块保留它们成熟的标准shadow-map投射路径，
//! 并在Solari及草叶主颜色完成后、透明与RR之前，以有下限的可见度乘法恢复动态接地。
//! 当前HDR颜色没有分离直接光与GI，因此这是明确受限的工程兼容近似，不是Solari DI的物理解。

mod node; // Core3d主通道中的原位RGB乘法合成。
mod pipeline; // 复用MeshView阴影atlas布局并按视图键专用化。
mod systems; // 为活动Solari视图异步准备已就绪管线组件。

use bevy::{
    asset::embedded_asset,
    core_pipeline::{
        core_3d::{main_opaque_pass_3d, main_transparent_pass_3d},
        schedule::{Core3d, Core3dSystems},
    },
    ecs::schedule::IntoScheduleConfigs,
    pbr::{MeshPipelineSystems, main_transmissive_pass_3d},
    prelude::*,
    render::{
        GpuResourceAppExt, Render, RenderApp, RenderStartup, RenderSystems,
        render_resource::SpecializedRenderPipelines,
    },
};

/// 标记Solari后方向光阴影合成的精确调度位置，供后续环境IBL显式排序。
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct RasterShadowCompositeSet;

/// 注册标准shadow map驱动的Solari光栅阴影兼容层；无RenderApp的逻辑测试保持可运行。
pub(super) fn install_raster_shadow_compatibility(app: &mut App) {
    embedded_asset!(app, "raster_shadow_composite.wgsl"); // WGSL随crate嵌入，不新增松散资产部署契约。
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_gpu_resource::<SpecializedRenderPipelines<pipeline::RasterShadowCompositePipeline>>()
        .add_systems(
            RenderStartup,
            pipeline::init_raster_shadow_composite_pipeline.after(MeshPipelineSystems),
        )
        .add_systems(
            Render,
            systems::prepare_raster_shadow_composite_pipelines.in_set(RenderSystems::Prepare),
        )
        .add_systems(
            Core3d,
            node::raster_shadow_composite
                .after(main_opaque_pass_3d) // 草叶Forward overlay已经覆盖最终pInColor，阴影必须晚于它。
                .before(main_transmissive_pass_3d) // 折射复制必须看到已经合成阴影的Opaque主颜色。
                .before(main_transparent_pass_3d) // 透明内容不应被不透明GBuffer法线误乘；RR仍在更晚EarlyPostProcess。
                .in_set(RasterShadowCompositeSet) // 天空IBL必须在本近似阴影完成后追加，避免环境光被shadow map误压暗。
                .in_set(Core3dSystems::MainPass),
        );
}

#[cfg(test)]
mod tests;
