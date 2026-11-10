//! 动态场景地表定义、五纹理与局部参数表预热、失败白模与热更新恢复系统。

use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use bevy::asset::{AssetEvent, LoadState};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::gltf::{GltfMaterialName, GltfMeshName};
use bevy::prelude::*;
use bevy::render::storage::ShaderBuffer;
use feathrise_environment::terrain_surface::{
    TerrainSurfaceCarrier, TerrainSurfaceFailure, TerrainSurfaceFallbackImages,
    TerrainSurfaceMaterial, TerrainSurfaceMaterialOverride, TerrainSurfacePhase,
    TerrainSurfaceRenderStats, TerrainSurfaceTextureProbe, TerrainSurfaceWarmup, terrain_surface_warmup_can_finalize,
};
use feathrise_scene_content::{
    ActiveDynamicScene, SceneManifestAsset, SceneSurfaceAsset, SurfacePresetRegistry,
};
use feathrise_scene_core::state::ScenePhase;

use super::author::DynamicSurfaceAuthorPreview; // 作者输入与正式文件候选共享源接管规则。
use super::components::{DynamicSurfaceSourceWarmup, DynamicSurfaceWarmup};
use super::helpers::{
    build_surface_material, load_surface_textures, source_identity, surface_texture_failure,
    surface_textures_loaded, validate_surface_definition,
};
use super::resources::DynamicSurfaceState;
use super::transactions::{
    cancel_surface_warmup, fail_current_revision, reconcile_current_surface_source,
    restore_white_model, surface_source_needs_reconcile,
};
// 唯一正式primitive是稳定源身份，不再依赖可能错过的Changed事件。
use crate::DynamicSceneTerrainPrimitive;
use crate::scenes::dynamic_scene::clipmap::{
    ClipmapHeightMaterial, SparseClipmapSurfaceLease, SparseClipmapSurfaceMaterial,
}; // 稀疏新图在内层地表完成GPU预热前后都保留高度位移材质。
use crate::scenes::dynamic_scene::resources::{
    DynamicSceneRuntimePhase, DynamicSceneRuntimeState, DynamicSceneRuntimeStatus,
};

pub(super) type SourceData = (
    Entity,
    &'static GltfMeshName,
    &'static GltfMaterialName,
    &'static Mesh3d,
    &'static Transform,
    &'static Visibility,
    Option<&'static ChildOf>,
    Option<&'static MeshMaterial3d<StandardMaterial>>,
    Option<&'static MeshMaterial3d<ClipmapHeightMaterial>>,
    Option<&'static MeshMaterial3d<TerrainSurfaceMaterial>>,
    Option<&'static MeshMaterial3d<SparseClipmapSurfaceMaterial>>,
    Option<&'static TerrainSurfaceCarrier>, // 稳定Active帧只读核对Carrier，不重复插入组件。
    Option<&'static TerrainSurfaceMaterialOverride>, // 仅接受与正式源和已安装句柄同时匹配的运行时替代。
    Option<&'static SparseClipmapSurfaceLease>,      // 新图高度外层必须精确包装当前作者地表。
);

