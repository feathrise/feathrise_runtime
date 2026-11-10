//! 宿主只读取实例自己的正式证据；不写父世界的加载门，不把任务开始冒充完成。

use bevy::prelude::*;
use feathrise_scene_core::residency::InstanceResourceTable;
use super::super::{resources::DynamicSceneRuntimeState, DynamicSceneRuntimePhase, DynamicSceneRuntimeStatus,
    DynamicVegetationPhase, DynamicVegetationStatus, DynamicWaterPhase, DynamicWaterStatus,
    sparse_runtime::SparseClipmapRuntime, clipmap::{AuthoritativeHeightRuntime, SparseHeightPageRuntime},
    layered_collision::LayeredCollisionRuntime, terrain_derived::DynamicTerrainDerivationState,
    vegetation::DynamicVegetationState, water::{DynamicWaterState, ClipmapWaterRuntime}};
use crate::shared::{placement_data::{ScenePlacementRuntimePhase, ScenePlacementRuntimeStatus},
    portal_data::{ScenePortalRuntimePhase, ScenePortalRuntimeStatus}};

pub(super) fn get<T: Resource>(world: &World, entity: Entity) -> &T {
    &world.resource::<InstanceResourceTable<T>>().values[&entity]
}

/// 只采用本准备席位的上一完整GPU帧；父图反馈和旧实例/旧地表世代都不能借用。
pub(super) fn refresh_surface_gpu_evidence(world: &mut World, entity: Entity) {
    use feathrise_environment::terrain_surface::{TerrainSurfaceInstanceFeedback, TerrainSurfaceRenderStats};
    let Some(mut snapshot) = world.get_resource::<TerrainSurfaceInstanceFeedback>()
        .and_then(|feedback| feedback.0.iter().find(|(instance, _)| *instance == entity).map(|(_, stats)| *stats)) else { return; };
    let mut table = world.resource_mut::<InstanceResourceTable<TerrainSurfaceRenderStats>>();
    let Some(stats) = table.values.get_mut(&entity).filter(|stats| stats.generation == snapshot.generation) else { return; };
    snapshot.phase = stats.phase; snapshot.failure = stats.failure;
    *stats = snapshot;
}

pub(super) fn check(world: &World, entity: Entity) -> Result<bool, String> {
    check_initial_display(world, entity)?;
    let puddles_ready = get::<crate::shared::surface_weather::PuddleRuntimeState>(world, entity).ready()?;
    let application_ready = world.get_resource::<crate::SceneApplicationHooks>()
        .map_or(Ok(true), |hooks| (hooks.instance_ready)(world, entity))?;
    let terrain = get::<DynamicSceneRuntimeStatus>(world, entity);
    let boundary = get::<super::super::boundary::DynamicSceneBoundaryStatus>(world, entity);
    let placements = get::<ScenePlacementRuntimeStatus>(world, entity);
    let portals = get::<ScenePortalRuntimeStatus>(world, entity);
    let vegetation = get::<DynamicVegetationStatus>(world, entity);
    let water = get::<DynamicWaterStatus>(world, entity);
    let height = get::<super::super::terrain_derived::DynamicTerrainHeightStatus>(world, entity);
    for (failed, detail) in [
        (terrain.phase == DynamicSceneRuntimePhase::Failed, terrain.detail.as_str()),
        (height.phase == super::super::terrain_derived::DynamicTerrainHeightPhase::Failed, height.detail.as_str()),
        (boundary.phase == super::super::boundary::DynamicSceneBoundaryPhase::Failed, boundary.detail.as_str()),
        (placements.phase == ScenePlacementRuntimePhase::Failed, placements.detail.as_str()),
        (portals.phase == ScenePortalRuntimePhase::Failed, portals.detail.as_str()),
        (vegetation.phase == DynamicVegetationPhase::Failed, vegetation.detail.as_str()),
        (water.phase == DynamicWaterPhase::Failed, water.detail.as_str()),
    ] { if failed { return Err(detail.to_owned()); } }
    // 某域仍在等待时也须优先报告其他域的失败，不能让“加载0/1”掩盖真实错误。
    Ok(puddles_ready && application_ready
        && super::super::systems::dynamic_scene_startup_inputs_ready(get::<DynamicSceneRuntimeState>(world, entity))
        && placements.phase == ScenePlacementRuntimePhase::Ready
        && portals.phase == ScenePortalRuntimePhase::Ready
        && vegetation.phase == DynamicVegetationPhase::Ready
        && matches!(water.phase, DynamicWaterPhase::Ready | DynamicWaterPhase::Disabled))
}

