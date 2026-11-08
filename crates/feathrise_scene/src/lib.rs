//! 通用场景入口：装配作者地图、资源生命周期与中性应用交接。
//! `maps`只维护单张地图内容，`shared`只维护可跨地图复用的基础能力。

use feathrise_scene_core::residency::SceneResidencyAppExt;

mod application;
pub mod animation_retarget; // SDK作者预览与游戏共享跨GLB骨架映射和骨长保护，不依赖应用角色类型。
pub mod author_view;
pub use application::{SceneViewLayers, ScenePhysicsPolicy, SceneApplicationHooks, SceneApplicationStartupStatus, SceneApplicationSet, SceneCollisionParticipant, SceneRuntimeObserver, VegetationInteractionSource, WaterInteractionSource};
pub use maps::dynamic_map::sources::MapRuntimeCatalog;

pub mod components; // 地图 GLB 热重建登记组件，供各地图生成入口复用。
pub mod gpu_grass_content; // 数据预设解析与稳定旧引用兼容；游戏专用GLB和外观由应用注入。
mod maps; // 统一数据驱动地图的内部实现。
mod resources; // 地图热重载期间 Avian 物理暂停所有权资源。
mod shared; // 场景运行时、GLB地形、动态水和热重载公共能力。
#[cfg(test)]
mod shader_test_support;
#[cfg(test)]
mod test_fixtures; // SDK自有中性输入仅供回归；产品不会引用测试目录或私有游戏素材。

pub use shared::scene_residency::{
    ResidentEntityIndex, ResidentScenePreparation, ResidentScenes, arm_resident_scene_transition, close_resident_scene,
    mark_resident_document_open, prepare_resident_scene, register_resident_scene_root,
    watch_scene_application_asset,
};

use bevy::prelude::*;

