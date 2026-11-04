//! 使用真实状态调度、实体关系和资源槽验证驻留；三个动态地图共享同一个枚举。

use super::resources::ResidentEntityIndex;
use super::*;
use avian3d::prelude::*;
use bevy::{
    asset::AssetPlugin,
    ecs::{entity_disabling::Disabled, query::Allow},
    state::app::StatesPlugin,
};
use feathrise_map_content::{ActiveDynamicMap, MapId};
use feathrise_scene_core::residency::{ResidentMapEntity, SceneResidencyAppExt};
use std::sync::Arc;
#[path = "gpu_tests.rs"]
mod gpu_tests;
#[path = "camera_tests.rs"]
mod camera_tests; // 真实Window/camera_system经过驻留恢复后重取当前尺寸与投影。

#[derive(Resource, Default)]
struct MapRuntimeProof {
    pixels: Arc<Vec<u8>>,
    image: Handle<Image>,
    serial: usize,
}
#[derive(Resource, Default)]
struct ConstructionCounts {
    entered: usize,
    exited: usize,
}
#[derive(Component)]
struct SceneActor(u64);
#[derive(Component)]
struct OriginalDisabled;

fn cold_enter(
    mut commands: Commands,
    mut proof: ResMut<MapRuntimeProof>,
    mut counts: ResMut<ConstructionCounts>,
    mut images: ResMut<Assets<Image>>,
) {
    counts.entered += 1;
    proof.serial = counts.entered;
    proof.pixels = Arc::new(vec![counts.entered as u8; 16_384]);
    proof.image = images.add(Image::default());
    commands
        .spawn((
            Name::new(format!("地图{}完整场景根", counts.entered)),
            SceneActor(0),
            DespawnOnExit(ScenePhase::Active),
            Visibility::Inherited,
            RigidBody::Static,
            Collider::cuboid(2.0, 2.0, 2.0),
        ))
        .with_children(|parent| {
            parent.spawn((Name::new("原相机"), Camera::default()));
            parent.spawn((Name::new("原隐藏节点"), Visibility::Hidden));
            parent.spawn((
                OriginalDisabled,
                Disabled,
                ColliderDisabled,
                RigidBodyDisabled,
                Collider::sphere(0.5),
                RigidBody::Static,
            ));
        });
    let mut gate = feathrise_scene_core::state::SceneStartupGate::waiting_for_collision(ScenePhase::Active);
    gate.collision_ready = true;
    gate.ready = true;
    commands.insert_resource(gate);
}
fn cold_exit(mut counts: ResMut<ConstructionCounts>) {
    counts.exited += 1;
}
fn update_actors(mut actors: Query<&mut SceneActor>) {
    for mut actor in &mut actors {
        actor.0 += 1;
    }
}

fn fixture() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), StatesPlugin))
        .init_state::<ScenePhase>()
        .init_resource::<ActiveDynamicMap>()
        .init_resource::<Assets<Image>>()
        .init_resource::<ConstructionCounts>();
    install(&mut app);
    app.world_mut()
        .resource_mut::<SceneResidencyActivity>()
        .enabled = true;
    app.init_resident_resource::<MapRuntimeProof>()
        .add_systems(OnEnter(ScenePhase::Active), cold_enter)
        .add_systems(OnExit(ScenePhase::Active), cold_exit)
        .add_resident_systems(Update, update_actors);
    app
}