/// 目录、地表JSON和当前纹理事件只推进修订，不在稳定帧重新扫描资产或世界。
pub(crate) fn observe_dynamic_surface_assets(
    registry: Option<crate::scenes::dynamic_scene::sources::SceneRuntimeCatalog>,
    surface_presets: Option<Res<SurfacePresetRegistry>>,
    mut surface_events: feathrise_scene_core::residency::ResidentAssetEventReader<SceneSurfaceAsset>,
    mut image_events: feathrise_scene_core::residency::ResidentAssetEventReader<Image>,
    mut state: InstanceResMut<DynamicSurfaceState>,
    mut stats: InstanceResMut<TerrainSurfaceRenderStats>,
    mut author: crate::scenes::dynamic_scene::author_sources::TerrainAuthorResMut<DynamicSurfaceAuthorPreview>, // 保存门只能由本次真实文件事件解除。
) {
    if registry
        .as_deref()
        .is_some_and(|registry| registry.revision != state.catalog_revision)
    {
        state.definition_dirty = true;
    }
    if surface_presets
        .as_deref()
        .is_some_and(|presets| presets.revision != state.preset_catalog_revision)
    {
        // 参数表或全局纹理数组的任一修订都必须走完整候选预热，活动材质保持到原子采用。
        state.definition_dirty = true;
    }
    if let Some(definition_id) = state.definition_handle.as_ref().map(Handle::id) {
        for event in surface_events.read() {
            let id = match event {
                AssetEvent::Added { id }
                | AssetEvent::Modified { id }
                | AssetEvent::Removed { id }
                | AssetEvent::LoadedWithDependencies { id }
                | AssetEvent::Unused { id } => *id,
            };
            if id == definition_id && !matches!(event, AssetEvent::Unused { .. }) {
                state.definition_dirty = true;
                if author.saving
                    && author.scene_id == state.scene_id
                    && matches!(
                        event,
                        AssetEvent::Modified { .. } | AssetEvent::LoadedWithDependencies { .. }
                    )
                {
                    author.pending_definition = false; // 发布后的定义事件才允许文件交接。
                }
            }
        }
    } else {
        surface_events.read().for_each(drop);
    }
    let phase = stats.phase;
    let mut texture_changed = false;
    if let Some(textures) = state.textures.as_ref() {
        for event in image_events.read() {
            let id = match event {
                AssetEvent::Added { id }
                | AssetEvent::Modified { id }
                | AssetEvent::Removed { id }
                | AssetEvent::LoadedWithDependencies { id }
                | AssetEvent::Unused { id } => *id,
            };
            if !textures.matches(id) {
                continue;
            }
            if author.saving
                && author.scene_id == state.scene_id
                && matches!(
                    event,
                    AssetEvent::Modified { .. } | AssetEvent::LoadedWithDependencies { .. }
                )
            {
                if id == textures.index.id() {
                    author.pending_images &= !1;
                } // 索引必须独立完成重载。
                if id == textures.weight.id() {
                    author.pending_images &= !2;
                } // 权重不能借索引事件提前放行。
            }
            texture_changed |= match event {
                AssetEvent::Modified { .. } | AssetEvent::Removed { .. } => true,
                AssetEvent::Added { .. } | AssetEvent::LoadedWithDependencies { .. } => {
                    phase == TerrainSurfacePhase::Failed
                }
                AssetEvent::Unused { .. } => false,
            };
        }
    } else {
        image_events.read().for_each(drop);
    }
    if texture_changed {
        state.request_reload();
        stats.set_phase(TerrainSurfacePhase::Loading);
    }
}