#[cfg(any(feature = "test_panel", feature = "author_view"))] // F12与作者预览跨crate共享唯一显式地形目标。
pub use components::TerrainGridTarget; // 发布构建不暴露调试标记。
pub use components::{
    DynamicMapTerrainPrimitive, DynamicSparseClipmapPrimitive, HotReloadableMapScene,
}; // 稀疏GPU位移地形排除旧静态Mesh光追目标。
#[cfg(feature = "runtime_test_support")]
pub use maps::dynamic_map::runtime_test_support; // 仅测试依赖可构造活动身份；正式运行时不导出可写夹具。
pub use maps::{
    clipmap_brush_overlay_evidence, ClipmapBrushOverlayEvidence,
    CLIPMAP_VEGETATION_OVERLAY_LAYER, // 编辑器只读高度之外的独立覆盖输入。
    ClipmapGroundHit,
    ClipmapGroundPublishSet, // Clipmap笔刷使用同代真实地表。
    ClipmapGroundSnapshot,
    ClipmapRaytracingAnchor,
    ClipmapRaytracingInstanceRoute,
    ClipmapRaytracingPhase,
    ClipmapRaytracingPublishSet,
    ClipmapRaytracingRequest,
    ClipmapRaytracingStatus,
    ClipmapSplineWaterSurface,
    ClipmapVegetationBrush,
    ClipmapVegetationDensity,
    ClipmapOverlaySet,
    ClipmapWaterMaterial,
    ClipmapWaterSurface,
    DYNAMIC_TERRAIN_MESH_NAME,
    DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME,
    DynamicBladeDraft,
    DynamicBladeDraftSnapshot, // 完整Blade会话快照供F12发布。
    DynamicClipmapAuthoringView,
    DynamicClipmapGroundSurface,
    DynamicClipmapVegetationOverlay,
    DynamicClipmapSurfaceOverlay, ClipmapSurfaceCoverage, ClipmapSurfaceOverlayIdentity,
    DynamicClipmapWaterDraft,
    DynamicClipmapWaterDraftSnapshot,
    DynamicMapBoundaryOverride,
    DynamicMapBoundaryPhase,
    DynamicMapBoundaryStatus,
    DynamicMapRuntimePhase,
    DynamicMapRuntimeStatus,
    SubMapInstance, SubMapActive, SubMapRuntimeStatus, TerrainObjectInstance, TerrainObjectTransformPreview,
    TerrainObjectAuthoringTarget,
    TerrainObjectStitchStatus,
    TerrainVegetationDrafts, TerrainWaterDrafts,
    TerrainObjectResource,
    TerrainObjectEditorVisibility,
    TerrainObjectClipmapGround,
    TerrainAuthoringApply,
    TerrainVolumeAuthorPreviewRoot,
    DynamicMeshProfileReadiness,
    DynamicMeshProfilePhase,
    DynamicMeshProfileRequests,
    DynamicMeshProfileStatus,
    DynamicSurfaceAuthorPreview,
    DynamicTerrainHeightCache,
    TerrainObjectQueries, TerrainObjectRaySnapshot, TerrainSurfaceVersion,
    DynamicTerrainHeightData,
    DynamicTerrainHeightPhase,
    DynamicTerrainHeightStatus,
    DynamicVegetationPhase,
    DynamicVegetationStatus,
    DynamicWaterPhase,
    DynamicWaterRebuildRequest, TerrainWaterRebuildRequest,
    DynamicWaterStatus,
    LayeredCollisionDiagnosticsSystemSet, // 唯一多层碰撞诊断的读取边界。
    SparseClipmapSurfaceMaterial,
    StaticTerrainRaytracingSources,
    StreamedVolumeResident,
    TerrainTopologySystems,
    TerrainVolumePreviewRequest,
    TerrainVolumePreviewTarget,
    TerrainVolumeStreamControl,
    TerrainVolumeStreamStatus,
    TerrainVolumeStreamSystems,
    VolumeDisplayBuild,
    VolumeSurfaceFields,
    build_volume_display,
    validate_layered_collision_settings, // 作者保存任务调用，禁止在UI帧内执行完整规划。
    volume_lod_signature,
}; // F3通过场景crate稳定入口读取接管证据，并在诊断发布后同帧应用新节点LOD状态。
#[cfg(any(feature = "test_panel", feature = "author_view"))] // 当前地图网格状态只存在本进程内。
pub use resources::TerrainGridPreview; // 菜单、加载和切图由F12会话统一更新。
pub use shared::authoring_version::AuthoringVersion; // 编辑器保存只冻结轻量作者拥有权，不复制完整摆放或水覆盖。
pub use shared::building_vegetation_clearance::{
    BuildingVegetationClearanceAuthoringIdentity, BuildingVegetationClearanceSnapshot,
    BuildingVegetationClearanceState,
}; // F12 与运行时植被通过同一完整净空快照换代，不写回作者密度图。
#[cfg(any(feature = "test_panel", feature = "author_view"))]
pub use shared::large_map::TerrainShadingPreview; // 仅开发构建暴露F12临时着色预览。
#[cfg(any(feature = "test_panel", feature = "author_view"))]
pub use shared::large_map::terrain_backfaces::TerrainBackfacesPreview; // 全部父/子实例和玩法视图共用的地形显示意图。
pub use shared::large_map::{
    BuiltWaterSurface, DynamicWaterMaterial, DynamicWaterMeshOverrideNumericPatch,
    DynamicWaterMeshOverrideStatus, DynamicWaterMeshOverrides, MAX_MAP_SPLINE_WATER_TRIANGLES,
    MAX_RIVER_RIBBON_SECTIONS, MAX_TERRAIN_GPU_UPLOAD_BYTES_PER_FRAME,
    MAX_WATER_GPU_UPLOAD_BYTES_PER_FRAME, MAX_WATERFALL_RIBBON_SECTIONS, PaintedWaterMeshData,
    PaintedWaterPurpose, PaintedWaterSurface, PaintedWaterSurfaceSpawn, PaintedWaterTerrainInput,
    StagedWaterSurface, WaterGpuUploadQueue, WaterGpuUploadReceipts, WaterGpuUploadTicket,
    WaterSurfaceDescriptor, WaterSurfaceKind, WaterSurfaceMeshData, advance_water_surface_material,
    build_painted_water_mesh, build_painted_water_mesh_with_depth, build_river_water_surface,
    build_waterfall_water_surface, create_water_surface_material, spawn_painted_water_surface,
    spawn_staged_water_surface, spawn_staged_water_warmup, spawn_water_surface,
    retire_water_cpu, stage_water_surface_assets, stage_water_surface_chunk, chunk_water_surfaces, validate_spline_water_build_budget,
}; // 编辑器与动态地图共用湖、河、急流和瀑布的正式GPU水面实现，不复制预览Shader。
pub use shared::placement_data::{
    ATTRIBUTE_AUTHORING_WIRE, DeferredTerrainPreviewCollider, MapPlacementInstance, MapPlacementPrimitiveVisual,
    editable_cuboid_mesh, editable_mesh_mesh, spawn_builtin_placement_visual, set_builtin_placement_collision_enabled, set_builtin_placement_collision_enabled_world, set_builtin_placement_batch_enabled, set_builtin_placement_batch_enabled_world,
    MapPlacementRebuildRequest, MapPlacementRuntimePhase, MapPlacementRuntimeStatus,
    MapPlacementWaterEvidence, PlacementPendingInstance, TerrainMeshUpload,
    TerrainPatchColliderFailure, TerrainPatchCollisionRequest, building_placement_collision,
    direct_primitive_requires_skinning, terrain_placement_collision,
}; // F12与保存门以公开只读实例快照定位正式placement根，并复用直接primitive蒙皮安全门。
pub use shared::portal_data::{MapPortalRuntimePhase, MapPortalRuntimeStatus}; // F3与F12读取数据驱动传送图阶段和失败原因。

use maps::MapsPlugin; // crate内部统一装配数据驱动地图。

