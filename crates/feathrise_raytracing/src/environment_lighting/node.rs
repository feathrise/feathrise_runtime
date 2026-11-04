//! Solari主颜色上的标准环境IBL原位加法节点。

use bevy::{
    camera::{MainPassResolutionOverride, Viewport}, // 与DLSS内部主通道使用同一物理viewport。
    core_pipeline::prepass::{DeferredPrepass, DepthPrepass}, // ViewQuery资格与附件存在证明。
    pbr::MeshViewBindGroup, // 复用Bevy已准备的环境Cube、LightProbes Uniform和GBuffer绑定。
    render::{
        camera::ExtractedCamera,                                // 恢复当前视口裁剪。
        diagnostic::RecordDiagnostics,                          // 发布独立GPU pass名称。
        render_resource::{PipelineCache, RenderPassDescriptor}, // 查询已编译管线并建立HDR加法pass。
        renderer::{RenderContext, ViewQuery},                   // 每相机Core3d执行上下文。
        view::ViewTarget,                                       // 原位写当前HDR颜色附件。
    },
};
use bevy_solari::prelude::SolariLighting; // 防御性锁定活动Solari视图。

use super::pipeline::SolariEnvironmentPipelineId; // 当前视图已准备的异步管线身份。

/// 在当前HDR附件上追加标准环境IBL，绝不翻转ViewTarget或读写任何时间历史。
#[allow(clippy::type_complexity)] // ViewQuery必须原子取得视图附件、两组绑定、管线身份和Solari资格。
pub(super) fn solari_environment_lighting(
    view: ViewQuery<(
        &ExtractedCamera,                    // 当前相机viewport。
        &ViewTarget,                         // Solari和Opaque已经写入的HDR目标。
        &MeshViewBindGroup,                  // group0主绑定与group1环境绑定数组。
        &SolariEnvironmentPipelineId,        // 当前布局对应的专用管线。
        Option<&MainPassResolutionOverride>, // DLSS/RR内部尺寸覆盖。
        &DepthPrepass,                       // 只作为资格证明。
        &DeferredPrepass,                    // 只作为资格证明。
        &SolariLighting,                     // 只作为资格证明。
    )>,
    pipeline_cache: bevy::prelude::Res<PipelineCache>, // 跳过尚未完成异步编译的管线。
    mut ctx: RenderContext,                            // 记录单个全屏HDR加法pass。
) {
    let (camera, target, mesh_view_bind_group, pipeline_id, resolution_override, _, _, _) =
        view.into_inner(); // 单次解构保持所有资格属于同一视图。
    let Some(pipeline) = pipeline_cache.get_render_pipeline(pipeline_id.0) else {
        return; // 异步编译完成前保留原Solari画面，不阻塞Render线程。
    };
    let diagnostics = ctx.diagnostic_recorder(); // 可选GPU诊断资源。
    let diagnostics = diagnostics.as_deref(); // 统一传给pass span。
    let color_attachments = [Some(target.get_color_attachment())]; // 以Load方式原位追加RGB，不翻转ping-pong。
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("solari_environment_lighting"), // wgpu捕获中的稳定标签。
        color_attachments: &color_attachments,      // 当前HDR主目标。
        depth_stencil_attachment: None, // 深度只通过MeshView纹理采样，不参与固定管线测试。
        timestamp_writes: None,         // 统一诊断记录器负责时间戳。
        occlusion_query_set: None,      // 全屏pass无需遮挡查询。
        multiview_mask: None,           // 项目当前每相机单视图执行。
    });
    let pass_span = diagnostics.pass_span(&mut pass, "solari_lighting/environment_map"); // 与Solari诊断命名空间聚合。
    if let Some(viewport) =
        Viewport::from_viewport_and_override(camera.viewport.as_ref(), resolution_override)
    {
        pass.set_camera_viewport(&viewport); // 内部像素与GBuffer绝对坐标保持一致。
    }
    pass.set_render_pipeline(pipeline); // 使用当前环境布局与HDR格式变体。
    pass.set_bind_group(
        0,
        &mesh_view_bind_group.main,
        &mesh_view_bind_group.main_offsets,
    ); // group0提供View、LightProbes、cluster、深度和Deferred GBuffer。
    pass.set_bind_group(1, &mesh_view_bind_group.binding_array, &[]); // group1提供漫反射/镜面Cube与采样器。
    pass.draw(0..3, 0..1); // 单个全屏三角覆盖当前viewport。
    pass_span.end(&mut pass); // 提交独立环境兼容GPU计时段。
}
