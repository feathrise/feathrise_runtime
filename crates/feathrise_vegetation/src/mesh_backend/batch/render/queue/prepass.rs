//! 深度、运动矢量和 Deferred GBuffer 阶段的植被批次排队。

use bevy::core_pipeline::{
    deferred::{AlphaMask3dDeferred, Opaque3dDeferred},
    prepass::{
        AlphaMask3dPrepass, Opaque3dPrepass, OpaqueNoLightmap3dBatchSetKey,
        OpaqueNoLightmap3dBinKey,
    },
};
use bevy::ecs::system::SystemParam;
use bevy::material::{OpaqueRendererMethod, RenderPhaseType};
use bevy::pbr::{
    PreparedMaterial, PrepassOpaqueDepthOnlyDrawFunction, RenderMaterialInstances,
    RenderMeshInstances, SpecializedPrepassMaterialPipelineCache,
};
use bevy::prelude::*;
use bevy::render::{
    erased_render_asset::ErasedRenderAssets,
    mesh::allocator::MeshAllocator,
    render_phase::{BinnedRenderPhaseType, DrawFunctionId, DrawFunctions, ViewBinnedRenderPhases},
    render_resource::PipelineCache,
    view::{ExtractedView, RenderVisibleEntities},
};

use super::super::{
    commands::{DrawVegetationMeshBatchPrepass, DrawVegetationMeshBatchPrepassDepthOnly},
    pipeline::{VegetationMeshBatchPass, VegetationMeshBatchPipelines},
    prepare::VegetationMeshBatchPrepared,
};
use crate::mesh_backend::batch::components::VegetationMeshBatchInstances;

#[derive(SystemParam)]
pub(crate) struct VegetationMeshBatchPrepassQueue<'w, 's> {
    opaque_prepass_draws: Res<'w, DrawFunctions<Opaque3dPrepass>>,
    alpha_prepass_draws: Res<'w, DrawFunctions<AlphaMask3dPrepass>>,
    opaque_deferred_draws: Res<'w, DrawFunctions<Opaque3dDeferred>>,
    alpha_deferred_draws: Res<'w, DrawFunctions<AlphaMask3dDeferred>>,
    pipeline_cache: Res<'w, PipelineCache>,
    batch_pipelines: ResMut<'w, VegetationMeshBatchPipelines>,
    specialized: Res<'w, SpecializedPrepassMaterialPipelineCache>,
    materials: Res<'w, ErasedRenderAssets<PreparedMaterial>>,
    material_instances: Res<'w, RenderMaterialInstances>,
    mesh_instances: Res<'w, RenderMeshInstances>,
    mesh_allocator: Res<'w, MeshAllocator>,
    opaque_prepass: ResMut<'w, ViewBinnedRenderPhases<Opaque3dPrepass>>,
    alpha_prepass: ResMut<'w, ViewBinnedRenderPhases<AlphaMask3dPrepass>>,
    opaque_deferred: ResMut<'w, ViewBinnedRenderPhases<Opaque3dDeferred>>,
    alpha_deferred: ResMut<'w, ViewBinnedRenderPhases<AlphaMask3dDeferred>>,
    views: Query<'w, 's, (&'static ExtractedView, &'static RenderVisibleEntities)>,
    batches: Query<
        'w,
        's,
        Option<&'static VegetationMeshBatchPrepared>,
        With<VegetationMeshBatchInstances>,
    >,
}

#[derive(Clone, Copy)]
struct PrepassDrawFunctions {
    opaque_with_material: DrawFunctionId,
    opaque_depth_only: DrawFunctionId,
    alpha_with_material: DrawFunctionId,
    opaque_deferred: DrawFunctionId,
    alpha_deferred: DrawFunctionId,
}