/// 当前目录或地表JSON变化后原子建立新的纹理组和RenderApp探针。
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_dynamic_surface_definition(
    frame: feathrise_scene_core::residency::InstanceFrame,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    active_scene: InstanceRes<ActiveDynamicScene>,
    registry: Option<crate::scenes::dynamic_scene::sources::SceneRuntimeCatalog>,
    surface_presets: Option<Res<SurfacePresetRegistry>>,
    fallback: Res<TerrainSurfaceFallbackImages>, // 空作者表无需依赖全局材质数组即可形成合法候选。
    manifests: Res<Assets<SceneManifestAsset>>,
    surfaces: Res<Assets<SceneSurfaceAsset>>,
    mut state: InstanceResMut<DynamicSurfaceState>,
    mut runtime: InstanceResMut<DynamicSceneRuntimeState>,
    mut status: InstanceResMut<DynamicSceneRuntimeStatus>,
    mut stats: InstanceResMut<TerrainSurfaceRenderStats>,
) {
    if !state.definition_dirty {
        return;
    }
    let Some(scene_id) = active_scene.scene_id.as_ref() else {
        return;
    };
    let Some(registry) = registry.as_deref() else {
        return;
    };
    let Some(surface_presets) = surface_presets.as_deref() else {
        return;
    };
    state.catalog_revision = registry.revision;
    let Some(handles) = registry.handles(scene_id) else {
        return;
    };
    state.scene_id = Some(scene_id.clone());
    let Some(surface_handle) = handles.surface.as_ref() else {
        state.definition_dirty = false;
        state.definition = None;
        state.definition_handle = None;
        state.textures = None;
        state.definition_failure_detail = None;
        state.white_requested = true;
        runtime.surface_required = false;
        runtime.surface_ready = false;
        stats.set_phase(TerrainSurfacePhase::White);
        return;
    };
    // 首载文件损坏时也必须保留句柄身份，修复后Added/Loaded事件才能重新进入定义校验。
    state.definition_handle = Some(surface_handle.clone());
    let (Some(surface), Some(manifest)) = (
        surfaces.get(surface_handle),
        manifests.get(&handles.manifest),
    ) else {
        if let LoadState::Failed(error) = asset_server.load_state(surface_handle.id()) {
            // 保留底层解析错误，避免旧格式或缺失文件仅显示笼统失败。
            runtime.surface_ready = false;
            status.phase = DynamicSceneRuntimePhase::Failed;
            status.detail = format!("场景{scene_id}地表JSON加载失败：{error}；保持白模"); // 加载界面与首次失败日志共享真实原因。
            state.definition_failure_detail = Some(status.detail.clone());
            state.definition_dirty = false;
        }
        return;
    };
    if surface.scene_id != *scene_id || manifest.id != *scene_id {
        status.phase = DynamicSceneRuntimePhase::Failed;
        status.detail = format!("场景{scene_id}地表或清单所属ID不一致，保持白模");
        state.definition_failure_detail = Some(status.detail.clone());
        runtime.surface_ready = false;
        state.definition_dirty = false;
        return;
    }
    if !surface.layers.is_empty()
        && (surface_presets.revision == 0 || surface_presets.presets().is_empty())
    {
        // 空表不引用全局材质，非空表仍等待首份有效目录。
        return; // 保持定义脏位直到依赖目录就绪。
    } // 已清空场景可以独立进入裸地状态。
    if let Err(error) = validate_surface_definition(surface, surface_presets) {
        status.phase = DynamicSceneRuntimePhase::Failed;
        status.detail = format!("场景{scene_id}地表定义无效：{error}；保持白模");
        state.definition_failure_detail = Some(status.detail.clone());
        runtime.surface_ready = false;
        state.definition_dirty = false;
        error!("{}", status.detail);
        return;
    }
    let first_definition = state.definition.is_none();
    let textures = load_surface_textures(&asset_server, surface, surface_presets, &fallback); // 空表绑定中性数组并沿用完整原子预热事务。
    if let Some(probe) = state.texture_probe {
        commands
            .entity(probe)
            .insert(TerrainSurfaceTextureProbe::new(textures.images()));
    } else {
        state.texture_probe = Some(
            commands
                .spawn((
                    Name::new("动态场景索引地表五纹理探针"),
                    TerrainSurfaceTextureProbe::new(textures.images()),
                    DespawnOnExit(ScenePhase::Active),
                ))
                .id(),
        );
    }
    state.scene_id = Some(scene_id.clone());
    state.definition_handle = Some(surface_handle.clone());
    if let (Some(instance), Some(probe)) = (frame.instance(), state.texture_probe) {
        commands.entity(probe).insert((feathrise_scene_core::residency::SceneInstanceOwner(instance),
            feathrise_scene_core::residency::SceneInstanceDetached,
            feathrise_environment::terrain_surface::TerrainSurfaceInstanceScope { instance, generation: stats.generation },
            feathrise_environment::terrain_surface::TerrainSurfaceInstanceWarmup)); // 无Transform的纹理探针也必须归实例清理与独立GPU反馈管理。
    }
    state.definition = Some(surface.clone());
    state.textures = Some(textures);
    state.preset_catalog_revision = surface_presets.revision;
    state.definition_failure_detail = None;
    state.white_requested = false;
    state.world_min = Vec2::from_array(manifest.world_min);
    state.world_max = Vec2::from_array(manifest.world_max);
    state.definition_dirty = false;
    if first_definition {
        state.reload_requested = true;
    } else {
        state.request_reload();
    }
    runtime.surface_required = true;
    if state.active_material.is_none() {
        runtime.surface_ready = false;
        status.phase = DynamicSceneRuntimePhase::Waiting;
        status.detail = format!("场景{scene_id}正在异步加载RGBA地表纹理");
    }
    stats.set_phase(TerrainSurfacePhase::Loading);
}

