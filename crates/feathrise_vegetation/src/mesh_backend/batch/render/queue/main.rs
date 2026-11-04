//! 主颜色阶段的植被批次排队。

use bevy::core_pipeline::{
    core_3d::{AlphaMask3d, Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey},
    prepass::{OpaqueNoLightmap3dBatchSetKey, OpaqueNoLightmap3dBinKey},
};
use bevy::ecs::system::SystemParam;
use bevy::material::{OpaqueRendererMethod, RenderPhaseType};
use bevy::pbr::{
    PreparedMaterial, RenderMaterialInstances, RenderMeshInstances,
    SpecializedMaterialPipelineCache,
};
use bevy::prelude::*;
use bevy::render::{
    erased_render_asset::ErasedRenderAssets,
    mesh::allocator::MeshAllocator,
    render_phase::{BinnedRenderPhaseType, DrawFunctions, ViewBinnedRenderPhases},
    render_resource::PipelineCache,
    view::{ExtractedView, RenderVisibleEntities},
};

use super::super::{
    commands::DrawVegetationMeshBatch,
    pipeline::{VegetationMeshBatchPass, VegetationMeshBatchPipelines},
    prepare::VegetationMeshBatchPrepared,
};
use crate::mesh_backend::batch::components::VegetationMeshBatchInstances;

#[derive(SystemParam)]
pub(crate) struct VegetationMeshBatchMainQueue<'w, 's> {
    opaque_draws: Res<'w, DrawFunctions<Opaque3d>>,
    alpha_mask_draws: Res<'w, DrawFunctions<AlphaMask3d>>,
    pipeline_cache: Res<'w, PipelineCache>,
    batch_pipelines: ResMut<'w, VegetationMeshBatchPipelines>,
    specialized: Res<'w, SpecializedMaterialPipelineCache>,
    materials: Res<'w, ErasedRenderAssets<PreparedMaterial>>,
    material_instances: Res<'w, RenderMaterialInstances>,
    mesh_instances: Res<'w, RenderMeshInstances>,
    mesh_allocator: Res<'w, MeshAllocator>,
    opaque_phases: ResMut<'w, ViewBinnedRenderPhases<Opaque3d>>,
    alpha_mask_phases: ResMut<'w, ViewBinnedRenderPhases<AlphaMask3d>>,
    views: Query<'w, 's, (&'static ExtractedView, &'static RenderVisibleEntities)>,
    batches: Query<
        'w,
        's,
        Option<&'static VegetationMeshBatchPrepared>,
        With<VegetationMeshBatchInstances>,
    >,
}

/// 每帧只重排可见批次承接节点。承接节点仍走官方材质专用化，随后在这里原位替换为实例顶点流。
pub(crate) fn queue_vegetation_mesh_batches(mut params: VegetationMeshBatchMainQueue<'_, '_>) {
    let opaque_draw = params.opaque_draws.read().id::<DrawVegetationMeshBatch>();
    let alpha_mask_draw = params
        .alpha_mask_draws
        .read()
        .id::<DrawVegetationMeshBatch>();

    for (view, visible_entities) in &params.views {
        let Some(visible_meshes) = visible_entities.get::<Mesh3d>() else {
            continue;
        };
        let Some(view_pipelines) = params.specialized.get(&view.retained_view_entity) else {
            continue;
        };
        let (Some(opaque_phase), Some(alpha_mask_phase)) = (
            params.opaque_phases.get_mut(&view.retained_view_entity),
            params.alpha_mask_phases.get_mut(&view.retained_view_entity),
        ) else {
            continue;
        };

        for (render_entity, main_entity) in visible_meshes.iter_visible() {
            let Ok(prepared) = params.batches.get(*render_entity) else {
                continue;
            };

            // 官方队列把承接节点当成位于批次原点的一棵普通网格；必须先删掉它，避免重影和错误剔除。
            opaque_phase.remove(*main_entity);
            alpha_mask_phase.remove(*main_entity);
            let Some(prepared) = prepared else {
                continue; // 当前LOD没有实例或仍在准备时也绝不能退回官方单实例绘制。
            };

            let Some(base_pipeline) = view_pipelines.get(main_entity).copied() else {
                continue;
            };
            let Some(mesh_instance) = params.mesh_instances.render_mesh_queue_data(*main_entity)
            else {
                continue;
            };
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
                VegetationMeshBatchPass::Main,
                prepared.vertex_mode,
            ) else {
                continue;
            };

            match material.properties.render_phase_type {
                RenderPhaseType::Opaque
                    if material.properties.render_method != OpaqueRendererMethod::Deferred =>
                {
                    opaque_phase.add(
                        Opaque3dBatchSetKey {
                            pipeline,
                            draw_function: opaque_draw,
                            material_bind_group_index: Some(material.binding.group.0),
                            slabs,
                            lightmap_slab: None,
                        },
                        Opaque3dBinKey {
                            asset_id: mesh_asset_id.into(),
                        },
                        (*render_entity, *main_entity),
                        mesh_instance.current_uniform_index,
                        BinnedRenderPhaseType::UnbatchableMesh,
                    );
                }
                RenderPhaseType::AlphaMask => {
                    alpha_mask_phase.add(
                        OpaqueNoLightmap3dBatchSetKey {
                            pipeline,
                            draw_function: alpha_mask_draw,
                            material_bind_group_index: Some(material.binding.group.0),
                            slabs,
                        },
                        OpaqueNoLightmap3dBinKey {
                            asset_id: mesh_asset_id.into(),
                        },
                        (*render_entity, *main_entity),
                        mesh_instance.current_uniform_index,
                        BinnedRenderPhaseType::UnbatchableMesh,
                    );
                }
                _ => {} // 当前正式植被只接受不透明或裁切材质；透明材质继续显式留在旧后端。
            }
        }
    }
}
