//! 点光、聚光与方向光阴影阶段的植被批次排队。

use bevy::camera::visibility::RenderLayers;
use bevy::ecs::system::SystemParam;
use bevy::pbr::{
    LightEntity, PreparedMaterial, RenderMaterialInstances, RenderMeshInstanceFlags,
    RenderMeshInstances, Shadow, ShadowBatchSetKey, ShadowBinKey, ShadowsDepthOnlyDrawFunction,
    SpecializedShadowMaterialPipelineCache,
};
use bevy::prelude::*;
use bevy::render::{
    erased_render_asset::ErasedRenderAssets,
    mesh::allocator::MeshAllocator,
    render_phase::{BinnedRenderPhaseType, DrawFunctions, ViewBinnedRenderPhases},
    render_resource::PipelineCache,
    view::{
        ExtractedView, RenderShadowMapVisibleEntities, RenderVisibleEntities, RetainedViewEntity,
    },
};

use super::super::{
    commands::{DrawVegetationMeshBatchPrepass, DrawVegetationMeshBatchPrepassDepthOnly},
    pipeline::{VegetationMeshBatchPass, VegetationMeshBatchPipelines},
    prepare::VegetationMeshBatchPrepared,
};
use crate::mesh_backend::batch::components::VegetationMeshBatchInstances;

#[derive(SystemParam)]
pub(crate) struct VegetationMeshBatchShadowQueue<'w, 's> {
    draws: Res<'w, DrawFunctions<Shadow>>,
    pipeline_cache: Res<'w, PipelineCache>,
    batch_pipelines: ResMut<'w, VegetationMeshBatchPipelines>,
    specialized: Res<'w, SpecializedShadowMaterialPipelineCache>,
    materials: Res<'w, ErasedRenderAssets<PreparedMaterial>>,
    material_instances: Res<'w, RenderMaterialInstances>,
    mesh_instances: Res<'w, RenderMeshInstances>,
    mesh_allocator: Res<'w, MeshAllocator>,
    phases: ResMut<'w, ViewBinnedRenderPhases<Shadow>>,
    light_views: Query<
        'w,
        's,
        (
            &'static LightEntity,
            &'static ExtractedView,
            &'static RenderLayers,
        ),
    >,
    shadow_visible: Query<'w, 's, &'static RenderShadowMapVisibleEntities>,
    batches: Query<
        'w,
        's,
        Option<&'static VegetationMeshBatchPrepared>,
        With<VegetationMeshBatchInstances>,
    >,
}

fn visible_entities_for_light<'a>(
    query: &'a Query<'_, '_, &RenderShadowMapVisibleEntities>,
    light: &LightEntity,
    view: &ExtractedView,
) -> Option<&'a RenderVisibleEntities> {
    let (light_entity, retained_view) = match light {
        LightEntity::Directional { light_entity, .. } => (*light_entity, view.retained_view_entity),
        LightEntity::Point {
            light_entity,
            face_index,
        } => (
            *light_entity,
            RetainedViewEntity {
                main_entity: view.retained_view_entity.main_entity,
                auxiliary_entity: None,
                subview_index: *face_index as u32,
            },
        ),
        LightEntity::Spot { light_entity } => (
            *light_entity,
            RetainedViewEntity {
                main_entity: view.retained_view_entity.main_entity,
                auxiliary_entity: None,
                subview_index: 0,
            },
        ),
    };
    query
        .get(light_entity)
        .ok()
        .and_then(|visible| visible.subviews.get(&retained_view))
        .map(|(_, entities)| entities)
}

/// 先让官方阴影系统建立材质专用化，再把每个批次承接节点换成一次实例化阴影绘制。
pub(crate) fn queue_vegetation_mesh_batch_shadows(
    mut params: VegetationMeshBatchShadowQueue<'_, '_>,
) {
    let with_material = params.draws.read().id::<DrawVegetationMeshBatchPrepass>();
    let depth_only = params
        .draws
        .read()
        .id::<DrawVegetationMeshBatchPrepassDepthOnly>();

    for (light, view, light_layers) in &params.light_views {
        let Some(visible_entities) =
            visible_entities_for_light(&params.shadow_visible, light, view)
        else {
            continue;
        };
        let Some(visible_meshes) = visible_entities.get::<Mesh3d>() else {
            continue;
        };
        let Some(view_pipelines) = params.specialized.get(&view.retained_view_entity) else {
            continue;
        };
        let Some(phase) = params.phases.get_mut(&view.retained_view_entity) else {
            continue;
        };

        for (render_entity, main_entity) in visible_meshes.iter_visible() {
            let Ok(prepared) = params.batches.get(*render_entity) else {
                continue;
            };
            phase.remove(*main_entity);
            let Some(prepared) = prepared else {
                continue; // 当前LOD为空时同样禁止官方单实例阴影残留。
            };

            let Some(&(base_pipeline, base_draw, _)) = view_pipelines.get(main_entity) else {
                continue;
            };
            let Some(mesh_instance) = params.mesh_instances.render_mesh_queue_data(*main_entity)
            else {
                continue;
            };
            if !mesh_instance
                .flags()
                .contains(RenderMeshInstanceFlags::SHADOW_CASTER)
                || !light_layers
                    .intersects(mesh_instance.render_layers.as_ref().unwrap_or_default())
            {
                continue;
            }
            let Some(material_instance) = params.material_instances.instances.get(main_entity)
            else {
                continue;
            };
            let Some(material) = params.materials.get(material_instance.asset_id) else {
                continue;
            };
            let mesh_asset_id = mesh_instance.mesh_asset_id();
            let Some(slabs) = params.mesh_allocator.mesh_slabs(&mesh_asset_id) else {
                continue;
            };
            let Some(pipeline) = params.batch_pipelines.get_or_queue(
                &params.pipeline_cache,
                base_pipeline,
                VegetationMeshBatchPass::Shadow,
                prepared.vertex_mode,
            ) else {
                continue;
            };
            let uses_depth_only = Some(base_draw)
                == material
                    .properties
                    .get_draw_function(ShadowsDepthOnlyDrawFunction);
            phase.add(
                ShadowBatchSetKey {
                    pipeline,
                    draw_function: if uses_depth_only {
                        depth_only
                    } else {
                        with_material
                    },
                    material_bind_group_index: (!uses_depth_only)
                        .then_some(material.binding.group.0),
                    slabs,
                },
                ShadowBinKey {
                    asset_id: mesh_asset_id.into(),
                },
                (*render_entity, *main_entity),
                mesh_instance.current_uniform_index,
                BinnedRenderPhaseType::UnbatchableMesh,
            );
        }
    }
}