/// 通用场景总插件，注册地图、热更新和可选应用完成门。
#[derive(Default)] // 无运行时字段，允许应用按默认值直接构造插件。
pub struct GameScenePlugin; // 根应用只需添加该插件即可启用完整场景域。

pub use shared::mesh_lod::{MeshLodAuthoringState, MeshLodRuntimeStatus, StaticMeshLodOriginal};

impl Plugin for GameScenePlugin {
    fn finish(&self, app: &mut App) {
        let limit = app
            .get_sub_app(bevy::render::RenderApp)
            .and_then(|render| {
                render
                    .world()
                    .get_resource::<bevy::render::renderer::RenderDevice>()
            })
            .map(|device| {
                u64::from(device.limits().max_storage_buffer_binding_size)
                    .min(device.limits().max_buffer_size)
            });
        if let Some(mut stream) = app
            .world_mut()
            .get_resource_mut::<TerrainVolumeStreamControl>()
        {
            stream.gpu_storage_limit = limit;
        }
    }
    fn build(&self, app: &mut App) {
        app.configure_sets(Update, (SceneApplicationSet::PublishSource, SceneApplicationSet::PublishReadiness, SceneApplicationSet::CompleteStartup).chain());
        app.init_resource::<SceneViewLayers>()
            .init_resource::<ScenePhysicsPolicy>()
            .init_resource::<feathrise_settings::SceneDisplaySettings>()
            .init_resource::<feathrise_settings::EnvironmentPreviewSettings>(); // 独立场景插件也具备应用无关的安全输入。
        shared::scene_residency::install(app); // 先登记地图所有权；普通单图模式仍沿用原加载与卸载。
        shared::mesh_lod::install(app); // 全图静态网格LOD持有全局信用，不随单个地形上下文重复注册。
        #[cfg(any(feature = "test_panel", feature = "author_view"))]
        shared::large_map::terrain_backfaces::install(app); // 全局覆盖不随任何父/子地图实例资源切换。
        app.add_plugins((
            shared::portal_data::MapPortalDataPlugin, // 先加载地图目录并为所有玩法地图安装JSON传送图热更新。
            shared::placement_data::MapPlacementDataPlugin, // 复用同一目录加载每图显式GLB/GLTF实例。
            shared::hot_reload::MapHotReloadPlugin,         // 先注册公共热重载与碰撞恢复。
            shared::vegetation_quality::VegetationQualityPlugin, // 所有地图玩法视图统一继承持久植被质量。
            shared::scene_detail_quality::SceneDetailQualityPlugin, // 固定景观新旧 LOD 节点统一继承持久场景细节档。
            MapsPlugin, // 最后装配当前三张地图及其 GPU 叶片内容。
            shared::surface_weather::SurfaceWeatherPlugin, // 地图材质与区域天气只通过唯一固定GPU桥交汇。
            shared::vegetation_weather::VegetationWeatherPlugin, // 天气解析后统一派生植被风向、摆幅和动画速度。
            shared::rain_shelter::RainShelterPlugin, // 无碰撞屋檐通过显式小代理接入所有雨效。
        )); // 完成地图公共插件与私有内容的稳定注册顺序。
        app.add_resident_systems(
            Update,
            shared::scene_runtime::complete_scene_startup
                .in_set(crate::SceneApplicationSet::CompleteStartup)
                .run_if(feathrise_scene_core::state::in_active_scene()),
        );
        #[cfg(any(feature = "test_panel", feature = "author_view"))] // 正式构建不注册F12着色或网格调试能力。
        app.init_resident_resource::<TerrainShadingPreview>() // 平滑/平直保持既有独立预览资源。
            .init_resident_resource::<TerrainGridPreview>() // 显示网格每次进程默认关闭。
            .add_observer(shared::large_map::mark_dynamic_terrain_grid_target) // 动态GLB与Clipmap只按正式地形身份登记目标。
            .add_resident_systems(
                PostUpdate, // F12输入、切图和安全关闭收口后应用最终显示状态。
                (
                    shared::large_map::sync_large_map_shading
                        .run_if(feathrise_scene_core::state::in_active_scene()), // 菜单阶段不访问地表材质。
                    shared::large_map::sync_regular_terrain_grid_visibility, // 开关变化只遍历显式普通地形目标。
                    shared::large_map::sync_changed_regular_terrain_grid_targets, // 新目标和Mesh换代自动恢复选择。
                )
                    .chain(), // 同帧先应用材质着色，再同步独立线框组件。
            ); // 两种显示设置都不触碰碰撞、作者数据或Mesh内容。
        app.add_resident_systems(
            Update, // 玩法场景运行期间持续处理Blend规划节点首次加载、按钮切换和热重载。
            shared::scene_runtime::sync_blender_planning_visibility
                .run_if(feathrise_scene_core::state::in_active_scene()), // 菜单和非玩法场景不扫描命名场景节点。
        ); // 完成地图规划显隐系统注册。
    }
}