#[test]
fn live_preview_orphaned_heights_retire_through_existing_single_background_slot() {
    use feathrise_map_content::{SparseHeightLiveOverride, SparseHeightSpec, SparseHeightTile, SparseHeightTileKey};
    let mut app = App::new();
    app.add_plugins(MinimalPlugins).init_resource::<ResidentScenes>();
    let spec = SparseHeightSpec::new([0.0; 2], [64.0; 2], 1.0, -8.0, 16.0).unwrap();
    let map = id("MAP-live-retire-slot");
    let mut weak = Vec::new();
    for revision in 1..=2 {
        let tile = Arc::new(SparseHeightTile::blank(&spec, SparseHeightTileKey { x: 0, z: 0 }).unwrap());
        weak.push(Arc::downgrade(&tile));
        let mut height = SparseHeightLiveOverride::default();
        height.retarget(Some(map.clone()), revision);
        height.publish_tiles(&map, revision, [tile]).unwrap();
        app.world_mut().resource_mut::<ResidentScenes>().retire_sparse_height(height);
    }
    assert!(weak.iter().all(|tile| tile.upgrade().is_some()));
    super::systems::retire_closed_scenes(app.world_mut());
    {
        let scenes = app.world().resource::<ResidentScenes>();
        assert!(scenes.retirement.is_some());
        assert_eq!(scenes.retired_resources.len(), 1, "一次只把一份根交给既有后台槽，不另开并发任务");
        assert!(weak[1].upgrade().is_some(), "第二份根仍在原队列，主帧不能提前释放");
    }
    for _ in 0..2000 {
        super::systems::retire_closed_scenes(app.world_mut());
        let scenes = app.world().resource::<ResidentScenes>();
        if scenes.retirement.is_none() && scenes.retired_resources.is_empty() { break; }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let scenes = app.world().resource::<ResidentScenes>();
    assert!(scenes.retirement.is_none() && scenes.retired_resources.is_empty());
    assert!(weak.iter().all(|tile| tile.upgrade().is_none()), "两份最后根最终都由唯一worker释放");
}
fn id(value: &str) -> MapId {
    MapId::parse(value).unwrap()
}

#[test]
fn cancelled_cold_loading_placeholder_is_retired_without_touching_other_maps() {
    let mut app = fixture();
    let target = id("MAP-cancelled-before-enter");
    let other = id("MAP-other-resident");
    app.world_mut().resource_mut::<ResidentScenes>().ensure(target.clone(), ScenePhase::Active);
    app.world_mut().resource_mut::<ResidentScenes>().ensure(other.clone(), ScenePhase::Active);
    close_resident_scene(app.world_mut(), target.clone());
    systems::retire_closed_scenes(app.world_mut());
    assert!(!app.world().resource::<ResidentScenes>().contains(&target));
    assert!(app.world().resource::<ResidentScenes>().contains(&other));
}
fn enter_first(app: &mut App, map: &MapId) -> Entity {
    app.world_mut()
        .resource_mut::<ActiveDynamicMap>()
        .select(map.clone());
    app.world_mut()
        .resource_mut::<NextState<ScenePhase>>()
        .set(ScenePhase::Active);
    app.update();
    mark_resident_document_open(app.world_mut(), map.clone());
    app.world_mut()
        .query_filtered::<Entity, With<SceneActor>>()
        .single(app.world())
        .unwrap()
}
fn prepare(app: &mut App, map: &MapId) -> ResidentScenePreparation {
    prepare_resident_scene(app.world_mut(), map.clone(), ScenePhase::Active);
    for _ in 0..100 {
        app.update();
        let state = app.world().resource::<ResidentScenes>().preparation(map);
        if matches!(
            state,
            ResidentScenePreparation::ColdReady | ResidentScenePreparation::WarmReady
        ) {
            return state;
        }
        assert!(app.world().resource::<SceneResidencyActivity>().switching);
    }
    panic!("有界实体交接必须最终完成");
}
fn adopt(app: &mut App, map: &MapId, warm: bool) {
    app.world_mut()
        .resource_mut::<ActiveDynamicMap>()
        .select(map.clone());
    arm_resident_scene_transition(app.world_mut());
    app.world_mut()
        .resource_mut::<NextState<ScenePhase>>()
        .set(if warm {
            ScenePhase::Active
        } else {
            ScenePhase::Loading
        });
    app.update();
    if !warm {
        app.world_mut()
            .resource_mut::<NextState<ScenePhase>>()
            .set(ScenePhase::Active);
        app.update();
    }
    mark_resident_document_open(app.world_mut(), map.clone());
    assert!(!app.world().resource::<SceneResidencyActivity>().switching);
}

#[test]
fn cold_loading_open_document_keeps_handoff_and_startup_gate_until_scene_entry() {
    // 作者草稿可早于三维冷载完成；LoadingScene里的目标标签不能被当作待暂停的源场景。
    for has_source in [false, true] {
        let mut app = fixture();
        let source = id("MAP-loading-source");
        let target = id("MAP-loading-target");
        let source_root = has_source.then(|| enter_first(&mut app, &source));
        assert_eq!(
            prepare(&mut app, &target),
            ResidentScenePreparation::ColdReady
        );
        app.world_mut()
            .resource_mut::<ActiveDynamicMap>()
            .select(target.clone());
        app.world_mut()
            .insert_resource(feathrise_scene_core::state::PendingSceneTransition::editor_map(
                ScenePhase::Active,
                "测试冷载地图",
            ));
        arm_resident_scene_transition(app.world_mut());
        app.world_mut()
            .resource_mut::<NextState<ScenePhase>>()
            .set(ScenePhase::Loading);
        app.update();
        mark_resident_document_open(app.world_mut(), target.clone());
        for _ in 0..3 {
            app.update(); // 模拟关键资源仍在加载，但作者草稿已经采用且标签已经打开。
        }
        assert!(
            app.world()
                .contains_resource::<feathrise_scene_core::state::PendingSceneTransition>()
        );
        assert!(app.world().resource::<ResidentScenes>().active.is_none());
        assert!(
            app.world()
                .resource::<SceneResidencyActivity>()
                .active
                .is_none()
        );

        // 复用正式加载门的交接契约：消费请求、创建碰撞启动门并申请进入正式场景。
        app.world_mut()
            .remove_resource::<feathrise_scene_core::state::PendingSceneTransition>();
        app.world_mut()
            .insert_resource(feathrise_scene_core::state::SceneStartupGate::waiting_for_collision(
                ScenePhase::Active,
            ));
        app.world_mut()
            .resource_mut::<NextState<ScenePhase>>()
            .set(ScenePhase::Active);
        app.update();
        assert_eq!(
            *app.world().resource::<State<ScenePhase>>().get(),
            ScenePhase::Active,
            "加载请求已经消费后必须直接进入冷场景，不能再次暂停一个尚未创建的目标"
        );
        let scenes = app.world().resource::<ResidentScenes>();
        assert!(scenes.active_is(&target));
        assert!(!scenes.sleeping(&target));
        assert!(scenes.maps[&target].resources.is_none());
        assert!(!app.world().resource::<SceneResidencyActivity>().switching);
        assert!(app.world().resource::<feathrise_scene_core::state::SceneStartupGate>().ready);
        let root = app
            .world_mut()
            .query_filtered::<Entity, With<SceneActor>>()
            .single(app.world())
            .unwrap();
        assert!(app.world().get::<Disabled>(root).is_none());
        assert_eq!(
            app.world().resource::<ConstructionCounts>().entered,
            1 + usize::from(has_source)
        );

        if let Some(source_root) = source_root {
            assert!(app.world().resource::<ResidentScenes>().sleeping(&source));
            assert!(app.world().get::<Disabled>(source_root).is_some());
            assert_eq!(
                prepare(&mut app, &source),
                ResidentScenePreparation::WarmReady
            );
            adopt(&mut app, &source, true);
            assert!(app.world().get::<Disabled>(source_root).is_none());
            assert!(app.world().resource::<feathrise_scene_core::state::SceneStartupGate>().ready);
            assert_eq!(app.world().resource::<ConstructionCounts>().entered, 2);
        }
    }
}

#[test]
fn external_cold_loading_preserves_source_and_enters_enabled_target_scene() {
    let mut app = fixture();
    let source = id("MAP-external-source");
    let target = id("MAP-external-target");
    let source_root = enter_first(&mut app, &source);
    app.world_mut()
        .resource_mut::<ActiveDynamicMap>()
        .select(target.clone());
    app.world_mut()
        .resource_mut::<NextState<ScenePhase>>()
        .set(ScenePhase::Loading);
    for _ in 0..20 {
        app.update();
        if *app.world().resource::<State<ScenePhase>>().get() == ScenePhase::Loading {
            break;
        }
    }
    // 传送等正式入口不预登记目标；仍应先暂停源图，再沿原加载页进入启用的目标场景。
    assert_eq!(
        *app.world().resource::<State<ScenePhase>>().get(),
        ScenePhase::Loading
    );
    assert!(app.world().resource::<ResidentScenes>().active.is_none());
    app.world_mut()
        .resource_mut::<NextState<ScenePhase>>()
        .set(ScenePhase::Active);
    app.update();
    assert!(app.world().resource::<ResidentScenes>().active_is(&target));
    assert!(app.world().resource::<ResidentScenes>().sleeping(&source));
    assert!(app.world().get::<Disabled>(source_root).is_some());
    let target_root = app
        .world_mut()
        .query_filtered::<Entity, With<SceneActor>>()
        .single(app.world())
        .unwrap();
    assert_ne!(target_root, source_root);
    assert!(app.world().get::<Disabled>(target_root).is_none());
    assert!(app.world().resource::<feathrise_scene_core::state::SceneStartupGate>().ready);
    assert_eq!(app.world().resource::<ConstructionCounts>().entered, 2);
}

#[test]
fn three_full_scenes_reuse_entities_resources_camera_and_collision_without_rebuilding() {
    let mut app = fixture();
    let (a, b, c) = (
        id("MAP-resident-a"),
        id("MAP-resident-b"),
        id("MAP-resident-c"),
    );
    let root_a = enter_first(&mut app, &a);
    let pixels_a = app.world().resource::<MapRuntimeProof>().pixels.clone();
    let image_a = app.world().resource::<MapRuntimeProof>().image.id();
    let camera_a = app.world().get::<Children>(root_a).unwrap()[0];
    let hidden_a = app.world().get::<Children>(root_a).unwrap()[1];
    let disabled_a = app.world().get::<Children>(root_a).unwrap()[2];
    assert_eq!(prepare(&mut app, &b), ResidentScenePreparation::ColdReady);
    let paused_tick = app.world().get::<SceneActor>(root_a).unwrap().0;
    adopt(&mut app, &b, false);
    assert_eq!(prepare(&mut app, &c), ResidentScenePreparation::ColdReady);
    adopt(&mut app, &c, false);
    for _ in 0..20 {
        app.update();
    }
    assert_eq!(
        app.world().get::<SceneActor>(root_a).unwrap().0,
        paused_tick
    );
    let mut all_actors = app
        .world_mut()
        .query_filtered::<&SceneActor, Allow<Disabled>>();
    assert_eq!(all_actors.iter(app.world()).count(), 3);
    assert!(app.world().get::<ColliderDisabled>(root_a).is_some());
    assert!(!app.world().get::<Camera>(camera_a).unwrap().is_active);
    assert_eq!(prepare(&mut app, &a), ResidentScenePreparation::WarmReady);
    assert!(!app.world().get::<Camera>(camera_a).unwrap().is_active); // 半采用场景不能先开启视图。
    adopt(&mut app, &a, true);
    let active = app
        .world_mut()
        .query_filtered::<Entity, With<SceneActor>>()
        .single(app.world())
        .unwrap();
    assert_eq!(active, root_a);
    let proof = app.world().resource::<MapRuntimeProof>();
    assert!(Arc::ptr_eq(&pixels_a, &proof.pixels));
    assert_eq!(proof.image.id(), image_a);
    assert_eq!(proof.serial, 1);
    assert!(app.world().get::<Camera>(camera_a).unwrap().is_active);
    assert_eq!(
        *app.world().get::<Visibility>(hidden_a).unwrap(),
        Visibility::Hidden
    );
    assert!(app.world().get::<Disabled>(disabled_a).is_some());
    assert!(app.world().get::<ColliderDisabled>(disabled_a).is_some());
    assert!(app.world().get::<RigidBodyDisabled>(disabled_a).is_some());
    assert!(app.world().get::<ColliderDisabled>(root_a).is_none());
    assert!(app.world().get::<RigidBodyDisabled>(root_a).is_none());
    assert!(app.world().resource::<feathrise_scene_core::state::SceneStartupGate>().ready);
    let counts = app.world().resource::<ConstructionCounts>();
    assert_eq!((counts.entered, counts.exited), (3, 0));
}

#[test]
fn avian_spatial_query_only_hits_active_map_and_restores_original_collider() {
    // 使用真实Avian碰撞树，而非只检查Disabled标记；同位置的两张地图不能互相命中。
    #[derive(Resource, Default)]
    struct RayProof(Option<Entity>);
    fn probe(query: SpatialQuery, mut proof: ResMut<RayProof>) {
        proof.0 = query
            .cast_ray(
                Vec3::Y * 4.0,
                Dir3::NEG_Y,
                10.0,
                true,
                &SpatialQueryFilter::default(),
            )
            .map(|hit| hit.entity);
    }
    let mut app = fixture();
    app.add_plugins((
        TransformPlugin,
        bevy::mesh::MeshPlugin,
        PhysicsPlugins::default(),
    ))
    .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / 60.0),
    ))
    .init_resource::<RayProof>()
    .add_systems(Last, probe.after(super::systems::advance_scene_residency));
    app.finish();
    let (a, b) = (id("MAP-physics-a"), id("MAP-physics-b"));
    let root_a = enter_first(&mut app, &a);
    app.update();
    assert_eq!(app.world().resource::<RayProof>().0, Some(root_a));
    prepare(&mut app, &b);
    assert_eq!(app.world().resource::<RayProof>().0, None);
    adopt(&mut app, &b, false);
    app.update();
    let root_b = app
        .world_mut()
        .query_filtered::<Entity, With<SceneActor>>()
        .single(app.world())
        .unwrap();
    assert_eq!(app.world().resource::<RayProof>().0, Some(root_b));
    prepare(&mut app, &a);
    assert_eq!(app.world().resource::<RayProof>().0, Some(root_a)); // 完整恢复但尚未开启相机；物理步仍由切图锁暂停。
    adopt(&mut app, &a, true);
    app.update();
    assert_eq!(app.world().resource::<RayProof>().0, Some(root_a));
    assert!(app.world().get::<Collider>(root_a).is_some());
    assert!(app.world().get::<Disabled>(root_b).is_some());
}

