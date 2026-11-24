//! 原隐藏候选和退役链的实体复用业务回归；不以直接写Ready代替生产采用。

use super::super::retirement::{
    cleanup_retired_placement_plan, cleanup_retired_placement_trees, retire_candidate,
};
use super::*;
use crate::shared::world_asset_lifecycle::WorldAssetLifecycleIndex;
use avian3d::prelude::{
    ColliderOf, Gravity, PhysicsPlugins, RigidBodyColliders, SpatialQuery, SpatialQueryFilter,
};
use bevy::asset::AssetPlugin;
use bevy::{camera::visibility::VisibilityPlugin, ecs::system::RunSystemOnce, mesh::MeshPlugin};

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

fn candidate_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), MeshPlugin))
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<WorldAssetLifecycleIndex>()
        .init_resource::<ScenePlacementRuntimeStatus>()
        .init_resource::<BuildingVegetationClearanceState>()
        .insert_resource(ScenePlacementRuntimeState {
            active_scene_id: Some(SceneId("MAP-reuse".into())),
            generation: 1,
            ..Default::default()
        })
        .add_systems(Update, tick);
    app
}

fn fixture(count: usize) -> (App, Vec<Entity>, Entity) {
    let mut app = candidate_app();
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
    // 复用要验证正式可见子节点。此夹具没有共享刚体绑定，用于隔离验证原来的携带和取消行为。
    let visual_roots = roots.clone();
    app.world_mut()
        .run_system_once(move |mut commands: Commands| {
            for &root in &visual_roots {
                assert!(crate::spawn_builtin_placement_visual(
                    &mut commands,
                    root,
                    feathrise_scene_content::ScenePlacementAssetTarget::DEFAULT_CUBOID,
                ));
            }
        })
        .unwrap();
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
    let mut state = app.world_mut().resource_mut::<ScenePlacementRuntimeState>();
    state.revision += 1;
    state.pending = Some(super::super::super::resources::PendingPlacementRevision {
        motion_clips: None,
        terrain_patch_count: 0,
        terrain_holes: Default::default(),
        generation: state.generation,
        revision: state.revision,
        scene_id: SceneId("MAP-reuse".into()),
        building_preset_revision: 0,
        building_clearances: Default::default(),
        building_clearance_floor_heights: Default::default(),
        plans: records
            .into_iter()
            .map(|record| PendingPlacementPlan {
                motion: None,
                transform: Transform::from_translation(Vec3::from_array(record.translation)),
                record,
                scene: Default::default(),
                primitive: None,
            })
            .collect(),
    });
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
fn cancel_restores_late_source_collision_ownership_to_the_active_batch() {
    use super::super::super::terrain_collision::{SourceSceneCollision, TerrainPatchCollisionRequest};

    let (mut app, roots, container) = fixture(1);
    let root = roots[0];
    let mut source = record("kept-0");
    source.asset_target = feathrise_scene_content::ScenePlacementAssetTarget::Scene0;
    source.asset_path = "models/source-mesh.glb".into();
    let gate = app.world().resource::<ScenePlacementRuntimeState>().active.as_ref().unwrap().gate.clone();
    app.world_mut().entity_mut(root).insert((
        ScenePlacementInstance::new_pending(&SceneId("MAP-reuse".into()), &source, gate),
        SourceSceneCollision::new(false),
    ));
    begin(&mut app, vec![source]);
    app.update();
    // 复用之后 World 换代才在候选读到子节点显式碰撞。它的形状仍在后台准备。
    app.world_mut().entity_mut(root).insert((SourceSceneCollision::new(true), TerrainPatchCollisionRequest::default()));
    app.world_mut().resource_mut::<ScenePlacementRuntimeState>().staging.as_mut().unwrap()
        .primitive_collision_roots.push(root);
    app.world_mut().run_system_once(|mut commands: Commands, mut state: ResMut<ScenePlacementRuntimeState>| {
        retire_candidate(&mut commands, &mut state);
    }).unwrap();
    for _ in 0..2 { cleanup_retired_placement_trees(app.world_mut()); }
    assert_eq!(app.world().get::<ChildOf>(root).unwrap().parent(), container);
    assert_eq!(app.world().resource::<ScenePlacementRuntimeState>().active.as_ref().unwrap().primitive_collision_roots, [root]);
}

#[test]
fn same_saved_cube_survives_retirement_of_its_old_shared_rigid_body() {
    let mut app = candidate_app();
    app.add_plugins((TransformPlugin, VisibilityPlugin, PhysicsPlugins::default()))
        .insert_resource(Gravity(Vec3::ZERO))
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(1.0 / 60.0),
        ))
        .add_systems(
            Update,
            (cleanup_active_placement_markers, cleanup_retired_placement_plan).after(tick),
        );
    app.finish();

    let cube = record("saved-cube");
    begin(&mut app, vec![cube.clone()]);
    let (old_body, old_visual) = wait_for_cube(&mut app, 1);
    assert_eq!(app.world().get::<ColliderOf>(old_visual).map(|link| link.body), Some(old_body));
    assert!(app.world().get::<RigidBodyColliders>(old_body).unwrap().contains(&old_visual));
    assert_eq!(app.world_mut().run_system_once(cube_ray_hit).unwrap(), Some(old_visual));

    // 同样的记录会被文件监听再次提交。旧刚体和 Mesh3d 之间还存在独立于 ChildOf 的 linked_spawn 关系。
    begin(&mut app, vec![cube]);
    app.update();
    assert_eq!(app.world().resource::<ScenePlacementRuntimeStatus>().revision, 1);
    assert_eq!(app.world_mut().run_system_once(cube_ray_hit).unwrap(), Some(old_visual),
        "候选隐藏准备期间，已有立方体必须继续承托");
    for _ in 0..8 {
        if app.world().resource::<ScenePlacementRuntimeStatus>().revision == 2 { break; }
        app.update();
    }
    assert_eq!(app.world().resource::<ScenePlacementRuntimeStatus>().revision, 2);
    assert_eq!(app.world().get::<Visibility>(old_body), Some(&Visibility::Hidden));
    assert!(app.world().get::<RigidBodyDisabled>(old_body).is_some());

    // 单独推进生产退役器，确保通过的是刚体真正销毁之后的画面和物理，而不是接管间隙。
    for _ in 0..8 {
        cleanup_retired_placement_trees(app.world_mut());
    }
    assert!(app.world().get_entity(old_body).is_err());
    let (body, visual) = wait_for_cube(&mut app, 2);
    assert_ne!(body, old_body);
    assert!(app.world().get::<RigidBodyDisabled>(body).is_none());
    assert_eq!(app.world().get::<ColliderOf>(visual).map(|link| link.body), Some(body));
    assert!(app.world().get::<RigidBodyColliders>(body).unwrap().contains(&visual));
    assert_eq!(app.world_mut().run_system_once(cube_ray_hit).unwrap(), Some(visual));
}

