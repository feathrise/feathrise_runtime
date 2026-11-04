//! 在原生后处理和 UI 前把低分辨率主场景放大到完整 ViewTarget。

use bevy::{
    prelude::{Local, Res, UVec2},
    render::{
        extract_component::{ComponentUniforms, DynamicUniformIndex},
        render_resource::{
            BindGroup, BindGroupEntries, BufferId, Operations, PipelineCache,
            RenderPassColorAttachment, RenderPassDescriptor, TextureViewId,
        },
        renderer::{RenderContext, ViewQuery},
        view::ViewTarget,
    },
};

use super::{pipeline::RenderScalePipeline, systems::RenderScalePipelineId};
use crate::components::RenderScaleView;

/// 源纹理或逐视图 uniform 缓冲变化时才重建绑定组。
#[derive(Default)]
pub(super) struct RenderScaleBindGroupCache {
    cached: Vec<(TextureViewId, BufferId, BindGroup)>, // 同时容纳少量窗口视图及 ViewTarget 的双缓冲源。
}

const MAX_CACHED_BIND_GROUPS: usize = 8; // 有界缓存避免反复缩放窗口时无限保留旧纹理绑定。

/// 空间放大以本帧真实颜色附件为准，避免窗口变化时沿用主世界滞后一帧的输出尺寸。
fn full_output_extent(texture_size: UVec2) -> Option<UVec2> {
    (texture_size.x > 0 && texture_size.y > 0).then_some(texture_size) // 最小化窗口或暂时无目标尺寸时不翻转 ViewTarget。
}

/// 执行四次 textureLoad 的确定性双线性空间放大，并翻转 ViewTarget 供后续原生阶段消费。
pub(super) fn spatial_upscale(
    view: ViewQuery<(
        &ViewTarget,
        &RenderScaleView,
        &DynamicUniformIndex<RenderScaleView>,
        &RenderScalePipelineId,
    )>,
    pipeline: Option<Res<RenderScalePipeline>>,
    pipeline_cache: Res<PipelineCache>,
    settings_uniforms: Res<ComponentUniforms<RenderScaleView>>,
    mut cache: Local<RenderScaleBindGroupCache>,
    mut context: RenderContext,
) {
    let Some(pipeline) = pipeline else {
        return;
    };
    let (view_target, _scale, settings_index, pipeline_id) = view.into_inner();
    let output_texture = view_target.main_texture();
    let Some(output_size) =
        full_output_extent(UVec2::new(output_texture.width(), output_texture.height()))
    else {
        return; // 无有效完整输出时保持当前主纹理，避免提交零尺寸 viewport/scissor。
    };
    let Some(render_pipeline) = pipeline_cache.get_render_pipeline(pipeline_id.0) else {
        return; // 准备阶段已做就绪门，设备丢失或缓存失效时仍不得翻转目标。
    };
    let uniforms = settings_uniforms.uniforms();
    let Some(uniform_buffer_id) = uniforms.buffer().map(|buffer| buffer.id()) else {
        return;
    };
    let Some(settings_binding) = uniforms.binding() else {
        return;
    };

    let post_process = view_target.post_process_write(); // 只有全部资源就绪后才翻转 ping-pong 目标。
    let source_id = post_process.source.id();
    let cached_index = cache.cached.iter().position(|(texture_id, buffer_id, _)| {
        *texture_id == source_id && *buffer_id == uniform_buffer_id
    });
    let bind_group_index = cached_index.unwrap_or_else(|| {
        if cache.cached.len() == MAX_CACHED_BIND_GROUPS {
            cache.cached.remove(0); // 只在窗口/视图资源真实变化后逐出最旧绑定。
        }
        let bind_group = context.render_device().create_bind_group(
            "render_scale_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipeline.layout),
            &BindGroupEntries::sequential((post_process.source, settings_binding.clone())),
        );
        cache
            .cached
            .push((source_id, uniform_buffer_id, bind_group));
        cache.cached.len() - 1
    });
    let bind_group = &cache.cached[bind_group_index].2;

    let mut pass = context
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("render_scale_spatial_upscale"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: post_process.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(), // 全屏三角形覆盖完整目标，无需保留低分辨率区域之外的旧内容。
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(render_pipeline);
    pass.set_bind_group(0, bind_group, &[settings_index.index()]);
    pass.set_viewport(
        0.0,
        0.0,
        output_size.x as f32,
        output_size.y as f32,
        0.0,
        1.0,
    ); // 明确脱离低分辨率主通道 viewport，防止只在窗口左上角绘制放大结果。
    pass.set_scissor_rect(0, 0, output_size.x, output_size.y); // 同步恢复完整裁剪矩形，覆盖目标的每个物理像素。
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spatial_upscale_pass_uses_full_output_extent() {
        let attachment_size = UVec2::new(1920, 1080); // ViewTarget 附件是最终输出尺寸，而非 67% 主通道尺寸。
        assert_eq!(full_output_extent(attachment_size), Some(attachment_size));
        assert_ne!(
            full_output_extent(attachment_size),
            Some(UVec2::new(1286, 724))
        );
    }

    #[test]
    fn spatial_upscale_skips_zero_sized_outputs() {
        assert_eq!(full_output_extent(UVec2::ZERO), None); // 窗口最小化期间不得创建非法零尺寸 viewport/scissor。
    }
}