#[test]
fn without_residency_original_scoped_scene_and_exit_contract_are_unchanged() {
    let mut app = fixture();
    app.world_mut()
        .resource_mut::<SceneResidencyActivity>()
        .enabled = false;
    let root = enter_first(&mut app, &id("MAP-original-lifecycle"));
    assert!(app.world().get::<ResidentMapEntity>(root).is_none());
    assert!(
        app.world()
            .resource::<ResidentEntityIndex>()
            .maps
            .is_empty()
    );
    assert!(
        !app.world()
            .resource::<ResidentScenes>()
            .contains(&id("MAP-original-lifecycle"))
    );
    app.world_mut()
        .resource_mut::<NextState<ScenePhase>>()
        .set(ScenePhase::Loading);
    app.update();
    assert!(app.world().get_entity(root).is_err());
    assert_eq!(app.world().resource::<ConstructionCounts>().exited, 1);
}

#[test]
fn large_tree_is_enrolled_and_switched_in_bounded_frames_and_closed_independently() {
    let mut app = fixture();
    let (a, b) = (id("MAP-tree-a"), id("MAP-tree-b"));
    let root = enter_first(&mut app, &a);
    for _ in 0..1_025 {
        app.world_mut()
            .spawn((ChildOf(root), Visibility::Inherited));
    }
    prepare_resident_scene(app.world_mut(), b.clone(), ScenePhase::Active);
    let before = app.world().get::<SceneActor>(root).unwrap().0;
    app.update();
    assert_eq!(
        app.world().resource::<ResidentScenes>().preparation(&b),
        ResidentScenePreparation::Parking
    );
    assert_eq!(app.world().get::<SceneActor>(root).unwrap().0, before);
    for _ in 0..100 {
        app.update();
        if app.world().resource::<ResidentScenes>().preparation(&b)
            == ResidentScenePreparation::ColdReady
        {
            break;
        }
    }
    adopt(&mut app, &b, false);
    let token = app.world().get::<ResidentMapEntity>(root).unwrap().0;
    assert!(app.world().resource::<ResidentEntityIndex>().maps[&token].len() > 1_025);
    let root_b = app
        .world_mut()
        .query_filtered::<Entity, With<SceneActor>>()
        .single(app.world())
        .unwrap();
    close_resident_scene(app.world_mut(), a.clone());
    app.update();
    assert!(app.world().get_entity(root).is_ok()); // 关闭不能在一帧递归despawn整图。
    for _ in 0..100 {
        app.update();
        if !app.world().resource::<ResidentScenes>().contains(&a) {
            break;
        }
    }
    assert!(!app.world().resource::<ResidentScenes>().contains(&a));
    assert!(app.world().get_entity(root).is_err());
    assert!(app.world().get_entity(root_b).is_ok());
    assert!(app.world().resource::<ResidentScenes>().contains(&b));
}

