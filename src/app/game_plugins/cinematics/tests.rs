//! 无窗口回归使用真实组件及正式呈现插件，验证独立接管、场景归属和倒带回滚。
use super::*;
use bevy::transform::TransformPlugin;
use feathrise_cinematic::Cinematic;
use feathrise_scene::{ScenePlacementInstance, SubsceneActive, SubsceneInstance};
use feathrise_scene_content::{
    ActiveDynamicScene, SceneId, SceneInstanceId, SceneInstancePath, ScenePlacementRecord,
};
use feathrise_scene_core::residency::{SceneInstanceOwner, SceneSourceFrame};
use serde_json::json;
use std::sync::Arc;

pub(super) fn scene() -> SceneId {
    SceneId::parse("MAP-shot").unwrap()
}

pub(super) fn placement(scene: &SceneId, id: &str) -> ScenePlacementInstance {
    let record: ScenePlacementRecord = serde_json::from_value(json!({
        "id":id, "translation":[0,0,0], "yaw_degrees":0, "scale":[1,1,1],
    }))
    .unwrap();
    ScenePlacementInstance::new(scene, &record)
}

pub(super) fn write_frame(
    world: &mut World,
    doc: &Arc<Cinematic>,
    ticket: u64,
    mode: FrameMode,
    time: f32,
) {
    world.resource_mut::<CinematicFrame>().active = Some(FrameData {
        document: doc.clone(),
        ticket,
        mode,
        time,
        instance: Some(scene().to_string()),
    });
}

fn fixture() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, CinematicPresentationPlugin))
        .insert_resource(ActiveDynamicScene {
            scene_id: Some(scene()),
        });
    app
}

#[test]
fn native_free_camera_moves_game_observer_separately_from_the_filmed_pose() {
    use bevy::{
        camera_controller::free_camera::{FreeCamera, FreeCameraPlugin},
        input::InputPlugin,
        time::TimeUpdateStrategy,
        window::WindowPlugin,
    };
    let mut app = fixture();
    app.add_plugins((InputPlugin, WindowPlugin::default(), FreeCameraPlugin))
        .insert_resource(TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_millis(25),
        ));
    let view = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            PrimarySceneView,
            Transform::from_xyz(0., 0., 4.),
            FreeCamera::default(),
        ))
        .id();
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);
    let mut d=Cinematic::from_slice(br#"{"version":1,"id":"free","name":"free","duration":1,
        "control_camera":true,"control_player":false,"camera":[{"time":0,"pose":{"translation":[12,5,30]}}]}"#).unwrap();
    write_frame(
        app.world_mut(),
        &Arc::new(d.clone()),
        30,
        FrameMode::Game,
        0.,
    );
    app.update();
    app.update();
    let moved = free_camera::gameplay_pose(app.world(), view).unwrap();
    assert!(
        moved.translation.z < 3.99,
        "W 应继续移动玩法观察点 {moved:?}"
    );
    assert_eq!(
        app.world().get::<Transform>(view).unwrap().translation,
        Vec3::new(12., 5., 30.)
    );
    d.control_camera = false;
    d.control_player = true;
    write_frame(app.world_mut(), &Arc::new(d), 31, FrameMode::Game, 0.);
    app.update();
    app.update();
    assert_eq!(
        app.world().get::<Transform>(view).unwrap().translation,
        moved.translation,
        "只锁玩家输入后镜头归还玩法姿态"
    );
    app.world_mut().resource_mut::<CinematicFrame>().active = None;
    app.update();
    app.update();
    assert!(
        app.world().get::<Transform>(view).unwrap().translation.z < moved.translation.z,
        "归还后 W 继续移动"
    );
}

