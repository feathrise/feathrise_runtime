//! 活动Solari视图的阴影兼容管线准备。

use bevy::{
    core_pipeline::prepass::{DeferredPrepass, DepthPrepass},
    pbr::{MeshPipelineKey, ViewKeyCache},
    prelude::*,
    render::{
        render_resource::{PipelineCache, SpecializedRenderPipelines},
        view::ExtractedView,
    },
};
use bevy_solari::prelude::SolariLighting;

use super::pipeline::{
    RasterShadowCompositePipeline, RasterShadowCompositePipelineId,
    RasterShadowCompositePipelineKey,
};

/// 将Bevy公开阴影过滤档折叠成稳定的小键，避免把无关材质位带入全屏管线缓存。
fn shadow_filter_method(key: MeshPipelineKey) -> u8 {
    let method = key.intersection(MeshPipelineKey::SHADOW_FILTER_METHOD_RESERVED_BITS);
    if method == MeshPipelineKey::SHADOW_FILTER_METHOD_HARDWARE_2X2 {
        1
    } else if method == MeshPipelineKey::SHADOW_FILTER_METHOD_GAUSSIAN {
        2
    } else if method == MeshPipelineKey::SHADOW_FILTER_METHOD_TEMPORAL {
        3
    } else {
        0
    }
}

/// 为唯一活动Solari视图准备已缓存管线；稳定帧仅在ID变化时写组件。
#[allow(clippy::type_complexity)] // RenderWorld Query 同时锁定 Solari、深度与 Deferred 三项附件资格。
pub(super) fn prepare_raster_shadow_composite_pipelines(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    view_key_cache: Res<ViewKeyCache>,
    pipeline: Option<Res<RasterShadowCompositePipeline>>,
    mut pipelines: ResMut<SpecializedRenderPipelines<RasterShadowCompositePipeline>>,
    views: Query<
        (
            Entity,
            &ExtractedView,
            Option<&RasterShadowCompositePipelineId>,
        ),
        (
            With<SolariLighting>,
            With<DepthPrepass>,
            With<DeferredPrepass>,
        ),
    >,
) {
    let Some(pipeline) = pipeline.as_deref() else {
        return;
    };
    for (entity, extracted_view, current) in &views {
        let Some(view_key) = view_key_cache
            .get(&extracted_view.retained_view_entity)
            .copied()
        else {
            if current.is_some() {
                commands
                    .entity(entity)
                    .remove::<RasterShadowCompositePipelineId>();
            }
            continue;
        };
        let mesh_view_layout_key: bevy::pbr::MeshPipelineViewLayoutKey = view_key.into();
        if mesh_view_layout_key.contains(bevy::pbr::MeshPipelineViewLayoutKey::MULTISAMPLED) {
            // Solari正式租约必须单采样；防御性拒绝错误附件，避免异步旧ID触发wgpu致命校验。
            if current.is_some() {
                commands
                    .entity(entity)
                    .remove::<RasterShadowCompositePipelineId>();
            }
            continue;
        }
        let desired = pipelines.specialize(
            &pipeline_cache,
            pipeline,
            RasterShadowCompositePipelineKey {
                mesh_view_layout_key,
                target_format: extracted_view.target_format,
                shadow_filter_method: shadow_filter_method(view_key),
            },
        );
        if current.map(|id| id.0) != Some(desired) {
            commands
                .entity(entity)
                .insert(RasterShadowCompositePipelineId(desired));
        }
    }
}