/// 地形对象没有独立角色、传送或边界；只核对它真正拥有的几何与依附内容。
pub(super) fn check_terrain(world: &World, entity: Entity) -> Result<bool, String> {
    check_initial_display(world, entity)?;
    let placements = get::<ScenePlacementRuntimeStatus>(world, entity);
    let terrain = get::<DynamicSceneRuntimeStatus>(world, entity);
    let height = get::<super::super::terrain_derived::DynamicTerrainHeightStatus>(world, entity);
    let vegetation = get::<DynamicVegetationStatus>(world, entity);
    let water = get::<DynamicWaterStatus>(world, entity);
    for (failed, detail) in [
        (terrain.phase == DynamicSceneRuntimePhase::Failed, terrain.detail.as_str()),
        (height.phase == super::super::terrain_derived::DynamicTerrainHeightPhase::Failed, height.detail.as_str()),
        (vegetation.phase == DynamicVegetationPhase::Failed, vegetation.detail.as_str()),
        (water.phase == DynamicWaterPhase::Failed, water.detail.as_str()),
        (placements.phase == ScenePlacementRuntimePhase::Failed, placements.detail.as_str()),
    ] { if failed { return Err(detail.to_owned()); } }
    Ok(super::super::systems::dynamic_scene_startup_inputs_ready(get::<DynamicSceneRuntimeState>(world, entity))
        && placements.phase == ScenePlacementRuntimePhase::Ready
        && get::<crate::shared::surface_weather::PuddleRuntimeState>(world, entity).ready()?
        && vegetation.phase == DynamicVegetationPhase::Ready
        && matches!(water.phase, DynamicWaterPhase::Ready | DynamicWaterPhase::Disabled))
}

/// 只读取当前实例的有限状态；GLB与Clipmap分别报告各自真实等待证据。
pub(super) fn waiting_detail(world: &World, entity: Entity) -> String {
    let runtime = get::<DynamicSceneRuntimeState>(world, entity);
    if runtime.sparse_height_path.is_some() && (!runtime.sparse_clipmap_ready || !runtime.sparse_height_pages_ready) {
        let display = get::<feathrise_scene_content::SceneTerrainDisplayLodStatus>(world, entity);
        if !display.detail.is_empty() { return display.detail.clone(); }
    }
    if !runtime.collision_ready {
        return if runtime.glb_path.is_some() {
            get::<super::super::terrain_derived::DynamicTerrainHeightStatus>(world, entity).detail.clone()
        } else { get::<DynamicSceneRuntimeStatus>(world, entity).detail.clone() };
    }
    if !runtime.surface_ready { return "正在准备子场景地表".into(); }
    if !runtime.boundary_ready { return get::<super::super::boundary::DynamicSceneBoundaryStatus>(world, entity).detail.clone(); }
    if runtime.sparse_height_path.is_some() && (!runtime.sparse_clipmap_ready || !runtime.sparse_height_pages_ready) {
        return "正在准备子场景Clipmap首屏高度页".into();
    }
    let placements = get::<ScenePlacementRuntimeStatus>(world, entity);
    if placements.phase != ScenePlacementRuntimePhase::Ready { return placements.detail.clone(); }
    let portals = get::<ScenePortalRuntimeStatus>(world, entity);
    if portals.phase != ScenePortalRuntimePhase::Ready { return portals.detail.clone(); }
    let vegetation = get::<DynamicVegetationStatus>(world, entity);
    if vegetation.phase != DynamicVegetationPhase::Ready { return vegetation.detail.clone(); }
    let water = get::<DynamicWaterStatus>(world, entity);
    if !matches!(water.phase, DynamicWaterPhase::Ready | DynamicWaterPhase::Disabled) { return water.detail.clone(); }
    if !get::<crate::shared::surface_weather::PuddleRuntimeState>(world, entity).ready().unwrap_or(false) { return "正在准备子场景积水纹理".into(); }
    "正在准备子场景应用内容".into()
}

