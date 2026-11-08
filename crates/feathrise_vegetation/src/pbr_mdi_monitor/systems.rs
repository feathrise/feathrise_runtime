//! Bevy 0.19 PBR 主画面与阴影 MDI 排队镜像。

use bevy::asset::AssetId;
use bevy::camera::visibility::RenderLayers;
use bevy::core_pipeline::{
    core_3d::{AlphaMask3d, Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey},
    prepass::{OpaqueNoLightmap3dBatchSetKey, OpaqueNoLightmap3dBinKey},
};
use bevy::ecs::system::SystemParam;
use bevy::material::{OpaqueRendererMethod, RenderPhaseType};
use bevy::pbr::{
    LightEntity, MainPassAlphaMaskDrawFunction, MainPassOpaqueDrawFunction, PreparedMaterial,
    RenderMaterialInstances, RenderMeshInstanceFlags, RenderMeshInstances, Shadow,
    ShadowBatchSetKey, ShadowBinKey, ShadowsDepthOnlyDrawFunction,
    SpecializedMaterialPipelineCache, SpecializedShadowMaterialPipelineCache,
};
use bevy::prelude::*;
use bevy::render::{
    batching::gpu_preprocessing::GpuPreprocessingSupport,
    erased_render_asset::ErasedRenderAssets,
    mesh::allocator::MeshAllocator,
    render_phase::{BinnedRenderPhaseType, ViewBinnedRenderPhases},
    sync_world::MainEntity,
    view::{
        ExtractedView, RenderShadowMapVisibleEntities, RenderVisibleEntities, RetainedViewEntity,
    },
};

use super::helpers::summarize_phase;
use super::types::{PbrMdiPhaseEntry, PbrMdiQueueMirror, PbrPhaseStats};

/// 重建MDI镜像所需的全部官方PBR只读资源。
#[derive(SystemParam)]
pub struct PbrMdiQueueMirrorParams<'w, 's> {
    render_materials: Option<Res<'w, ErasedRenderAssets<PreparedMaterial>>>,
    render_mesh_instances: Option<Res<'w, RenderMeshInstances>>,
    render_material_instances: Option<Res<'w, RenderMaterialInstances>>,
    mesh_allocator: Option<Res<'w, MeshAllocator>>,
    gpu_preprocessing_support: Option<Res<'w, GpuPreprocessingSupport>>,
    specialized_material_pipeline_cache: Option<Res<'w, SpecializedMaterialPipelineCache>>,
    specialized_shadow_material_pipeline_cache:
        Option<Res<'w, SpecializedShadowMaterialPipelineCache>>,
    views: Query<'w, 's, (&'static ExtractedView, &'static RenderVisibleEntities)>,
    view_light_entities: Query<
        'w,
        's,
        (
            &'static LightEntity,
            &'static ExtractedView,
            Option<&'static RenderLayers>,
        ),
    >,
    shadow_map_visible_entities: Query<'w, 's, &'static RenderShadowMapVisibleEntities>,
}

impl PbrMdiQueueMirror {
    /// 按当前完整官方排队输入重建调用方目标的三阶段MDI实体。
    pub fn rebuild(
        &mut self,
        params: &PbrMdiQueueMirrorParams<'_, '_>,
        opaque_phases: &ViewBinnedRenderPhases<Opaque3d>,
        alpha_mask_phases: &ViewBinnedRenderPhases<AlphaMask3d>,
        shadow_phases: &ViewBinnedRenderPhases<Shadow>,
        include: impl Fn(MainEntity, AssetId<Mesh>) -> bool,
    ) -> bool {
        self.clear(); // 每个Render阶段只描述当前完整可见集合，容量跨帧复用。
        let (
            Some(render_materials),
            Some(render_mesh_instances),
            Some(render_material_instances),
            Some(mesh_allocator),
            Some(gpu_preprocessing_support),
            Some(specialized_material_pipeline_cache),
            Some(specialized_shadow_material_pipeline_cache),
        ) = (
            &params.render_materials,
            &params.render_mesh_instances,
            &params.render_material_instances,
            &params.mesh_allocator,
            &params.gpu_preprocessing_support,
            &params.specialized_material_pipeline_cache,
            &params.specialized_shadow_material_pipeline_cache,
        )
        else {
            return false; // 极简RenderApp缺少任一官方PBR资源时不能伪造完整采样。
        };

        collect_main_view_mdi(
            self,
            params,
            opaque_phases,
            alpha_mask_phases,
            render_materials,
            render_mesh_instances,
            render_material_instances,
            mesh_allocator,
            gpu_preprocessing_support,
            specialized_material_pipeline_cache,
            &include,
        );
        collect_shadow_mdi(
            self,
            params,
            shadow_phases,
            render_materials,
            render_mesh_instances,
            render_material_instances,
            mesh_allocator,
            gpu_preprocessing_support,
            specialized_shadow_material_pipeline_cache,
            &include,
        );
        true // 两类官方排队输入都存在时，空镜像也代表真实零MDI目标。
    }

