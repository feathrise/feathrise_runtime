//! Solari主颜色上的标准方向光阴影原位合成节点。

use bevy::{
    camera::{MainPassResolutionOverride, Viewport},
    core_pipeline::prepass::{DeferredPrepass, DepthPrepass},
    pbr::MeshViewBindGroup,
    render::{
        camera::ExtractedCamera,
        diagnostic::RecordDiagnostics,
        render_resource::{PipelineCache, RenderPassDescriptor},
        renderer::{RenderContext, ViewQuery},
        view::ViewTarget,
    },
};
use bevy_solari::prelude::SolariLighting;

use super::pipeline::RasterShadowCompositePipelineId;

/// 在当前HDR附件上原位乘以有界方向光阴影，绝不翻转ViewTarget或触碰任何历史资源。
#[allow(clippy::type_complexity)] // ViewNode 必须原子取得视图附件、绑定组、管线身份与 Solari/预通道资格。
pub(super) fn raster_shadow_composite(
    view: ViewQuery<(
        &ExtractedCamera,
        &ViewTarget,
        &MeshViewBindGroup,
        &RasterShadowCompositePipelineId,
        Option<&MainPassResolutionOverride>,
        &DepthPrepass,
        &DeferredPrepass,
        &SolariLighting,
    )>,
    pipeline_cache: bevy::prelude::Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (camera, target, mesh_view_bind_group, pipeline_id, resolution_override, _, _, _) =
        view.into_inner();
    let Some(pipeline) = pipeline_cache.get_render_pipeline(pipeline_id.0) else {
        return;
    };
    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    // 这里不采主颜色，只借硬件blend原位乘色；沿用ViewTarget当前attachment，不能翻转历史ping-pong。
    let color_attachments = [Some(target.get_color_attachment())];
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("solari_raster_shadow_composite"),
        color_attachments: &color_attachments,
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    let pass_span = diagnostics.pass_span(&mut pass, "solari_lighting/raster_shadow_composite");
    if let Some(viewport) =
        Viewport::from_viewport_and_override(camera.viewport.as_ref(), resolution_override)
    {
        pass.set_camera_viewport(&viewport); // DLSS/DLAA必须与主Opaque使用同一物理内部尺寸。
    }
    pass.set_render_pipeline(pipeline);
    pass.set_bind_group(
        0,
        &mesh_view_bind_group.main,
        &mesh_view_bind_group.main_offsets,
    );
    pass.draw(0..3, 0..1);
    pass_span.end(&mut pass);
}