#[test]
fn newly_spawned_dormant_children_inherit_pause_and_external_physics_disable_is_preserved() {
    let mut app = fixture();
    let (a, b) = (id("MAP-late-a"), id("MAP-late-b"));
    let root = enter_first(&mut app, &a);
    prepare(&mut app, &b);
    adopt(&mut app, &b, false);
    let child = app
        .world_mut()
        .spawn((
            ChildOf(root),
            DespawnOnExit(ScenePhase::Active),
            RigidBody::Static,
            Collider::sphere(0.5),
            ColliderDisabled,
        ))
        .id();
    app.update();
    assert_eq!(
        app.world().get::<ResidentMapEntity>(child),
        app.world().get::<ResidentMapEntity>(root)
    );
    assert!(app.world().get::<Disabled>(child).is_some());
    prepare(&mut app, &a);
    adopt(&mut app, &a, true);
    assert!(app.world().get::<Disabled>(child).is_none());
    assert!(app.world().get::<RigidBodyDisabled>(child).is_none());
    assert!(app.world().get::<ColliderDisabled>(child).is_some());
}

#[test]
fn gameplay_loading_and_menu_paths_preserve_open_scenes_and_resume_existing_scene() {
    let mut app = fixture();
    let a = id("MAP-portal-return");
    let root = enter_first(&mut app, &a);
    let pixels = app.world().resource::<MapRuntimeProof>().pixels.clone();
    app.world_mut()
        .resource_mut::<NextState<ScenePhase>>()
        .set(ScenePhase::Idle);
    for _ in 0..40 {
        app.update();
        if *app.world().resource::<State<ScenePhase>>().get() == ScenePhase::Idle {
            break;
        }
    }
    assert!(app.world().resource::<ResidentScenes>().sleeping(&a));
    app.world_mut()
        .resource_mut::<NextState<ScenePhase>>()
        .set(ScenePhase::Loading);
    app.update();
    app.world_mut()
        .resource_mut::<NextState<ScenePhase>>()
        .set(ScenePhase::Active);
    for _ in 0..40 {
        app.update();
        if app.world().resource::<ResidentScenes>().restored(&a) {
            break;
        }
    }
    assert!(app.world().resource::<ResidentScenes>().restored(&a));
    assert!(Arc::ptr_eq(
        &pixels,
        &app.world().resource::<MapRuntimeProof>().pixels
    ));
    assert!(app.world().get::<Disabled>(root).is_none());
    assert_eq!(app.world().resource::<ConstructionCounts>().entered, 1);
}