    /// 汇总Opaque公开分箱与当前镜像MDI条目。
    pub fn summarize_opaque(
        &self,
        phases: &ViewBinnedRenderPhases<Opaque3d>,
        matches_bin: impl Fn(&Opaque3dBinKey) -> bool,
        matches_entity: impl Fn(MainEntity) -> bool,
    ) -> PbrPhaseStats {
        summarize_phase(phases, &self.opaque, matches_bin, matches_entity)
    }

    /// 汇总AlphaMask公开分箱与当前镜像MDI条目。
    pub fn summarize_alpha_mask(
        &self,
        phases: &ViewBinnedRenderPhases<AlphaMask3d>,
        matches_bin: impl Fn(&OpaqueNoLightmap3dBinKey) -> bool,
        matches_entity: impl Fn(MainEntity) -> bool,
    ) -> PbrPhaseStats {
        summarize_phase(phases, &self.alpha_mask, matches_bin, matches_entity)
    }

    /// 汇总Shadow公开分箱与当前镜像MDI条目。
    pub fn summarize_shadow(
        &self,
        phases: &ViewBinnedRenderPhases<Shadow>,
        matches_bin: impl Fn(&ShadowBinKey) -> bool,
        matches_entity: impl Fn(MainEntity) -> bool,
    ) -> PbrPhaseStats {
        summarize_phase(phases, &self.shadow, matches_bin, matches_entity)
    }
}