fn wait_for_cube(app: &mut App, revision: u64) -> (Entity, Entity) {
    for _ in 0..8 {
        if app.world().resource::<ScenePlacementRuntimeStatus>().revision == revision { break; }
        app.update();
    }
    let world = app.world();
    let status = world.resource::<ScenePlacementRuntimeStatus>();
    assert_eq!(status.phase, ScenePlacementRuntimePhase::Ready, "{}", status.detail);
    assert_eq!(status.revision, revision);
    assert_eq!(status.placement_count, 1);
    let active = world.resource::<ScenePlacementRuntimeState>().active.as_ref().unwrap();
    assert_eq!(active.roots.len(), 1);
    let root = active.roots[0];
    assert!(world.get::<ScenePlacementInstance>(root).unwrap().is_active());
    assert!(crate::builtin_placement_visual_ready(world, root), "活动立方体必须保有真实网格");
    let visual = world.get::<Children>(root).unwrap()[0];
    let mesh = &world.get::<Mesh3d>(visual).unwrap().0;
    assert_eq!(world.resource::<Assets<Mesh>>().get(mesh).unwrap().count_vertices(), 36);
    assert!(world.get::<InheritedVisibility>(visual).unwrap().get());
    (active.root, visual)
}

fn cube_ray_hit(physics: SpatialQuery, ground: feathrise_grounding::GroundSurfaceFilter) -> Option<Entity> {
    physics.cast_ray_predicate(Vec3::new(0.0, 5.0, 0.0), Dir3::NEG_Y, 10.0, true,
        &SpatialQueryFilter::default(), &|entity| ground.is_active(entity)).map(|hit| hit.entity)
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