/// 推进唯一源发现、完整纹理门、同网格预热和正式材质接管。
#[allow(clippy::too_many_arguments)]
pub(crate) fn install_dynamic_surface(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    base_materials: Res<Assets<StandardMaterial>>,
    mut materials: ResMut<Assets<TerrainSurfaceMaterial>>,
    clipmap_materials: Res<Assets<SparseClipmapSurfaceMaterial>>,
    mut layer_tables: ResMut<Assets<ShaderBuffer>>,
    fallback: (Res<TerrainSurfaceFallbackImages>, feathrise_scene_core::residency::InstanceFrame, feathrise_scene_core::residency::EntitySourceTransform,
        Query<&super::super::instances::TerrainObjectInstance>),
    surface_presets: Option<Res<SurfacePresetRegistry>>,
    mut state: InstanceResMut<DynamicSurfaceState>,
    mut runtime: InstanceResMut<DynamicSceneRuntimeState>,
    mut status: InstanceResMut<DynamicSceneRuntimeStatus>,
    mut stats: InstanceResMut<TerrainSurfaceRenderStats>,
    sources: InstanceQuery<SourceData, With<DynamicSceneTerrainPrimitive>>, // 常数成本核对当前唯一正式地形源。
    mut warming: InstanceQuery<
        (
            Entity,
            &mut DynamicSurfaceWarmup,
            &MeshMaterial3d<TerrainSurfaceMaterial>,
        ),
        With<TerrainSurfaceCarrier>,
    >,
    carriers: InstanceQuery<Option<&TerrainSurfaceWarmup>, With<TerrainSurfaceCarrier>>,
    mut author: crate::scenes::dynamic_scene::author_sources::TerrainAuthorResMut<DynamicSurfaceAuthorPreview>, // 在正式源所有权内部采用作者材质。
) {
    let (fallback, frame, source_transform, objects) = fallback;
    let terrain_id = frame.instance().and_then(|entity| objects.get(entity).ok()).map(|object| &object.definition.id);
    let author_material = author
        .material
        .as_ref()
        .filter(|material| author.scene_id == state.scene_id && author.matches_terrain(terrain_id)
            && materials.get(material.id()).is_some())
        .cloned(); // 不接收其他场景或已回收资产。
    if state.author_material != author_material {
        if author_material.is_some()
            && let (Some(entity), Some(source)) = (state.warmup_entity, state.warmup_source_entity)
        {
            cancel_surface_warmup(
                &mut commands,
                &mut materials,
                &mut layer_tables,
                &mut state,
                entity,
                source,
                true,
            ); // 新作者源不能与旧文件候选竞争同一正式实体。
        }
        state.author_material = author_material; // 下一步既有reconcile统一维护普通源及Solari租约。
    }
    if state.author_material.is_some()
        && author.file_inputs_ready()
        && state.author_save_ticket != author.save_ticket
    {
        if let (Some(entity), Some(source)) = (state.warmup_entity, state.warmup_source_entity) {
            cancel_surface_warmup(
                &mut commands,
                &mut materials,
                &mut layer_tables,
                &mut state,
                entity,
                source,
                false,
            ); // 保存前候选不能冒充本次文件结果。
        }
        state.revision = state.revision.wrapping_add(1).max(1); // 显式新修订不合并旧Ready。
        state.reload_requested = true; // 沿原有五纹理、表和GPU管线预热。
        state.author_save_ticket = author.save_ticket; // 固定当前严格保存票据。
    }
    let current_source = sources.single().ok().and_then(
        |(
            entity,
            mesh_name,
            material_name,
            _,
            _,
            _,
            _,
            standard,
            height_fallback,
            surface,
            clipmap,
            carrier,
            material_override,
            clipmap_lease,
        )| {
            source_identity(&mesh_name.0, &material_name.0).then_some((
                entity,
                standard,
                height_fallback,
                surface,
                clipmap,
                carrier.is_some(),
                material_override,
                clipmap_lease,
            )) // 标记与GLTF稳定名称必须同时证明正式源身份。
        },
    );
    if surface_source_needs_reconcile(
        &materials,
        &clipmap_materials,
        current_source,
        &state,
        &runtime,
        &stats,
    ) {
        reconcile_current_surface_source(
            &mut commands,
            &materials,
            &clipmap_materials,
            current_source, // Scene换代空窗或非唯一状态由事务保持等待，下一帧自动重核对。
            &mut state,
            &mut runtime,
            &mut stats,
        ); // 稳定Active或白模帧完全只读，只有真实身份差异才写组件和状态。
    }
    if state.author_material.is_some() && !author.file_inputs_ready() {
        return; // 未保存或等待本次文件事件时继续完整作者画面，关闭F12不影响此门。
    }
    if state.white_requested {
        restore_white_model(
            &mut commands,
            &mut materials,
            &mut layer_tables,
            &mut state,
            &sources,
        );
        if let Some(probe) = state.texture_probe.take() {
            commands.entity(probe).try_despawn();
        }
        runtime.surface_required = false;
        runtime.surface_ready = true;
        stats.clear();
        stats.set_phase(TerrainSurfacePhase::White);
        status.detail = state.scene_id.as_ref().map_or_else(
            || "动态场景未声明通用地表，保持白模".to_owned(),
            |scene_id| format!("场景{scene_id}未声明通用地表，保持白模"),
        );
        return;
    }
    if let Some(detail) = state.definition_failure_detail.take() {
        fail_current_revision(
            &mut commands,
            &mut materials,
            &mut layer_tables,
            &mut state,
            &mut runtime,
            &mut status,
            &mut stats,
            &sources,
            TerrainSurfaceFailure::Definition,
        );
        status.detail = detail;
        return;
    }
    let (Some(textures), Some(definition)) = (state.textures.clone(), state.definition.clone())
    else {
        return;
    };
    let failure = surface_texture_failure(&asset_server, &textures);
    if failure != TerrainSurfaceFailure::None {
        fail_current_revision(
            &mut commands,
            &mut materials,
            &mut layer_tables,
            &mut state,
            &mut runtime,
            &mut status,
            &mut stats,
            &sources,
            failure,
        );
        return;
    }
    if stats.pipeline_error != 0
        && (state.active_material.is_some() || state.candidate_material.is_some())
    {
        fail_current_revision(
            &mut commands,
            &mut materials,
            &mut layer_tables,
            &mut state,
            &mut runtime,
            &mut status,
            &mut stats,
            &sources,
            TerrainSurfaceFailure::ShaderPipeline,
        );
        return;
    }
    if let Ok((warmup_entity, mut warmup, candidate)) = warming.single_mut() {
        if !state.current(warmup.generation, warmup.revision) {
            cancel_surface_warmup(
                &mut commands,
                &mut materials,
                &mut layer_tables,
                &mut state,
                warmup_entity,
                warmup.source_entity,
                true,
            ); // 过期候选必须完整回收，并为当前修订保留唯一一次重试。
            stats.set_phase(TerrainSurfacePhase::Loading); // 当前修订尚未建立新候选。
            return;
        }
        let Ok((
            _,
            mesh_name,
            material_name,
            _,
            _,
            _,
            _,
            standard,
            height_fallback,
            surface,
            clipmap,
            _,
            _,
            _,
        )) = sources.get(warmup.source_entity)
        else {
            if state.source_entity == Some(warmup.source_entity) {
                state.source_entity = None; // 仅清除仍指向旧源的状态，不能覆盖本帧刚发现的新primitive。
            }
            cancel_surface_warmup(
                &mut commands,
                &mut materials,
                &mut layer_tables,
                &mut state,
                warmup_entity,
                warmup.source_entity,
                true,
            ); // Scene换代销毁旧源时同时清理句柄和候选，避免永久卡住。
            if state.active_material.is_none() {
                runtime.surface_ready = false; // 没有可转交活动材质时继续保持正式等待。
            }
            stats.set_phase(TerrainSurfacePhase::Loading);
            return;
        };
        if !source_identity(&mesh_name.0, &material_name.0)
            || (standard.is_none()
                && height_fallback.is_none()
                && surface.is_none()
                && clipmap.is_none())
        {
            if state.source_entity == Some(warmup.source_entity) {
                state.source_entity = None; // 身份失效的旧源不再允许承接任何候选。
            }
            cancel_surface_warmup(
                &mut commands,
                &mut materials,
                &mut layer_tables,
                &mut state,
                warmup_entity,
                warmup.source_entity,
                true,
            ); // 名称或材质路径失效同样使用统一完整取消事务。
            if state.active_material.is_none() {
                runtime.surface_ready = false; // 只有已活动材质仍可等待下一正式源后立即转交。
            }
            stats.set_phase(TerrainSurfacePhase::Loading);
            return;
        }
        warmup.observed_frames = warmup.observed_frames.saturating_add(1); // 只有仍绑定当前正式源的候选才累计RenderApp观察帧。
        let expected_carriers = if frame.instance().is_some() { 1 } else { dynamic_surface_warmup_carrier_count(&carriers) }; // 父/子反馈均只验本次明确候选，旧活动网格可被F12隐藏。
        if !terrain_surface_warmup_can_finalize(&stats, expected_carriers, warmup.observed_frames) {
            stats.set_phase(TerrainSurfacePhase::Warming);
            return; // 纹理、管线和唯一Carrier证据尚未共同成立时继续离屏预热。
        }
        let candidate = candidate.0.clone();
        let old_active = state.active_material.take();
        let old_active_table = state.active_layer_table.take();
        commands
            .entity(warmup.source_entity)
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .remove::<MeshMaterial3d<ClipmapHeightMaterial>>()
            .remove::<MeshMaterial3d<TerrainSurfaceMaterial>>()
            .remove::<MeshMaterial3d<SparseClipmapSurfaceMaterial>>()
            .remove::<TerrainSurfaceMaterialOverride>() // 真实地表修订采用前显式撤销旧Deferred租约。
            .remove::<SparseClipmapSurfaceLease>()
            .remove::<DynamicSurfaceSourceWarmup>()
            .insert((MeshMaterial3d(candidate.clone()), TerrainSurfaceCarrier));
        if let Some(instance) = frame.instance() {
            commands.entity(warmup.source_entity).insert(feathrise_environment::terrain_surface::TerrainSurfaceInstanceScope {
                instance, generation: stats.generation });
            if let Some(probe) = state.texture_probe { commands.entity(probe).remove::<feathrise_environment::terrain_surface::TerrainSurfaceInstanceWarmup>(); }
        } // 活动子图退出候选统计，仍与父图GPU证据隔离。
        commands.entity(warmup_entity).try_despawn();
        if let Some(old) = old_active
            && old.id() != candidate.id()
        {
            materials.remove(old.id());
        }
        if let Some(old) = old_active_table {
            layer_tables.remove(old.id());
        }
        state.active_material = Some(candidate);
        if state.author_material.is_some()
            && author.file_inputs_ready()
            && state.author_save_ticket == author.save_ticket
        {
            state.author_material = None; // 本次文件候选已真实GPU就绪，同批归还正式源。
            author.clear(); // 消费覆盖，编辑器干净帧不得再次发布旧作者句柄。
        }
        state.active_layer_table = state.candidate_layer_table.take();
        state.candidate_material = None;
        state.warmup_entity = None;
        state.warmup_source_entity = None; // 正式采用后旧源不再持有预热事务身份。
        state.reload_requested = false;
        state.failure_logged_revision = None;
        runtime.surface_ready = true;
        stats.set_phase(TerrainSurfacePhase::Active);
        info!(
            "动态场景RGBA地表已原子接管白模：generation={} revision={} layers={}",
            state.generation,
            state.revision,
            definition.layers.len()
        );
        return;
    }
    if state.warmup_entity.is_some() || !state.reload_requested {
        return;
    }
    if !surface_textures_loaded(&asset_server, &textures) {
        stats.set_phase(TerrainSurfacePhase::Loading);
        return;
    }
    let Some(source_entity) = state.source_entity else {
        stats.set_phase(TerrainSurfacePhase::White);
        return;
    };
    let Ok((
        _,
        mesh_name,
        material_name,
        mesh,
        transform,
        visibility,
        parent,
        standard,
        height_fallback,
        surface,
        clipmap,
        _,
        _,
        _,
    )) = sources.get(source_entity)
    else {
        state.source_entity = None;
        return;
    };
    if !source_identity(&mesh_name.0, &material_name.0)
        || (standard.is_none()
            && height_fallback.is_none()
            && surface.is_none()
            && clipmap.is_none())
    {
        return;
    }
    let Some(white_handle) = state.white_material.clone() else {
        return;
    };
    let Some(white) = base_materials.get(&white_handle).cloned() else {
        return;
    };
    let Some(surface_presets) = surface_presets.as_deref() else {
        return;
    };
    if surface_presets.revision != state.preset_catalog_revision {
        state.definition_dirty = true;
        return; // 目录在本帧更换时拒绝用旧定义身份构建候选。
    }
    let (candidate, layer_table) = match build_surface_material(
        white,
        &definition,
        surface_presets,
        &textures,
        &fallback,
        &mut layer_tables,
        state.world_min,
        state.world_max,
    ) {
        Ok(candidate) => candidate,
        Err(error) => {
            state.definition_failure_detail = Some(format!("场景地表材质构造失败：{error}"));
            fail_current_revision(
                &mut commands,
                &mut materials,
                &mut layer_tables,
                &mut state,
                &mut runtime,
                &mut status,
                &mut stats,
                &sources,
                TerrainSurfaceFailure::Definition,
            );
            return;
        }
    };
    let candidate_handle = materials.add(candidate);
    commands
        .entity(source_entity)
        .insert(DynamicSurfaceSourceWarmup);
    let mut warmup_transform = *transform;
    if frame.instance().is_some() {
        let local = source_transform.affine(source_entity, &GlobalTransform::from(*transform));
        warmup_transform = Transform::from_matrix(frame.frame().source_to_world * Mat4::from(local));
    } // GLB内部多层变换必须保留，不能只乘网格自己的局部Transform。
    warmup_transform.translation.y -= 100_000.0;
    warmup_transform.scale *= Vec3::splat(0.000_001);
    let mut entity = commands.spawn((
        Name::new("动态场景通用地表同网格离屏预热"),
        feathrise_environment::terrain_surface::TerrainSurfaceWarmup, // 预热可见性只服务GPU，不能扩大场景取景范围。
        feathrise_scene_core::residency::SceneInstanceDetached,
        mesh.clone(),
        MeshMaterial3d(candidate_handle.clone()),
        warmup_transform,
        dynamic_surface_warmup_visibility(*visibility), // F12预览会隐藏正式网格，但离屏预热必须仍进入RenderApp。
        NoFrustumCulling,
        TerrainSurfaceCarrier,
        DynamicSurfaceWarmup {
            source_entity,
            generation: state.generation,
            revision: state.revision,
            observed_frames: 0,
        },
        DespawnOnExit(ScenePhase::Active),
    ));
    if frame.instance().is_none() && let Some(parent) = parent {
        entity.insert(ChildOf(parent.parent()));
    }
    if let Some(instance) = frame.instance() {
        entity.insert((feathrise_environment::terrain_surface::TerrainSurfaceInstanceScope { instance, generation: stats.generation },
            feathrise_environment::terrain_surface::TerrainSurfaceInstanceWarmup));
    }
    state.warmup_entity = Some(entity.id());
    state.warmup_source_entity = Some(source_entity); // 保存统一取消所需的旧源身份，不能只依赖可能已消失的Query。
    state.candidate_material = Some(candidate_handle);
    state.candidate_layer_table = Some(layer_table);
    state.reload_requested = false;
    if let Some(probe) = state.texture_probe {
        commands
            .entity(probe)
            .insert(TerrainSurfaceTextureProbe::new(textures.images()));
        if frame.instance().is_some() { commands.entity(probe).insert(feathrise_environment::terrain_surface::TerrainSurfaceInstanceWarmup); }
    }
    stats.set_phase(TerrainSurfacePhase::Warming);
}