impl PrepassDrawFunctions {
    fn from_params(params: &VegetationMeshBatchPrepassQueue<'_, '_>) -> Self {
        Self {
            opaque_with_material: params
                .opaque_prepass_draws
                .read()
                .id::<DrawVegetationMeshBatchPrepass>(),
            opaque_depth_only: params
                .opaque_prepass_draws
                .read()
                .id::<DrawVegetationMeshBatchPrepassDepthOnly>(),
            alpha_with_material: params
                .alpha_prepass_draws
                .read()
                .id::<DrawVegetationMeshBatchPrepass>(),
            opaque_deferred: params
                .opaque_deferred_draws
                .read()
                .id::<DrawVegetationMeshBatchPrepass>(),
            alpha_deferred: params
                .alpha_deferred_draws
                .read()
                .id::<DrawVegetationMeshBatchPrepass>(),
        }
    }
}

/// 用与官方材质完全相同的基础专用化描述符替换顶点阶段，保留材质裁切和 GBuffer 片元输出。
pub(crate) fn queue_vegetation_mesh_batch_prepasses(
    mut params: VegetationMeshBatchPrepassQueue<'_, '_>,
) {
    let draws = PrepassDrawFunctions::from_params(&params);

    for (view, visible_entities) in &params.views {
        let Some(visible_meshes) = visible_entities.get::<Mesh3d>() else {
            continue;
        };
        let Some(view_pipelines) = params.specialized.get(&view.retained_view_entity) else {
            continue;
        };
        let mut opaque_prepass = params.opaque_prepass.get_mut(&view.retained_view_entity);
        let mut alpha_prepass = params.alpha_prepass.get_mut(&view.retained_view_entity);
        let mut opaque_deferred = params.opaque_deferred.get_mut(&view.retained_view_entity);
        let mut alpha_deferred = params.alpha_deferred.get_mut(&view.retained_view_entity);

        for (render_entity, main_entity) in visible_meshes.iter_visible() {
            let Ok(prepared) = params.batches.get(*render_entity) else {
                continue;
            };
            if let Some(phase) = opaque_prepass.as_mut() {
                phase.remove(*main_entity);
            }
            if let Some(phase) = alpha_prepass.as_mut() {
                phase.remove(*main_entity);
            }
            if let Some(phase) = opaque_deferred.as_mut() {
                phase.remove(*main_entity);
            }
            if let Some(phase) = alpha_deferred.as_mut() {
                phase.remove(*main_entity);
            }
            let Some(prepared) = prepared else {
                continue; // 空LOD仍需清掉官方深度/GBuffer项，不能在批次原点留下单实例。
            };

            let Some(&(_, base_pipeline, base_draw)) = view_pipelines.get(main_entity) else {
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
                VegetationMeshBatchPass::Prepass,
                prepared.vertex_mode,
            ) else {
                continue;
            };
            let deferred = material.properties.render_method == OpaqueRendererMethod::Deferred;

            match (material.properties.render_phase_type, deferred) {
                (RenderPhaseType::Opaque, true) => {
                    let Some(phase) = opaque_deferred.as_mut() else {
                        continue;
                    };
                    phase.add(
                        OpaqueNoLightmap3dBatchSetKey {
                            pipeline,
                            draw_function: draws.opaque_deferred,
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
                (RenderPhaseType::Opaque, false) => {
                    let Some(phase) = opaque_prepass.as_mut() else {
                        continue;
                    };
                    let depth_only = Some(base_draw)
                        == material
                            .properties
                            .get_draw_function(PrepassOpaqueDepthOnlyDrawFunction);
                    phase.add(
                        OpaqueNoLightmap3dBatchSetKey {
                            pipeline,
                            draw_function: if depth_only {
                                draws.opaque_depth_only
                            } else {
                                draws.opaque_with_material
                            },
                            material_bind_group_index: (!depth_only)
                                .then_some(material.binding.group.0),
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
                (RenderPhaseType::AlphaMask, true) => {
                    let Some(phase) = alpha_deferred.as_mut() else {
                        continue;
                    };
                    phase.add(
                        OpaqueNoLightmap3dBatchSetKey {
                            pipeline,
                            draw_function: draws.alpha_deferred,
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
                (RenderPhaseType::AlphaMask, false) => {
                    let Some(phase) = alpha_prepass.as_mut() else {
                        continue;
                    };
                    phase.add(
                        OpaqueNoLightmap3dBatchSetKey {
                            pipeline,
                            draw_function: draws.alpha_with_material,
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
                _ => {}
            }
        }
    }
}