/// 复现官方主画面排队门，并仅记录实际存在于MDI批集的目标实体。
#[allow(clippy::too_many_arguments)]
fn collect_main_view_mdi(
    mirror: &mut PbrMdiQueueMirror,
    params: &PbrMdiQueueMirrorParams<'_, '_>,
    opaque_phases: &ViewBinnedRenderPhases<Opaque3d>,
    alpha_mask_phases: &ViewBinnedRenderPhases<AlphaMask3d>,
    render_materials: &ErasedRenderAssets<PreparedMaterial>,
    render_mesh_instances: &RenderMeshInstances,
    render_material_instances: &RenderMaterialInstances,
    mesh_allocator: &MeshAllocator,
    gpu_preprocessing_support: &GpuPreprocessingSupport,
    specialized_cache: &SpecializedMaterialPipelineCache,
    include: &impl Fn(MainEntity, AssetId<Mesh>) -> bool,
) {
    for (view, visible_entities) in &params.views {
        let retained_view = view.retained_view_entity;
        let (Some(opaque_phase), Some(alpha_mask_phase), Some(view_cache)) = (
            opaque_phases.get(&retained_view),
            alpha_mask_phases.get(&retained_view),
            specialized_cache.get(&retained_view),
        ) else {
            continue; // 当前视图没有完整PBR阶段或专用化缓存时官方同样不会排队。
        };
        let Some(visible_meshes) = visible_entities.get::<Mesh3d>() else {
            continue; // 当前视图没有可见3D Mesh。
        };
        for (_, main_entity) in visible_meshes.iter_visible() {
            let Some(pipeline) = view_cache.get(main_entity).copied() else {
                continue; // 专用化失败或等待中的实体不进入官方阶段。
            };
            let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(*main_entity)
            else {
                continue; // Mesh实例尚未完成提取或准备。
            };
            let mesh_asset_id = mesh_instance.mesh_asset_id();
            if !include(*main_entity, mesh_asset_id) {
                continue; // 调用方只承担当前F3目标的镜像成本。
            }
            if BinnedRenderPhaseType::mesh(mesh_instance.should_batch(), gpu_preprocessing_support)
                != BinnedRenderPhaseType::MultidrawableMesh
            {
                continue; // 普通可合批与独立实体仍由公开分箱直接统计。
            }
            let Some(material_instance) = render_material_instances.instances.get(main_entity)
            else {
                continue; // 没有材质实例时官方队列进入等待而不提交。
            };
            let Some(material) = render_materials.get(material_instance.asset_id) else {
                continue; // 材质尚未准备时不镜像伪条目。
            };
            let Some(slabs) = mesh_allocator.mesh_slabs(&mesh_asset_id) else {
                continue; // 未进入MeshAllocator的网格不能形成真实MDI批集。
            };
            match material.properties.render_phase_type {
                RenderPhaseType::Opaque
                    if material.properties.render_method != OpaqueRendererMethod::Deferred =>
                {
                    let Some(draw_function) = material
                        .properties
                        .get_draw_function(MainPassOpaqueDrawFunction)
                    else {
                        continue; // 当前材质没有主画面Opaque绘制函数。
                    };
                    let batch_set_key = Opaque3dBatchSetKey {
                        pipeline,
                        draw_function,
                        material_bind_group_index: Some(material.binding.group.0),
                        slabs,
                        lightmap_slab: mesh_instance.lightmap_slab_index().map(|index| *index),
                    };
                    if opaque_phase
                        .multidrawable_meshes
                        .contains_key(&batch_set_key)
                    {
                        mirror.opaque.push(PbrMdiPhaseEntry {
                            view: retained_view,
                            entity: *main_entity,
                            batch_set_key,
                            bin_key: Opaque3dBinKey {
                                asset_id: mesh_asset_id.into(),
                            },
                        }); // 外层官方批集存在才记录，兼容视图级禁用间接绘制。
                    }
                }
                RenderPhaseType::AlphaMask => {
                    let Some(draw_function) = material
                        .properties
                        .get_draw_function(MainPassAlphaMaskDrawFunction)
                    else {
                        continue; // 当前材质没有主画面AlphaMask绘制函数。
                    };
                    let batch_set_key = OpaqueNoLightmap3dBatchSetKey {
                        pipeline,
                        draw_function,
                        material_bind_group_index: Some(material.binding.group.0),
                        slabs,
                    };
                    if alpha_mask_phase
                        .multidrawable_meshes
                        .contains_key(&batch_set_key)
                    {
                        mirror.alpha_mask.push(PbrMdiPhaseEntry {
                            view: retained_view,
                            entity: *main_entity,
                            batch_set_key,
                            bin_key: OpaqueNoLightmap3dBinKey {
                                asset_id: mesh_asset_id.into(),
                            },
                        }); // 使用与官方AlphaMask完全一致的批集和分箱键。
                    }
                }
                _ => {} // 透明、透射及延迟Opaque不属于本监控的三个阶段。
            }
        }
    }
}