#[test]
fn independent_camera_control_rewind_and_missing_target_are_atomic() {
    let mut app = fixture();
    let origin = Transform::from_xyz(2., 1., 6.);
    let camera = app
        .world_mut()
        .spawn((Camera3d::default(), PrimarySceneView, origin))
        .id();
    let actor = app
        .world_mut()
        .spawn((
            Transform::IDENTITY,
            CinematicActor {
                target: "player".into(),
                instance: None,
                model: None,
                player: None,
            },
        ))
        .id();
    let prop = app
        .world_mut()
        .spawn((Transform::IDENTITY, placement(&scene(), "door")))
        .id();
    let mut doc = Cinematic::from_slice(br#"{"version":1,"id":"test","name":"test","duration":2,
        "control_camera":false,"control_player":true,"camera":[{"time":0.5,"pose":{"translation":[2,3,4]}}],
        "actors":[{"target":"player","keys":[{"time":0.5,"pose":{"translation":[3,0,0]}}]}],
        "objects":[{"target":"door","keys":[{"time":0.5,"pose":{"translation":[0,2,0]}}]}]}"#).unwrap();
    let doc_game = Arc::new(doc.clone());
    write_frame(app.world_mut(), &doc_game, 1, FrameMode::Game, 1.);
    app.update();
    assert!(app.world().resource::<CinematicPresentation>().ready(1, 1.));
    assert_eq!(
        *app.world().resource::<CinematicControl>(),
        CinematicControl {
            camera: false,
            player: true
        }
    );
    assert_eq!(*app.world().get::<Transform>(camera).unwrap(), origin);
    assert_eq!(
        app.world().get::<Transform>(actor).unwrap().translation.x,
        3.
    );
    assert_eq!(
        app.world()
            .get::<GlobalTransform>(prop)
            .unwrap()
            .translation()
            .y,
        2.
    );
    write_frame(app.world_mut(), &doc_game, 1, FrameMode::Game, 0.);
    app.update();
    assert_eq!(
        *app.world().get::<Transform>(actor).unwrap(),
        Transform::IDENTITY
    );
    assert_eq!(
        *app.world().get::<Transform>(prop).unwrap(),
        Transform::IDENTITY
    );
    doc.control_camera = true;
    doc.control_player = false;
    doc.objects[0].target = "missing".into();
    let invalid = Arc::new(doc.clone());
    write_frame(app.world_mut(), &invalid, 2, FrameMode::Game, 1.);
    app.update();
    assert!(
        app.world()
            .resource::<CinematicPresentation>()
            .error(2, 1.)
            .unwrap()
            .contains("missing")
    );
    assert_eq!(
        *app.world().get::<Transform>(actor).unwrap(),
        Transform::IDENTITY
    );
    assert_eq!(*app.world().get::<Transform>(camera).unwrap(), origin);
    doc.objects[0].target = "door".into();
    write_frame(app.world_mut(), &Arc::new(doc), 3, FrameMode::Game, 1.);
    app.update();
    assert_eq!(
        app.world().get::<Transform>(camera).unwrap().translation,
        Vec3::new(2., 3., 4.)
    );
    assert_eq!(
        *app.world().resource::<CinematicControl>(),
        CinematicControl {
            camera: true,
            player: false
        }
    );
    app.world_mut().resource_mut::<CinematicFrame>().active = None;
    app.update();
    assert_eq!(
        *app.world().get::<Transform>(actor).unwrap(),
        Transform::IDENTITY
    );
    assert_eq!(*app.world().get::<Transform>(camera).unwrap(), origin);
    assert_eq!(
        *app.world().resource::<CinematicControl>(),
        CinematicControl::default()
    );
}

