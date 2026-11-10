//! 地形联合采用回归的空批次夹具；私有门构造仍留在摆放所有权模块。
use super::{components::PlacementInstanceGate, resources::*};
use bevy::prelude::*;
use feathrise_scene_content::SceneId;
use std::sync::Arc;

pub(crate) fn install_mesh_upload(app: &mut App) {
    super::terrain_mesh_upload::install(app);
}

pub(crate) fn install_proxy_observers(app: &mut App) {
    use super::terrain_collision::*;
    app.init_resource::<TerrainCollisionRuntime>()
        .init_resource::<TerrainCollisionGarbage>()
        .add_observer(install_ready_collider)
        .add_observer(retire_ready_shape)
        .add_observer(retire_installed_shape)
        .add_observer(retire_physics_proxy)
        .add_observer(register_request)
        .add_observer(unregister_request);
}

pub(crate) fn physical_collider(
    world: &World,
    root: Entity,
) -> Option<&avian3d::prelude::Collider> {
    let proxy = world.get::<super::terrain_collision::TerrainColliderProxy>(root)?;
    world.get(proxy.0)
}

pub(crate) fn collision_fixture(
    generation: u64,
    collider: avian3d::prelude::Collider,
    meshes: Arc<[(Entity, Handle<Mesh>)]>,
) -> (
    super::TerrainPatchCollisionRequest,
    super::TerrainPatchColliderReady,
) {
    super::terrain_collision::collision_fixture(generation, collider, meshes)
}

pub(crate) fn closing_candidate(id: SceneId, container: Entity) -> ScenePlacementRuntimeState {
    ScenePlacementRuntimeState {
        active_scene_id: Some(id.clone()),
        generation: 4,
        revision: 9,
        dirty: false,
        terrain_patch_active: true,
        pending: Some(PendingPlacementRevision {
            terrain_patch_count: 0,
            terrain_holes: Arc::default(),
            generation: 4,
            revision: 9,
            scene_id: id,
            building_preset_revision: 0,
            building_clearances: Arc::default(),
            building_clearance_floor_heights: Arc::default(),
            plans: Vec::new(),
        }),
        scene_load: Some(PlacementSceneLoadTracker {
            generation: 4,
            revision: 9,
            placement_ids: default(),
            scene_ids: default(),
            ready_scene_ids: default(),
            queued_scene_ids: default(),
            scene_checks: default(),
        }),
        staging: Some(PlacementStaging {
            reuse: Default::default(),
            terrain_roots: Vec::new(),
            root: container,
            roots: Vec::new(),
            gate: PlacementInstanceGate::new(false),
            generation: 4,
            revision: 9,
            building_preset_revision: 0,
            building_clearances: Arc::default(),
            building_clearance_floor_heights: Arc::default(),
            next_plan: 0,
            ready_roots: default(),
            blocked_scene_assets: default(),
            total_instances: 0,
            container_observed: true,
            eligibility_epoch: 0,
            verification_epoch: 0,
            verification_cursor: 0,
        }),
        ..default()
    }
}