/// 复现官方阴影排队门，并仅记录实际存在于MDI批集的目标实体。
#[allow(clippy::too_many_arguments)]
fn collect_shadow_mdi(
    mirror: &mut PbrMdiQueueMirror,
    params: &PbrMdiQueueMirrorParams<'_, '_>,
    shadow_phases: &ViewBinnedRenderPhases<Shadow>,
    render_materials: &ErasedRenderAssets<PreparedMaterial>,
    render_mesh_instances: &RenderMeshInstances,
    render_material_instances: &RenderMaterialInstances,
    mesh_allocator: &MeshAllocator,
    gpu_preprocessing_support: &GpuPreprocessingSupport,
    specialized_cache: &SpecializedShadowMaterialPipelineCache,
    include: &impl Fn(MainEntity, AssetId<Mesh>) -> bool,
) {
    for (light_entity, view, view_layers) in &params.view_light_entities {
        let retained_view = view.retained_view_entity;
        let (Some(shadow_phase), Some(view_cache)) = (
            shadow_phases.get(&retained_view),
            specialized_cache.get(&retained_view),
        ) else {
            continue; // 当前灯光子视图未建立阶段或专用化缓存。
        };
        let visible_entities = match light_entity {
            LightEntity::Directional { light_entity, .. } => params
                .shadow_map_visible_entities
                .get(*light_entity)
                .ok()
                .and_then(|visible| visible.subviews.get(&retained_view)),
            LightEntity::Point {
                light_entity,
                face_index,
            } => {
                let lookup_view = RetainedViewEntity {
                    main_entity: retained_view.main_entity,
                    auxiliary_entity: None,
                    subview_index: *face_index as u32,
                };
                params
                    .shadow_map_visible_entities
                    .get(*light_entity)
                    .ok()
                    .and_then(|visible| visible.subviews.get(&lookup_view))
            }
            LightEntity::Spot { light_entity } => {
                let lookup_view = RetainedViewEntity {
                    main_entity: retained_view.main_entity,
                    auxiliary_entity: None,
                    subview_index: 0,
                };
                params
                    .shadow_map_visible_entities
                    .get(*light_entity)
                    .ok()
                    .and_then(|visible| visible.subviews.get(&lookup_view))
            }
        };
        let Some(visible_meshes) = visible_entities.and_then(|(_, entities)| entities.get::<Mesh3d>())
        else {
            continue; // 当前灯光子视图没有可见3D Mesh。
        };
        for (_, main_entity) in visible_meshes.iter_visible() {
            let Some(&(pipeline, draw_function, _)) = view_cache.get(main_entity) else {
                continue; // 阴影材质专用化尚未成功。
            };
            let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(*main_entity)
            else {
                continue; // Mesh实例尚未准备。
            };
            let mesh_asset_id = mesh_instance.mesh_asset_id();
            if !include(*main_entity, mesh_asset_id)
                || !mesh_instance
                    .flags()
                    .contains(RenderMeshInstanceFlags::SHADOW_CASTER)
            {
                continue; // 非目标或明确不投影节点不进入阴影镜像。
            }
            if !view_layers
                .unwrap_or_default()
                .intersects(mesh_instance.render_layers.as_ref().unwrap_or_default())
            {
                continue; // 灯光与Mesh渲染层不相交时官方同样跳过。
            }
            if BinnedRenderPhaseType::mesh(mesh_instance.should_batch(), gpu_preprocessing_support)
                != BinnedRenderPhaseType::MultidrawableMesh
            {
                continue; // 普通或独立阴影分箱继续走公开路径。
            }
            let Some(material_instance) = render_material_instances.instances.get(main_entity)
            else {
                continue; // 缺材质实例时不形成阴影提交。
            };
            let Some(material) = render_materials.get(material_instance.asset_id) else {
                continue; // 材质准备等待期间不镜像。
            };
            let material_bind_group_index = if Some(draw_function)
                == material
                    .properties
                    .get_draw_function(ShadowsDepthOnlyDrawFunction)
            {
                None // 深度专用阴影不绑定材质组。
            } else {
                Some(material.binding.group.0) // AlphaMask等阴影保持材质绑定身份。
            };
            let Some(slabs) = mesh_allocator.mesh_slabs(&mesh_asset_id) else {
                continue; // 未分配网格无法进入真实阴影批集。
            };
            let batch_set_key = ShadowBatchSetKey {
                pipeline,
                draw_function,
                material_bind_group_index,
                slabs,
            };
            if shadow_phase
                .multidrawable_meshes
                .contains_key(&batch_set_key)
            {
                mirror.shadow.push(PbrMdiPhaseEntry {
                    view: retained_view,
                    entity: *main_entity,
                    batch_set_key,
                    bin_key: ShadowBinKey {
                        asset_id: mesh_asset_id.into(),
                    },
                }); // 只记录官方外层阴影MDI批集已存在的实体。
            }
        }
    }
}