#[test]
fn only_addressed_child_takes_its_local_pose_and_slot_restore_uses_the_same_instance() {
    let mut app = fixture();
    let source = SceneId::parse("MAP-room").unwrap();
    let mut roots = Vec::new();
    let mut paths = Vec::new();
    let batch = app.world_mut().spawn(SubsceneActive).id();
    for (id, offset) in [("left", 10.), ("right", 20.)] {
        let path = SceneInstancePath::root(scene()).child(SceneInstanceId::parse(id).unwrap());
        paths.push(path.to_string());
        let owner = app
            .world_mut()
            .spawn((
                SubsceneInstance {
                    path,
                    source_scene_id: source.clone(),
                    generation: 1,
                    batch,
                },
                SceneSourceFrame::from_transform(Transform::from_translation(Vec3::X * offset)),
            ))
            .id();
        roots.push(
            app.world_mut()
                .spawn((
                    placement(&source, "gate"),
                    SceneInstanceOwner(owner),
                    Transform::IDENTITY,
                ))
                .id(),
        );
    }
    let doc = Arc::new(serde_json::from_value::<Cinematic>(json!({ "version":1,"id":"gate","name":"gate","duration":1,
        "objects":[{ "target":"gate","instance":paths[1],"keys":[{"time":0,"pose":{"translation":[1,0,0]}}] }]
    })).unwrap());
    write_frame(app.world_mut(), &doc, 5, FrameMode::Export, 1.);
    app.update();
    assert!(app.world().resource::<CinematicPresentation>().ready(5, 1.));
    assert_eq!(
        app.world()
            .get::<Transform>(roots[1])
            .unwrap()
            .translation
            .x,
        21.
    );
    assert_eq!(
        *app.world().get::<Transform>(roots[0]).unwrap(),
        Transform::IDENTITY
    );
    let mut final_frame = app
        .world_mut()
        .resource_mut::<CinematicFrame>()
        .active
        .take()
        .unwrap();
    app.update();
    assert_eq!(
        *app.world().get::<Transform>(roots[1]).unwrap(),
        Transform::IDENTITY
    );
    final_frame.mode = FrameMode::Game;
    app.world_mut()
        .resource_mut::<CinematicWorldState>()
        .completed
        .push(final_frame);
    app.update();
    assert_eq!(
        app.world()
            .get::<Transform>(roots[1])
            .unwrap()
            .translation
            .x,
        21.
    );
    let owner = app.world().get::<SceneInstanceOwner>(roots[1]).unwrap().0;
    app.world_mut()
        .entity_mut(owner)
        .insert(SceneSourceFrame::from_transform(Transform::from_xyz(
            30., 0., 0.,
        )));
    app.update();
    assert_eq!(
        app.world()
            .get::<Transform>(roots[1])
            .unwrap()
            .translation
            .x,
        31.,
        "同一个已结算的实例重新定位后也要更新终态"
    );
    app.world_mut()
        .resource_mut::<CinematicWorldState>()
        .completed
        .clear();
    app.update();
    assert_eq!(
        *app.world().get::<Transform>(roots[1]).unwrap(),
        Transform::IDENTITY
    );
}

