//! 子场景仅预热候选参与GPU证据聚合；主世界按实例实体与地表世代采用完整反馈。

use bevy::{prelude::*, core_pipeline::{core_3d::Opaque3d, deferred::Opaque3dDeferred},
    pbr::{RenderMaterialInstances, SpecializedMaterialPipelineCache, SpecializedPrepassMaterialPipelineCache,
        queue_material_meshes, queue_prepass_material_meshes},
    render::{ExtractSchedule, MainWorld, Render, RenderSystems,
        render_asset::{RenderAssets, prepare_assets}, render_phase::ViewBinnedRenderPhases,
        render_resource::PipelineCache, sync_world::MainEntity, texture::GpuImage,
        view::{ExtractedView, RenderVisibleEntities}}};
use super::{components::{TerrainSurfaceCarrier, TerrainSurfaceTextureProbe,
    TerrainSurfaceInstanceScope, TerrainSurfaceInstanceWarmup}, material::TerrainSurfaceMaterial,
    resources::{TerrainSurfaceInstanceFeedback, TerrainSurfaceRenderStats}};
use std::collections::BTreeMap;

pub(super) fn install(app: &mut bevy::app::SubApp) {
    app.init_resource::<TerrainSurfaceInstanceFeedback>()
        .add_systems(ExtractSchedule, publish)
        .add_systems(Render, observe.in_set(RenderSystems::QueueMeshes)
            .after(queue_material_meshes).after(queue_prepass_material_meshes).after(prepare_assets::<GpuImage>));
}

fn publish(mut world: ResMut<MainWorld>, mut frames: ResMut<TerrainSurfaceInstanceFeedback>) {
    if let Some(mut feedback) = world.get_resource_mut::<TerrainSurfaceInstanceFeedback>() {
        feedback.0 = std::mem::take(&mut frames.0); // 渲染层只发布小候选反馈，实例资源采用由场景宿主负责。
    }
}

#[allow(clippy::too_many_arguments)]
fn observe(carriers: Query<(&MainEntity, &TerrainSurfaceInstanceScope), (With<TerrainSurfaceCarrier>, With<TerrainSurfaceInstanceWarmup>)>,
    probes: Query<(&TerrainSurfaceTextureProbe, &TerrainSurfaceInstanceScope), With<TerrainSurfaceInstanceWarmup>>,
    images: Res<RenderAssets<GpuImage>>, materials: Res<RenderMaterialInstances>,
    pipelines: Res<SpecializedMaterialPipelineCache>, cache: Res<PipelineCache>,
    opaque: Res<ViewBinnedRenderPhases<Opaque3d>>, prepass: Res<SpecializedPrepassMaterialPipelineCache>,
    deferred: Res<ViewBinnedRenderPhases<Opaque3dDeferred>>, views: Query<(&ExtractedView, &RenderVisibleEntities)>,
    mut frames: ResMut<TerrainSurfaceInstanceFeedback>) {
    let mut groups = BTreeMap::<Entity, TerrainSurfaceRenderStats>::new();
    for (probe, scope) in &probes {
        let stats = groups.entry(scope.instance).or_insert_with(|| TerrainSurfaceRenderStats { generation: scope.generation, ..default() });
        stats.texture_expected += probe.images.len() as u32;
        stats.texture_ready += probe.images.iter().filter(|image| images.get(*image).is_some()).count() as u32;
        stats.texture_error |= u32::from(probe.failed);
    }
    for (entity, scope) in &carriers {
        let stats = groups.entry(scope.instance).or_insert_with(|| TerrainSurfaceRenderStats { generation: scope.generation, ..default() });
        stats.carrier_instances += 1;
        if !materials.instances.get(entity).is_some_and(|material| material.asset_id.type_id() == std::any::TypeId::of::<TerrainSurfaceMaterial>()) {
            stats.pipeline_waiting = 1; continue;
        }
        let evidence = super::render::terrain_surface_pipeline_evidence(*entity, &views,
            &pipelines, &prepass, &cache, &opaque, &deferred);
        stats.pipeline_error |= u32::from(evidence.error);
        stats.pipeline_waiting |= u32::from(evidence.waiting);
        // 每个实例承接实体都必须真实入队；另一候选的同管线不能补齐当前实体的证据。
        if evidence.queued { stats.phase_queued += 1; }
    }
    frames.0 = groups.into_iter().map(|(entity, mut stats)| {
        stats.texture_waiting = stats.texture_expected.saturating_sub(stats.texture_ready);
        stats.phase_queued = u32::from(stats.carrier_instances > 0 && stats.phase_queued == stats.carrier_instances);
        stats.pipeline_ready = u32::from(stats.carrier_instances > 0 && stats.pipeline_waiting == 0 && stats.pipeline_error == 0);
        (entity, stats)
    }).collect(); // 只保留当前完整候选帧；活动子图已撤销Warmup标记，不随引用数量增长。
}