/// 动态地表预热使用不受F12正式网格显隐策略影响的独立可见性。
fn dynamic_surface_warmup_visibility(_source_visibility: Visibility) -> Visibility {
    Visibility::Inherited // 实体已移到远裁面外；这里只需确保材质会被专用化并排入Opaque3d。
}

/// 与RenderApp根图候选过滤保持一致；活动源和其他实例不得增加本次预热的期望载体数。
fn dynamic_surface_warmup_carrier_count(
    carriers: &InstanceQuery<Option<&TerrainSurfaceWarmup>, With<TerrainSurfaceCarrier>>,
) -> u32 {
    carriers.iter().filter(|warmup| warmup.is_some()).count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn root_surface_warmup_counts_current_candidate_without_hidden_active_or_other_instance() {
        let mut world = World::new();
        world.spawn((TerrainSurfaceCarrier, Visibility::Hidden)); // F12中完整旧版本让位作者画面。
        let candidate = world.spawn((TerrainSurfaceCarrier, TerrainSurfaceWarmup)).id();
        let other = world.spawn_empty().id();
        world.spawn((TerrainSurfaceCarrier, TerrainSurfaceWarmup, feathrise_scene_core::residency::SceneInstanceOwner(other)));
        let count = world.run_system_once(|carriers: InstanceQuery<Option<&TerrainSurfaceWarmup>, With<TerrainSurfaceCarrier>>| {
            dynamic_surface_warmup_carrier_count(&carriers)
        }).unwrap();
        assert_eq!(count, 1, "旧活动源和其他实例都不能阻塞本次根图候选预热");
        world.entity_mut(candidate).despawn();
        let count = world.run_system_once(|carriers: InstanceQuery<Option<&TerrainSurfaceWarmup>, With<TerrainSurfaceCarrier>>| {
            dynamic_surface_warmup_carrier_count(&carriers)
        }).unwrap();
        assert_eq!(count, 0, "没有本次候选时不能借隐藏活动源证明预热已完成");
    }

    #[test]
    fn hidden_f12_runtime_terrain_does_not_hide_surface_warmup() {
        assert_eq!(
            dynamic_surface_warmup_visibility(Visibility::Hidden),
            Visibility::Inherited,
        ); // F12正式地形让位作者预览时，GPU材质预热仍必须能完成启动门。
    }
}
