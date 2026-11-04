//! 独立WorldUI使用专用投影管线；同帧绘制失败时选择原材质管线，不能带着无抖动状态回退。
use super::{components::WorldUiLayer, resources::WorldUiPipelines, types::WorldUi3d};
use bevy::{
    ecs::{
        query::ROQueryItem,
        system::{SystemParamItem, lifetimeless::SRes},
    },
    render::{
        render_phase::{RenderCommand, RenderCommandResult, TrackedRenderPass},
        render_resource::*,
    },
}; // 使用公开绘制命令和缓存。 // 通道与当前帧失败事实。
pub(super) fn prepare_variant(
    source: CachedRenderPipelineId,
    cache: &PipelineCache,
    variants: &mut WorldUiPipelines,
) -> Option<CachedRenderPipelineId> {
    // 最多每个源管线排队一次，不等待编译。
    if cache.get_render_pipeline(source).is_none() {
        return None;
    } // 描述只有源管线处理完成后才可访问。
    let id = if let Some(id) = variants.0.get(&source) {
        *id
    } else {
        // 稳定帧只查哈希表。
        if variants.0.len() >= 256 {
            return None;
        } // 固定最坏上限，溢出走普通材质。
        let mut descriptor = cache.get_render_pipeline_descriptor(source).clone(); // 保留原mesh、材质与视图布局。
        descriptor.vertex.shader_defs.push("WORLD_UI_PASS".into()); // 只有此管线采用无抖动投影。
        let id = cache.queue_render_pipeline(descriptor); // 异步编译派生管线。
        variants.0.insert(source, id); // 保留唯一源到派生映射。
        id // 首帧通常尚未编译完成。
    }; // 派生身份确定结束。
    cache.get_render_pipeline(id).map(|_| id) // 所有变体就绪后才允许分流。
} // 派生管线预检结束。
pub(super) type DrawWorldUi = (
    SetWorldUiPipeline,
    bevy::pbr::SetMeshViewBindGroup<0>,
    bevy::pbr::SetMeshViewBindingArrayBindGroup<1>,
    bevy::pbr::SetMeshBindGroup<2>,
    bevy::pbr::SetMaterialBindGroup<3>,
    bevy::pbr::DrawMesh,
); // 与DrawMaterial相同绑定，仅改变管线选择。
pub(super) struct SetWorldUiPipeline; // 根据当前pass成功/回退状态绑定正确投影。
impl RenderCommand<WorldUi3d> for SetWorldUiPipeline {
    // 所有访问只读，兼容DrawMaterial世界借用。
    type Param = SRes<PipelineCache>; // 原管线与派生管线均已预检。
    type ViewQuery = &'static WorldUiLayer; // 读取当前帧失败标记。
    type ItemQuery = (); // 无逐实体额外查询。
    fn render<'w>(
        item: &WorldUi3d,
        layer: ROQueryItem<'w, '_, Self::ViewQuery>,
        _: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        cache: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        // 普通回退沿用同批次实例。
        let id = if layer.has_failed() {
            item.original_pipeline
        } else {
            item.item.pipeline
        }; // 回退不使用WORLD_UI_PASS。
        let Some(pipeline) = cache.into_inner().get_render_pipeline(id) else {
            return RenderCommandResult::Failure("世界UI绘制管线失效");
        }; // 明确失败，不跳过后假报完成。
        pass.set_render_pipeline(pipeline); // 布局与原材质完全相同。
        RenderCommandResult::Success // 后续标准绑定与DrawMesh继续执行。
    } // 渲染命令结束。
} // 管线选择命令结束。