#[test]
fn opening_and_loading_a_slot_wait_for_the_addressed_placement_batch() {
    use feathrise_scene::{ScenePlacementRuntimePhase as Phase, ScenePlacementRuntimeStatus};
    use feathrise_scene_core::residency::InstanceResourceTable;
    let mut app = fixture();
    let map = scene();
    let status = |phase| ScenePlacementRuntimeStatus {
        scene_id: Some(map.clone()),
        phase,
        ..default()
    };
    app.insert_resource(status(Phase::Building));
    let doc=Arc::new(Cinematic::from_slice(br#"{"version":1,"id":"late","name":"late","duration":1,"objects":[{"target":"door","keys":[{"time":0,"pose":{"translation":[0,2,0]}}]}]}"#).unwrap());
    write_frame(app.world_mut(), &doc, 8, FrameMode::Export, 0.);
    app.update();
    let receipt = app.world().resource::<CinematicPresentation>();
    assert!(!receipt.ready(8, 0.) && receipt.error(8, 0.).is_none());
    app.insert_resource(status(Phase::Ready));
    app.update();
    assert!(
        app.world()
            .resource::<CinematicPresentation>()
            .error(8, 0.)
            .unwrap()
            .contains("door")
    );

    // 父场景已就绪，存档恢复的目标却来自尚在采用的子实例。只采信它自己的摆放回执。
    let path = SceneInstancePath::root(map.clone()).child(SceneInstanceId::parse("east").unwrap());
    let batch = app.world_mut().spawn_empty().id();
    app.insert_resource(feathrise_scene::SubsceneRuntimeStatus {
        root_scene_id: Some(map.clone()),
        ..default()
    });
    let instance = app
        .world_mut()
        .spawn((
            SubsceneInstance {
                path: path.clone(),
                source_scene_id: map.clone(),
                generation: 1,
                batch,
            },
            SceneSourceFrame::default(),
        ))
        .id();
    let mut statuses = InstanceResourceTable::default();
    statuses.values.insert(instance, status(Phase::Building));
    app.insert_resource(statuses);
    app.world_mut()
        .resource_mut::<CinematicFrame>()
        .active
        .as_mut()
        .unwrap()
        .instance = Some(path.to_string());
    app.update();
    let receipt = app.world().resource::<CinematicPresentation>();
    assert!(!receipt.ready(8, 0.) && receipt.error(8, 0.).is_none());
    let mut saved = app
        .world_mut()
        .resource_mut::<CinematicFrame>()
        .active
        .take()
        .unwrap();
    saved.time = 1.;
    saved.mode = FrameMode::Game;
    app.world_mut()
        .resource_mut::<CinematicWorldState>()
        .completed
        .push(saved);
    app.update();
    app.world_mut().entity_mut(batch).insert(SubsceneActive);
    app.world_mut()
        .resource_mut::<feathrise_scene::SubsceneRuntimeStatus>()
        .ready = true;
    app.update(); // 外层子实例已采用，内层摆放仍在 Building，读档继续等待。
    let prop = app
        .world_mut()
        .spawn((
            placement(&map, "door"),
            SceneInstanceOwner(instance),
            Transform::IDENTITY,
        ))
        .id();
    app.world_mut()
        .resource_mut::<InstanceResourceTable<ScenePlacementRuntimeStatus>>()
        .values
        .insert(instance, status(Phase::Ready));
    app.update();
    assert_eq!(
        app.world().get::<Transform>(prop).unwrap().translation.y,
        2.
    );
    app.world_mut().entity_mut(batch).remove::<SubsceneActive>();
    write_frame(app.world_mut(), &doc, 9, FrameMode::Preview, 0.);
    app.world_mut()
        .resource_mut::<CinematicFrame>()
        .active
        .as_mut()
        .unwrap()
        .instance = Some(path.to_string());
    app.update();
    assert!(
        app.world()
            .resource::<CinematicPresentation>()
            .error(9, 0.)
            .unwrap()
            .contains("east"),
        "已撤回批次中的旧实体不可重用"
    );
}

#[test]
fn parked_players_and_other_root_maps_never_supply_an_actor() {
    use bevy::ecs::entity_disabling::Disabled;
    let mut app = fixture();
    let actor = |name: &str, address: Option<&str>| CinematicActor {
        target: name.into(),
        instance: address.map(str::to_owned),
        model: None,
        player: None,
    };
    let parked = app
        .world_mut()
        .spawn((actor("player", None), Disabled, Transform::IDENTITY))
        .id();
    let active = app
        .world_mut()
        .spawn((actor("player", None), Transform::IDENTITY))
        .id();
    let elsewhere = app
        .world_mut()
        .spawn((actor("guard#0", Some("MAP-away")), Transform::IDENTITY))
        .id();
    let guard = app
        .world_mut()
        .spawn((
            actor("guard#0", Some(scene().as_str())),
            Transform::IDENTITY,
        ))
        .id();
    let doc = Arc::new(
        serde_json::from_value::<Cinematic>(
            json!({"version":1,"id":"guard","name":"guard","duration":1,
        "actors":[{"target":"player","keys":[{"time":0,"pose":{"translation":[0,2,0]}}]},
            {"target":"guard#0","keys":[{"time":0,"pose":{"translation":[0,3,0]}}]}]}),
        )
        .unwrap(),
    );
    write_frame(app.world_mut(), &doc, 10, FrameMode::Game, 0.);
    app.update();
    assert!(
        app.world()
            .resource::<CinematicPresentation>()
            .ready(10, 0.)
    );
    for (entity, height) in [(parked, 0.), (active, 2.), (elsewhere, 0.), (guard, 3.)] {
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation.y,
            height
        );
    }
}
