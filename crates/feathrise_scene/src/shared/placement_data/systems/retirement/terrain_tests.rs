//! 删除洞穴或离图时立即撤销旧物理，实体树仍按原预算分帧回收。
use super::*;
use crate::shared::placement_data::{
    components::{PlacementInstanceGate, PlacementStagingRoot},
    resources::{ScenePlacementRuntimeStatus, PlacementActiveBatch},
    terrain_collision::{TerrainColliderProxy, TerrainPatchCollisionRequest},
    terrain_topology_test_support::{
        closing_candidate, collision_fixture, install_proxy_observers,
    },
};
use avian3d::prelude::Collider;
use feathrise_scene_content::SceneId;

fn check_retired_physics(replacement: bool) {
    let mut app = App::new();
    install_proxy_observers(&mut app);
    app.init_resource::<ScenePlacementRuntimeStatus>()
        .init_resource::<crate::BuildingVegetationClearanceState>();
    let old_container = app
        .world_mut()
        .spawn((PlacementStagingRoot, Visibility::Inherited))
        .id();
    let ordinary = app
        .world_mut()
        .spawn((Collider::cuboid(1.0, 1.0, 1.0), ChildOf(old_container)))
        .id();
    let cave = app
        .world_mut()
        .spawn((
            collision_fixture(
                1,
                Collider::cuboid(2.0, 1.0, 2.0),
                std::sync::Arc::default(),
            ),
            crate::shared::placement_data::InstallTerrainPatchCollider,
            GlobalTransform::default(),
            ChildOf(old_container),
        ))
        .id();
    let new_container = app
        .world_mut()
        .spawn((PlacementStagingRoot, Visibility::Hidden))
        .id();
    app.world_mut().flush();
    let physical = app.world().get::<TerrainColliderProxy>(cave).unwrap().0;
    let mut state = closing_candidate(SceneId("MAP-retired-cave".into()), new_container);
    state.terrain_mesh_generation = Some([4, 9]); // 新空批次已经联合采用关闭洞口。
    state.active = Some(PlacementActiveBatch {
        building_preset_revision: 0,
        terrain_roots: vec![cave],
        primitive_collision_roots: Vec::new(),
        root: old_container,
        roots: vec![ordinary, cave],
        gate: PlacementInstanceGate::new(true),
        marker_cleanup_cursor: 0,
    });
    app.insert_resource(state).add_systems(
        Update,
        move |mut commands: Commands,
              mut state: ResMut<ScenePlacementRuntimeState>,
              mut status: ResMut<ScenePlacementRuntimeStatus>,
              mut clearance: ResMut<crate::BuildingVegetationClearanceState>,
              mut visibility: feathrise_scene_core::residency::InstanceQuery<&mut Visibility, With<PlacementStagingRoot>>| {
            if replacement {
                super::super::candidate::finish_placement_candidate(
                    &mut commands,
                    &mut state,
                    &mut status,
                    &mut clearance,
                    &mut visibility,
                );
            } else {
                retire_active_batch(&mut commands, &mut state);
            }
        },
    );
    app.update();
    // 不运行树清理：模型根仍存在，却已不能挡角色或继续抢占烘焙槽。
    assert!(app.world().get_entity(cave).is_ok());
    assert!(app.world().get::<Collider>(cave).is_none());
    assert!(app.world().get_entity(physical).is_err());
    assert!(
        app.world()
            .get::<TerrainPatchCollisionRequest>(cave)
            .is_none()
    );
    assert!(app.world().get::<Collider>(ordinary).is_some());
    assert_eq!(
        app.world()
            .resource::<ScenePlacementRuntimeState>()
            .retired_trees
            .len(),
        1
    );
    if replacement {
        assert_eq!(
            app.world().resource::<ScenePlacementRuntimeStatus>().phase,
            super::super::super::resources::ScenePlacementRuntimePhase::Ready
        );
        assert_eq!(
            app.world().get::<Visibility>(new_container),
            Some(&Visibility::Inherited)
        );
    }
}

#[test]
fn deleting_last_cave_removes_old_collider_before_tree_cleanup() {
    check_retired_physics(true);
}

#[test]
fn leaving_scene_removes_old_collider_before_tree_cleanup() {
    check_retired_physics(false);
}