#[test]
fn warm_identity_survives_resource_move_during_multiframe_restoration() {
    let mut app = fixture();
    let (a, b) = (id("MAP-restore-phase-a"), id("MAP-restore-phase-b"));
    let root = enter_first(&mut app, &a);
    for _ in 0..800 {
        app.world_mut().spawn(ChildOf(root));
    }
    prepare(&mut app, &b);
    adopt(&mut app, &b, false);
    prepare_resident_scene(app.world_mut(), a.clone(), ScenePhase::Active);
    for _ in 0..50 {
        app.update();
        let scenes = app.world().resource::<ResidentScenes>();
        if scenes.preparation(&a) == ResidentScenePreparation::Restoring {
            assert!(scenes.restoring(&a));
            assert!(!scenes.sleeping(&a)); // 原快照已经移到活动槽，作者预览仍应视为暖恢复。
            assert!(app.world().resource::<SceneResidencyActivity>().switching);
            return;
        }
    }
    panic!("应观察到分帧恢复阶段");
}

#[test]
fn fresh_load_reconstructs_world_while_ordinary_return_reuses_it() {
    let mut app = fixture();
    let a = id("MAP-save-reload");
    let root = enter_first(&mut app, &a);
    let pixels = app.world().resource::<MapRuntimeProof>().pixels.clone();
    app.world_mut().insert_resource(
        feathrise_scene_core::state::PendingSceneTransition::new(ScenePhase::Active)
            .with_fresh_world(),
    );
    app.world_mut()
        .resource_mut::<NextState<ScenePhase>>()
        .set(ScenePhase::Loading);
    for _ in 0..40 {
        app.update();
        if *app.world().resource::<State<ScenePhase>>().get() == ScenePhase::Loading {
            break;
        }
    }
    assert!(!app.world().resource::<ResidentScenes>().sleeping(&a));
    app.world_mut()
        .remove_resource::<feathrise_scene_core::state::PendingSceneTransition>();
    app.world_mut()
        .resource_mut::<NextState<ScenePhase>>()
        .set(ScenePhase::Active);
    app.update();
    let new_root = app
        .world_mut()
        .query_filtered::<Entity, With<SceneActor>>()
        .single(app.world())
        .unwrap();
    assert_ne!(new_root, root);
    assert!(!Arc::ptr_eq(
        &pixels,
        &app.world().resource::<MapRuntimeProof>().pixels
    ));
    assert_eq!(app.world().resource::<ConstructionCounts>().entered, 2);
    assert!(app.world().resource::<ResidentScenes>().maps[&a].document_open);
    for _ in 0..10 {
        app.update();
    }
    assert!(app.world().get_entity(root).is_err());
}

