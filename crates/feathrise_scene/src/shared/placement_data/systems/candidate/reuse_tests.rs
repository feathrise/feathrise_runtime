//! 原隐藏候选和退役链的实体复用业务回归；不以直接写Ready代替生产采用。

use super::super::retirement::{cleanup_retired_placement_trees, retire_candidate};
use super::*;
use crate::shared::world_asset_lifecycle::WorldAssetLifecycleIndex;
use bevy::asset::AssetPlugin;

#[derive(Component, Debug, PartialEq)]
struct GameplayMemory(u64);

fn record(id: &str) -> feathrise_scene_content::ScenePlacementRecord {
    let mut value: feathrise_scene_content::ScenePlacementRecord = serde_json::from_str(&format!(
        r#"{{"id":"{id}","asset_path":"","translation":[0,1,0],"yaw_degrees":0,"scale":[1,1,1]}}"#
    ))
    .unwrap();
    value.asset_target = feathrise_scene_content::ScenePlacementAssetTarget::DEFAULT_CUBOID;
    value
}

fn fixture(count: usize) -> (App, Vec<Entity>, Entity) {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<WorldAssetLifecycleIndex>()
        .init_resource::<ScenePlacementRuntimeStatus>()
        .init_resource::<BuildingVegetationClearanceState>();
    let map = SceneId("MAP-reuse".into());
    let container = app
        .world_mut()
        .spawn((
            PlacementStagingRoot,
            Visibility::Inherited,
            Transform::IDENTITY,
        ))
        .id();
    let gate = PlacementInstanceGate::new(true);
    let mut roots = Vec::new();
    for index in 0..count {
        let root = app
            .world_mut()
            .spawn((
                ScenePlacementInstance::new_pending(
                    &map,
                    &record(&format!("kept-{index}")),
                    gate.clone(),
                ),
                Transform::from_xyz(17.0, 3.0, 4.0), // 运行时已移动；未改作者记录不能重置这个运行状态。
                Visibility::Inherited,
                GameplayMemory(42),
                ChildOf(container),
            ))
            .id();
        app.world_mut().spawn((GameplayMemory(99), ChildOf(root)));
        roots.push(root);
    }
    let state = ScenePlacementRuntimeState {
        active_scene_id: Some(map),
        generation: 1,
        revision: 2,
        active: Some(PlacementActiveBatch {
            building_preset_revision: 0,
            terrain_roots: Vec::new(),
            primitive_collision_roots: Vec::new(),
            root: container,
            roots: roots.clone(),
            gate,
            marker_cleanup_cursor: roots.len(),
        }),
        ..Default::default()
    };
    app.insert_resource(state);
    (app, roots, container)
}

fn begin(app: &mut App, records: Vec<feathrise_scene_content::ScenePlacementRecord>) {
    app.world_mut()
        .resource_mut::<ScenePlacementRuntimeState>()
        .pending = Some(super::super::super::resources::PendingPlacementRevision {
        terrain_patch_count: 0,
        terrain_holes: Default::default(),
        generation: 1,
        revision: 2,
        scene_id: SceneId("MAP-reuse".into()),
        building_preset_revision: 0,
        building_clearances: Default::default(),
        building_clearance_floor_heights: Default::default(),
        plans: records
            .into_iter()
            .map(|record| PendingPlacementPlan {
                transform: Transform::from_translation(Vec3::from_array(record.translation)),
                record,
                scene: Default::default(),
                primitive: None,
            })
            .collect(),
    });
    app.add_systems(Update, tick);
}

fn tick(
    mut commands: Commands,
    server: Res<AssetServer>,
    meshes: Res<Assets<Mesh>>,
    mut state: ResMut<ScenePlacementRuntimeState>,
    mut status: ResMut<ScenePlacementRuntimeStatus>,
    mut clearances: ResMut<BuildingVegetationClearanceState>,
    mut visibility: InstanceQuery<&mut Visibility, With<PlacementStagingRoot>>,
    inputs: PlacementReuseInputs,
) {
    if state.pending.is_none() {
        return;
    }
    advance_placement_candidate(
        &mut commands,
        &server,
        &meshes,
        &SceneId("MAP-reuse".into()),
        &mut state,
        &mut status,
        &mut clearances,
        &mut visibility,
        &inputs,
    );
}

