//! 地图私有实现入口。
//! 每张地图在自己的目录中维护场景配置和玩法地标，公共底座统一从shared复用。

use feathrise_scene_core::residency::SceneResidencyAppExt;

use bevy::prelude::*; // Bevy ECS、网格、材质和灯光常用类型。
use feathrise_environment::EnvironmentPlugin; // 统一注册草地、树木及灌木岩石公共底座，具体内容仍由地图层维护。

pub(crate) mod dynamic_map; // 编辑器新建地图共用的数据驱动白模场景。

pub(crate) use self::dynamic_map::ClipmapRainHeightMaterial; // 仅供crate内雨水高度代理使用。
pub use self::dynamic_map::{
    clipmap_brush_overlay_evidence, ClipmapBrushOverlayEvidence,
    CLIPMAP_VEGETATION_OVERLAY_LAYER, // F12 贴地覆盖公共输入。
    ClipmapGroundHit,
    ClipmapGroundPublishSet, // 贴地笔刷查询公开入口。
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
    DynamicBladeDraftSnapshot, // 未保存密度与正式窗口共享生命周期。
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
    LayeredCollisionDiagnosticsSystemSet, // 唯一多层碰撞诊断发布顺序。
    SparseClipmapSurfaceMaterial,
    StaticTerrainRaytracingSources,
    StreamedVolumeResident,
    VolumeSurfaceFields, VolumeDisplayBuild, build_volume_display, volume_lod_signature,
    TerrainTopologySystems,
    TerrainVolumePreviewRequest,
    TerrainVolumePreviewTarget,
    TerrainVolumeStreamControl,
    TerrainVolumeStreamStatus,
    TerrainVolumeStreamSystems,
    validate_layered_collision_settings, // 新碰撞模式的后台配置预算检查。
};
#[cfg(test)]
pub(crate) use self::dynamic_map::{test_clipmap_water_material, test_clipmap_water_surface};

use self::dynamic_map::DynamicMapPlugin;
use crate::shared::large_map::LargeMapWaterPlugin;

/// 地图只由目录和统一动态运行时装配，素材与公共植被后端独立保留。
pub(crate) struct MapsPlugin;

impl Plugin for MapsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((EnvironmentPlugin, DynamicMapPlugin, LargeMapWaterPlugin))
            .init_resident_resource::<crate::gpu_grass_content::trail::BladeTrailState>();
    }
}