#[derive(Resource, Default)]
struct WatchedMesh(Handle<Mesh>);
#[derive(Resource, Default)]
struct MeshChanges(Vec<usize>);
fn capture_mesh_change(
    mut events: feathrise_scene_core::residency::ResidentAssetEventReader<Mesh>,
    mesh: Res<WatchedMesh>,
    proof: Res<MapRuntimeProof>,
    mut changes: ResMut<MeshChanges>,
) {
    for event in events.read() {
        if event.is_modified(mesh.0.id()) {
            changes.0.push(proof.serial);
        }
    }
}

#[test]
fn dormant_shared_asset_changes_replay_once_without_bouncing_between_maps() {
    let mut app = fixture();
    app.init_asset::<Mesh>()
        .init_resident_resource::<WatchedMesh>()
        .init_resource::<MeshChanges>()
        .add_resident_systems(Update, capture_mesh_change);
    let (a, b) = (id("MAP-assets-a"), id("MAP-assets-b"));
    let root_a = enter_first(&mut app, &a);
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::new(2.0, 2.0, 2.0));
    app.world_mut()
        .entity_mut(root_a)
        .insert(Mesh3d(mesh.clone()));
    app.world_mut().resource_mut::<WatchedMesh>().0 = mesh.clone();
    for _ in 0..4 {
        app.update();
    }
    prepare(&mut app, &b);
    adopt(&mut app, &b, false);
    let root_b = app
        .world_mut()
        .query_filtered::<Entity, With<SceneActor>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .entity_mut(root_b)
        .insert(Mesh3d(mesh.clone()));
    app.world_mut().resource_mut::<WatchedMesh>().0 = mesh.clone();
    for _ in 0..4 {
        app.update();
    }
    app.world_mut()
        .resource_mut::<Assets<Mesh>>()
        .insert(mesh.id(), Mesh::from(Cuboid::new(3.0, 3.0, 3.0)))
        .unwrap();
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(app.world().resource::<MeshChanges>().0, vec![2]);
    prepare(&mut app, &a);
    adopt(&mut app, &a, true);
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(app.world().resource::<MeshChanges>().0, vec![2, 1]);
    prepare(&mut app, &b);
    adopt(&mut app, &b, true);
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(app.world().resource::<MeshChanges>().0, vec![2, 1]);
}