/// 首屏GPU失败必须结束等待；已经有完整显示的热更新失败仍保留原场景。
fn check_initial_display(world: &World, entity: Entity) -> Result<(), String> {
    let runtime = get::<DynamicSceneRuntimeState>(world, entity);
    let display = get::<feathrise_scene_content::SceneTerrainDisplayLodStatus>(world, entity);
    if runtime.sparse_height_path.is_some()
        && (!runtime.sparse_clipmap_ready || !runtime.sparse_height_pages_ready)
        && display.phase == feathrise_scene_content::SceneTerrainDisplayLodPhase::Failed
    {
        return Err(display.detail.clone());
    }
    Ok(())
}

pub(super) fn busy(world: &World, entity: Entity) -> bool {
    get::<super::super::glb_objects::GlbObjectRuntime>(world, entity).busy()
        || super::super::volume_stream::busy(world, entity)
        || get::<super::super::voxel_objects::VoxelObjectRuntime>(world, entity).busy()
        || get::<super::super::boundary::DynamicSceneBoundaryState>(world, entity).background_busy()
        || get::<crate::shared::surface_weather::PuddleRuntimeState>(world, entity).busy(world.resource::<AssetServer>())
        || get::<SparseClipmapRuntime>(world, entity).background_busy()
        || get::<SparseHeightPageRuntime>(world, entity).background_busy()
        || get::<AuthoritativeHeightRuntime>(world, entity).background_busy()
        || get::<LayeredCollisionRuntime>(world, entity).background_busy(world.resource::<AssetServer>())
        || get::<DynamicTerrainDerivationState>(world, entity).background_busy()
        || get::<DynamicVegetationState>(world, entity).background_busy()
        || get::<DynamicWaterState>(world, entity).background_busy()
        || get::<ClipmapWaterRuntime>(world, entity).background_busy()
}

pub(super) fn finished(world: &World, entity: Entity) -> bool {
    let server = world.resource::<AssetServer>();
    get::<super::super::glb_objects::GlbObjectRuntime>(world, entity).finished()
        && super::super::volume_stream::finished(world, entity)
        && get::<super::super::voxel_objects::VoxelObjectRuntime>(world, entity).finished()
        && get::<super::super::boundary::DynamicSceneBoundaryState>(world, entity).background_finished()
        && !get::<crate::shared::surface_weather::PuddleRuntimeState>(world, entity).busy(server)
        && get::<SparseClipmapRuntime>(world, entity).background_finished()
        && get::<SparseHeightPageRuntime>(world, entity).background_finished(server)
        && get::<AuthoritativeHeightRuntime>(world, entity).background_finished(server)
        && get::<LayeredCollisionRuntime>(world, entity).background_finished(server)
        && get::<DynamicTerrainDerivationState>(world, entity).background_finished()
        && get::<DynamicVegetationState>(world, entity).background_finished()
        && get::<DynamicWaterState>(world, entity).background_finished()
        && get::<ClipmapWaterRuntime>(world, entity).background_finished()
}

#[cfg(test)]
mod tests {
    use super::*;
    use feathrise_scene_content::{SceneTerrainDisplayLodPhase, SceneTerrainDisplayLodStatus};

    #[test]
    fn first_gpu_failure_and_timeout_stop_waiting_but_hot_reload_keeps_old_display() {
        for detail in ["显示LOD GPU管线预热失败", "显示LOD GPU预热超过30秒"] {
            let mut world = World::new();
            let entity = world.spawn_empty().id();
            let mut runtime = InstanceResourceTable::<DynamicSceneRuntimeState>::default();
            let mut state = DynamicSceneRuntimeState::default();
            state.sparse_height_path = Some("scenes/test/height.json".into());
            runtime.values.insert(entity, state);
            world.insert_resource(runtime);
            let mut display = InstanceResourceTable::<SceneTerrainDisplayLodStatus>::default();
            display.values.insert(entity, SceneTerrainDisplayLodStatus {
                phase: SceneTerrainDisplayLodPhase::Failed, detail: detail.into(), ..default()
            });
            world.insert_resource(display);
            assert_eq!(check_initial_display(&world, entity), Err(detail.into()));
            assert_eq!(waiting_detail(&world, entity), detail);
            let mut runtime = world.resource_mut::<InstanceResourceTable<DynamicSceneRuntimeState>>();
            let state = runtime.values.get_mut(&entity).unwrap();
            state.sparse_clipmap_ready = true;
            state.sparse_height_pages_ready = true;
            assert_eq!(check_initial_display(&world, entity), Ok(()));
        }
    }
}