#[test]
fn placement_reuse_create_delete_preserves_unmodified_gameplay_entity_and_children() {
    let (mut app, old, container) = fixture(2);
    let child = app.world().get::<Children>(old[0]).unwrap()[0];
    begin(&mut app, vec![record("kept-0"), record("new")]);
    for _ in 0..8 {
        app.update();
        if app.world().resource::<ScenePlacementRuntimeStatus>().phase
            == ScenePlacementRuntimePhase::Ready
        {
            break;
        }
    }
    assert_eq!(
        app.world().resource::<ScenePlacementRuntimeStatus>().phase,
        ScenePlacementRuntimePhase::Ready
    );
    let active = app
        .world()
        .resource::<ScenePlacementRuntimeState>()
        .active
        .as_ref()
        .unwrap();
    assert_eq!(active.roots[0], old[0]);
    assert_ne!(active.roots[1], old[1]);
    for _ in 0..8 {
        cleanup_retired_placement_trees(app.world_mut());
    }
    assert!(app.world().get_entity(container).is_err());
    assert!(app.world().get_entity(old[1]).is_err());
    assert_eq!(
        app.world().get::<GameplayMemory>(old[0]),
        Some(&GameplayMemory(42))
    );
    assert_eq!(
        app.world().get::<GameplayMemory>(child),
        Some(&GameplayMemory(99))
    );
    assert_eq!(
        app.world().get::<Transform>(old[0]).unwrap().translation,
        Vec3::new(17.0, 3.0, 4.0)
    );
    assert!(
        app.world()
            .get::<ScenePlacementInstance>(old[0])
            .unwrap()
            .is_active()
    );
}

#[test]
fn placement_reuse_cancel_returns_roots_and_removes_carry_without_losing_state() {
    use bevy::ecs::system::RunSystemOnce;
    let (mut app, roots, old_container) = fixture(1);
    begin(&mut app, vec![record("kept-0")]);
    app.update(); // 首帧只预留容器，不可能提前采用。
    let carry = app
        .world()
        .resource::<ScenePlacementRuntimeState>()
        .staging
        .as_ref()
        .unwrap()
        .reuse
        .carry
        .unwrap()
        .0;
    assert_eq!(
        app.world().get::<ChildOf>(roots[0]).unwrap().parent(),
        carry
    );
    app.world_mut()
        .run_system_once(
            |mut commands: Commands, mut state: ResMut<ScenePlacementRuntimeState>| {
                retire_candidate(&mut commands, &mut state);
            },
        )
        .unwrap();
    for _ in 0..8 {
        cleanup_retired_placement_trees(app.world_mut());
    }
    assert!(app.world().get_entity(carry).is_err());
    assert_eq!(
        app.world().get::<ChildOf>(roots[0]).unwrap().parent(),
        old_container
    );
    assert_eq!(
        app.world().get::<GameplayMemory>(roots[0]),
        Some(&GameplayMemory(42))
    );
    assert!(
        app.world()
            .get::<ScenePlacementInstance>(roots[0])
            .unwrap()
            .is_active()
    );
    assert!(
        app.world()
            .get::<PlacementPendingRevision>(roots[0])
            .is_none()
    );
}

#[test]
fn placement_reuse_large_index_is_bounded_and_does_not_publish_partial_batch() {
    let (mut app, roots, _) = fixture(300);
    begin(
        &mut app,
        (0..300)
            .map(|index| record(&format!("kept-{index}")))
            .collect(),
    );
    app.update();
    let state = app.world().resource::<ScenePlacementRuntimeState>();
    let staging = state.staging.as_ref().unwrap();
    assert_eq!(staging.reuse.cursor, 128);
    assert_eq!(staging.next_plan, 0);
    assert_eq!(state.active.as_ref().unwrap().roots, roots);
    assert_eq!(
        app.world().resource::<ScenePlacementRuntimeStatus>().revision,
        0
    );
}