#[test]
fn shared_image_without_map_path_replays_only_real_changes_to_dormant_runtime() {
    // 高度/密度源可能来自共享目录，仅存在资源中而不挂在Mesh3d或WorldAssetRoot上。
    #[derive(Resource, Default)]
    struct ImageChanges(Vec<usize>);
    fn capture(
        mut events: feathrise_scene_core::residency::ResidentAssetEventReader<Image>,
        proof: Res<MapRuntimeProof>,
        mut changes: ResMut<ImageChanges>,
    ) {
        for event in events.read() {
            if event.is_modified(proof.image.id()) {
                changes.0.push(proof.serial);
            }
        }
    }
    let mut app = fixture();
    app.init_asset::<Image>()
        .init_resource::<ImageChanges>()
        .add_resident_systems(Update, capture);
    let (a, b) = (id("MAP-shared-image-a"), id("MAP-shared-image-b"));
    enter_first(&mut app, &a);
    let image_a = app.world().resource::<MapRuntimeProof>().image.clone();
    for _ in 0..4 {
        app.update();
    }
    prepare(&mut app, &b);
    adopt(&mut app, &b, false);
    app.world_mut()
        .resource_mut::<Assets<Image>>()
        .get_mut(image_a.id())
        .unwrap()
        .data = Some(vec![73; 4]);
    for _ in 0..4 {
        app.update();
    }
    assert!(app.world().resource::<ImageChanges>().0.is_empty());
    prepare(&mut app, &a);
    adopt(&mut app, &a, true);
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(app.world().resource::<ImageChanges>().0, vec![1]);
    prepare(&mut app, &b);
    adopt(&mut app, &b, true);
    for _ in 0..4 {
        app.update();
    }
    prepare(&mut app, &a);
    adopt(&mut app, &a, true);
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(app.world().resource::<ImageChanges>().0, vec![1]); // 未修改的下一次切回不伪造热重载。
}


#[derive(Resource)]
struct ReplaceLoadingAfterStateEntry(feathrise_scene_core::state::LoadingProgress);

#[test]
fn warm_restore_publishes_only_the_transaction_captured_before_actual_state_entry() {
    use feathrise_scene_core::state::{LoadingProgress, LoadingStage, PendingSceneTransition, SceneStartupGate};
    for replace_during_entry in [false, true] {
        let mut app = fixture();
        let a = id("MAP-completed-resident-a");
        let b = id("MAP-completed-resident-b");
        let original = enter_first(&mut app, &a);
        // 与正式加载器一致：首次真正完成后已经移除启动门，暖恢复不能要求重新创建它。
        app.world_mut().remove_resource::<SceneStartupGate>();
        assert_eq!(prepare(&mut app, &b), ResidentScenePreparation::ColdReady);
        adopt(&mut app, &b, false);
        app.world_mut().remove_resource::<SceneStartupGate>();
        assert_eq!(prepare(&mut app, &a), ResidentScenePreparation::WarmReady);
        let request = PendingSceneTransition::named(ScenePhase::Active, "恢复地图A");
        let progress = LoadingProgress::for_transition(&request);
        let dispatched = progress.transaction();
        app.insert_resource(progress);
        if replace_during_entry {
            // 请求拥有事务身份；模拟真正的新请求，而不是重建同一请求的进度视图。
            let newer_request = PendingSceneTransition::named(ScenePhase::Active, "恢复地图A");
            app.insert_resource(ReplaceLoadingAfterStateEntry(LoadingProgress::for_transition(&newer_request)))
                .add_systems(StateTransition, (|world: &mut World| {
                    if let Some(newer) = world.remove_resource::<ReplaceLoadingAfterStateEntry>() {
                        world.insert_resource(newer.0);
                    }
                }).after(bevy::state::state::StateTransitionSystems::EnterSchedules)
                    .before(systems::finish_scene_transition));
        }
        adopt(&mut app, &a, true);
        assert!(app.world().get::<Disabled>(original).is_none());
        assert!(!app.world().contains_resource::<SceneStartupGate>());
        assert_eq!(app.world().resource::<ConstructionCounts>().entered, 2);
        let progress = app.world().resource::<LoadingProgress>();
        let activity = app.world().resource::<SceneResidencyActivity>();
        if replace_during_entry {
            assert_ne!(progress.transaction(), dispatched);
            assert_ne!(progress.stage, LoadingStage::Complete);
            assert!(!progress.resident_restore_ready(ScenePhase::Active, activity));
        } else {
            assert_eq!(progress.transaction(), dispatched);
            assert_eq!(progress.stage, LoadingStage::Complete);
            assert!(progress.resident_restore_ready(ScenePhase::Active, activity));
        }
    }
}
