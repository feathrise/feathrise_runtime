//! 常驻模式合同回归：暂停真实项目系统，来源读者持有完整版本，取消不会半读新旧资产。

use super::{
    EmbeddedProjectState, ProjectGameplay,
    asset_overlay::{AssetOverlay, Files, Publication, SourceEvidence, valid_path},
};
use bevy::{asset::io::Reader, prelude::*, tasks::futures_lite::future::block_on};
use feathrise_editor_protocol::Mode;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, atomic::Ordering},
};

#[derive(Component)]
struct ProjectCounter(u64);
fn advance(mut counters: Query<&mut ProjectCounter>) {
    for mut counter in &mut counters {
        counter.0 += 1;
    }
}

#[test]
fn editing_and_playing_keep_the_same_entity_and_only_advance_project_systems_in_play() {
    let mut app = App::new();
    app.insert_resource(EmbeddedProjectState {
        mode: Mode::Editing,
        world_id: 123,
        revision: 7,
        transitioning: false,
    })
    .add_systems(Update, advance.in_set(ProjectGameplay))
    .add_systems(FixedUpdate, advance.in_set(ProjectGameplay));
    super::install(&mut app);
    let entity = app.world_mut().spawn(ProjectCounter(9)).id();
    let world_id = app.world().id();
    app.update();
    app.world_mut().run_schedule(FixedUpdate);
    assert_eq!(app.world().get::<ProjectCounter>(entity).unwrap().0, 9);
    app.world_mut().resource_mut::<EmbeddedProjectState>().mode = Mode::Playing;
    app.update();
    app.update();
    app.world_mut().run_schedule(FixedUpdate);
    assert_eq!(app.world().get::<ProjectCounter>(entity).unwrap().0, 12);
    app.world_mut().resource_mut::<EmbeddedProjectState>().mode = Mode::Editing;
    app.update();
    app.world_mut().run_schedule(FixedUpdate);
    assert_eq!(app.world().id(), world_id);
    assert_eq!(app.world().get::<ProjectCounter>(entity).unwrap().0, 12);
    assert_eq!(app.world().resource::<EmbeddedProjectState>().revision, 7);
}

#[test]
fn normal_game_without_embedded_state_keeps_project_systems_enabled() {
    let mut app = App::new();
    app.add_systems(Update, advance.in_set(ProjectGameplay));
    super::install(&mut app);
    let entity = app.world_mut().spawn(ProjectCounter(0)).id();
    app.update();
    assert_eq!(app.world().get::<ProjectCounter>(entity).unwrap().0, 1);
}

#[test]
fn stop_is_terminal_even_when_a_native_mode_result_is_ready_in_the_same_frame() {
    use super::{resources::PendingMode, transport::Inbound};
    use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow, WindowCloseRequested};
    use feathrise_editor_protocol::Message;
    let (mut runtime, input, output) = super::transport::test_runtime();
    runtime.pending_mode = Some(PendingMode { request_id: 7, mode: Mode::Playing, revision: 1,
        started: std::time::Instant::now(), native_started: true });
    runtime.native_result = Some((7, Ok(())));
    input.try_send(Inbound::Control(Message::Stop)).unwrap();
    let mut app = App::new();
    app.add_plugins(bevy::input::InputPlugin)
        .add_message::<WindowCloseRequested>()
        .insert_resource(EmbeddedProjectState { mode: Mode::Playing, world_id: 1, revision: 1, transitioning: false })
        .insert_non_send(runtime);
    let primary = app.world_mut().spawn((Window::default(), PrimaryWindow,
        CursorOptions { grab_mode: CursorGrabMode::Locked, visible: false, ..default() })).id();
    let world_id = app.world().id();
    app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::KeyW);
    app.world_mut().write_message(WindowCloseRequested { window: primary });
    super::systems::drive(app.world_mut());
    assert!(matches!(output.try_recv().unwrap().message, Message::Stopped));
    assert!(output.try_recv().is_err(), "Stop后不能再发布旧ModeApplied或Status");
    assert!(app.world().non_send::<super::resources::Runtime>().stopping);
    assert!(app.world().resource::<EmbeddedProjectState>().transitioning);
    assert_eq!(app.world().resource::<EmbeddedProjectState>().mode, Mode::Playing);
    assert_eq!(app.world().resource::<Messages<AppExit>>().len(), 1);
    assert_eq!(app.world().resource::<Messages<WindowCloseRequested>>().len(), 1);
    assert!(!app.world().resource::<ButtonInput<KeyCode>>().pressed(KeyCode::KeyW));
    assert_eq!(app.world().get::<CursorOptions>(primary).unwrap().grab_mode, CursorGrabMode::None);
    assert_eq!(app.world().id(), world_id);
    super::systems::drive(app.world_mut());
    assert!(output.try_recv().is_err(), "已停止的drive不得重复应答");
    assert_eq!(app.world().resource::<Messages<AppExit>>().len(), 1);
}

#[test]
fn direct_public_helper_pauses_a_handoff_without_changing_the_confirmed_mode() {
    let mut app = App::new();
    app.insert_resource(EmbeddedProjectState {
        mode: Mode::Playing, world_id: 123, revision: 7, transitioning: false,
    })
        // 不加入ProjectGameplay集合，证明合法的直接helper使用者同样受转场许可约束。
        .add_systems(Update, advance.run_if(super::project_simulation_active))
        .add_systems(FixedUpdate, advance.run_if(super::project_simulation_active));
    let entity = app.world_mut().spawn(ProjectCounter(0)).id();
    app.update();
    app.world_mut().resource_mut::<EmbeddedProjectState>().transitioning = true;
    app.update();
    app.world_mut().run_schedule(FixedUpdate);
    assert_eq!(app.world().get::<ProjectCounter>(entity).unwrap().0, 1);
    assert_eq!(app.world().resource::<EmbeddedProjectState>().mode, Mode::Playing);
    app.world_mut().resource_mut::<EmbeddedProjectState>().transitioning = false;
    app.update();
    assert_eq!(app.world().get::<ProjectCounter>(entity).unwrap().0, 2);
}

#[test]
fn only_a_genuinely_empty_initial_world_can_bypass_scene_readiness() {
    use feathrise_scene_content::ActiveDynamicScene;
    use feathrise_scene_core::state::{PendingSceneTransition, ScenePhase, SceneStartupGate};
    let mut world = World::new();
    world.insert_resource(EmbeddedProjectState {
        mode: Mode::Editing,
        world_id: 123,
        revision: 0,
        transitioning: false,
    });
    world.insert_resource(ActiveDynamicScene::default());
    world.insert_resource(State::new(ScenePhase::Idle));
    assert!(super::systems::scene_ready(&world));
    assert!(
        world.resource::<ActiveDynamicScene>().scene_id.is_none(),
        "不得为预览偷偷补默认场景"
    );
    world.insert_resource(PendingSceneTransition::named(
        ScenePhase::Active,
        "尚未采用场景",
    ));
    assert!(!super::systems::scene_ready(&world));
    world.remove_resource::<PendingSceneTransition>();
    world.insert_resource(SceneStartupGate::waiting_for_collision(ScenePhase::Active));
    assert!(!super::systems::scene_ready(&world));
    world.remove_resource::<SceneStartupGate>();
    world.insert_resource(State::new(ScenePhase::Loading));
    assert!(!super::systems::scene_ready(&world));
    world.insert_resource(State::new(ScenePhase::Idle));
    world.resource_mut::<EmbeddedProjectState>().revision = 1;
    assert!(
        !super::systems::scene_ready(&world),
        "非初始缺图不能当成空工程放行"
    );
}

#[test]
fn returning_to_edit_releases_the_game_cursor_and_clears_pressed_inputs() {
    use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
    let mut world = World::new();
    let mut keys = ButtonInput::<KeyCode>::default();
    keys.press(KeyCode::KeyW);
    let mut buttons = ButtonInput::<MouseButton>::default();
    buttons.press(MouseButton::Left);
    world.insert_resource(keys);
    world.insert_resource(buttons);
    let cursor = world
        .spawn((
            PrimaryWindow,
            CursorOptions {
                grab_mode: CursorGrabMode::Locked,
                visible: false,
                ..default()
            },
        ))
        .id();
    super::systems::reset_mode_input(&mut world, Mode::Editing);
    let cursor = world.get::<CursorOptions>(cursor).unwrap();
    assert_eq!(cursor.grab_mode, CursorGrabMode::None);
    assert!(cursor.visible);
    assert!(
        !world
            .resource::<ButtonInput<KeyCode>>()
            .pressed(KeyCode::KeyW)
    );
    assert!(
        !world
            .resource::<ButtonInput<MouseButton>>()
            .pressed(MouseButton::Left)
    );
}

#[test]
fn the_first_gameplay_update_observes_the_restored_game_camera() {
    use feathrise_scene_core::view::PrimarySceneView;
    #[derive(Resource)]
    struct SavedCamera(Entity, Transform, Projection);
    fn resume(world: &mut World) {
        let saved = world.remove_resource::<SavedCamera>().unwrap();
        super::systems::restore_game_camera(world, Some(&(saved.0, saved.1, saved.2)));
        world.resource_mut::<EmbeddedProjectState>().mode = Mode::Playing;
    }
    fn gameplay(mut cameras: Query<(&mut Transform, &mut Projection), With<PrimarySceneView>>) {
        let (mut transform, mut projection) = cameras.single_mut().unwrap();
        assert_eq!(transform.translation, Vec3::new(4., 5., 6.));
        let Projection::Perspective(perspective) = &mut *projection else {
            panic!("首帧玩法不能继续使用作者正交投影");
        };
        assert_eq!(perspective.fov, 0.8);
        transform.translation.x += 2.;
        perspective.fov = 1.1;
    }
    let mut app = App::new();
    app.insert_resource(EmbeddedProjectState { mode: Mode::Editing, world_id: 7, revision: 3, transitioning: false });
    let entity = app.world_mut().spawn((PrimarySceneView,
        Transform::from_xyz(-10., 20., 30.),
        Projection::Orthographic(OrthographicProjection::default_3d()),
    )).id();
    app.insert_resource(SavedCamera(entity, Transform::from_xyz(4., 5., 6.),
        Projection::Perspective(PerspectiveProjection { fov: 0.8, ..default() })))
        .add_systems(Update, resume.before(ProjectGameplay))
        .add_systems(Update, gameplay.in_set(ProjectGameplay));
    super::install(&mut app);
    app.update();
    assert_eq!(app.world().get::<Transform>(entity).unwrap().translation.x, 6.);
    let Projection::Perspective(projection) = app.world().get::<Projection>(entity).unwrap() else {
        panic!("玩法投影必须保留");
    };
    assert_eq!(projection.fov, 1.1);
}

#[test]
fn restoring_a_removed_game_camera_does_not_overwrite_the_new_scene_camera() {
    use feathrise_scene_core::view::PrimarySceneView;
    let mut world = World::new();
    let old = world.spawn((PrimarySceneView, Transform::IDENTITY,
        Projection::Perspective(PerspectiveProjection::default()))).id();
    let saved = (old, Transform::from_xyz(5., 6., 7.),
        Projection::Perspective(PerspectiveProjection::default()));
    world.despawn(old);
    let current = world.spawn((PrimarySceneView, Transform::from_xyz(10., 20., 30.),
        Projection::Orthographic(OrthographicProjection::default_3d()))).id();
    super::systems::restore_game_camera(&mut world, Some(&saved));
    assert_eq!(world.get::<Transform>(current).unwrap().translation, Vec3::new(10., 20., 30.));
    assert!(matches!(world.get::<Projection>(current), Some(Projection::Orthographic(_))));
}

#[test]
fn live_height_handback_preserves_shared_pages_and_rejects_old_scene_results() {
    use feathrise_scene_content::{ActiveDynamicScene, SceneId, SparseHeightLiveOverride,
        SparseHeightSpec, SparseHeightTile, SparseHeightTileKey};
    let map = SceneId::parse("MAP-height-source").unwrap();
    let other = SceneId::parse("MAP-height-next").unwrap();
    let mut world = World::new();
    let mut active = ActiveDynamicScene::default(); active.select(map.clone());
    world.insert_resource(active);
    let spec = SparseHeightSpec::new([-512.0;2], [512.0;2], 0.125, -8.0, 16.0).unwrap();
    let key = SparseHeightTileKey { x:3, z:5 };
    let tile = Arc::new(SparseHeightTile::blank(&spec, key).unwrap());
    let initial = super::systems::next_height_source_revision().unwrap();
    let mut source = SparseHeightLiveOverride::default(); source.retarget(Some(map.clone()),initial);
    source.publish_tiles(&map,initial,[tile.clone()]).unwrap();
    world.insert_resource(source);
    let mut prepared = super::systems::height_preview_source(&world,&map,None).unwrap();
    let live = super::systems::next_height_source_revision().unwrap();
    prepared.retarget(Some(map.clone()), live);
    let previous = super::systems::publish_height_preview(&mut world,&map,None,prepared.clone()).unwrap().unwrap();
    assert_eq!(previous.revision(),initial);
    assert!(Arc::ptr_eq(world.resource::<SparseHeightLiveOverride>().tile(key).unwrap(), &tile));
    assert!(super::systems::publish_height_preview(&mut world,&map,None,previous).is_err());
    let complete = super::systems::next_height_source_revision().unwrap();
    assert!(complete > live, "完整来源不能与上一个实时页版本碰撞");
    prepared.retarget(Some(map.clone()), complete);
    world.resource_mut::<ActiveDynamicScene>().select(other);
    assert!(super::systems::publish_height_preview(&mut world,&map,None,prepared).is_err());
    assert_eq!(world.resource::<SparseHeightLiveOverride>().revision(),live);
}

#[test]
fn an_asset_reader_never_combines_bytes_from_two_publications() {
    let store = AssetOverlay::default();
    let path = "scenes/TEST/map.json".to_owned();
    let publish = |bytes: &[u8]| Publication {
        files: Arc::new(Files::from([(path.clone(), Arc::new(bytes.to_vec()))])),
        reads: Arc::new(BTreeMap::from([(path.clone(), SourceEvidence::default())])),
    };
    let first = publish(b"first-complete");
    let evidence = first.reads.clone();
    assert!(store.publish(first).is_ok());
    assert!(!evidence[&path].read.load(Ordering::Acquire));
    let mut reader = store.read(Path::new(&path)).unwrap();
    assert!(evidence[&path].read.load(Ordering::Acquire));
    assert!(store.publish(publish(b"second-complete")).is_ok());
    block_on(async {
        let mut old = Vec::new();
        reader.read_to_end(&mut old).await.unwrap();
        assert_eq!(old, b"first-complete");
        let mut new = Vec::new();
        store
            .read(Path::new(&path))
            .unwrap()
            .read_to_end(&mut new)
            .await
            .unwrap();
        assert_eq!(new, b"second-complete");
    });
}

#[test]
fn reading_a_new_source_does_not_mean_production_assets_adopted_it() {
    let source = SourceEvidence::default();
    super::source_readiness::record_applied(&source);
    assert!(
        !super::source_readiness::adopted(&source),
        "旧资产事件不能采用未读取的新正文"
    );
    source.read.store(true, Ordering::Release);
    assert!(
        !super::source_readiness::adopted(&source),
        "打开新正文不能复用旧 Loaded 状态"
    );
    super::source_readiness::record_applied(&source);
    assert!(super::source_readiness::adopted(&source));
    assert!(
        !super::source_readiness::adopted(&SourceEvidence::default()),
        "下一版本必须重新取证"
    );
}

#[test]
fn removed_draft_assets_reload_but_cannot_block_the_new_scene_forever() {
    let unchanged = Arc::new(vec![1]);
    let previous = Files::from([
        ("same.json".into(), unchanged.clone()),
        ("old-unsaved.glb".into(), Arc::new(vec![2])),
    ]);
    let current = Files::from([
        ("same.json".into(), unchanged),
        ("new.glb".into(), Arc::new(vec![3])),
    ]);
    let (ready, reload) = super::snapshot::changed_paths(&current, &previous);
    assert_eq!(ready, vec!["new.glb"]);
    assert_eq!(reload, vec!["new.glb", "old-unsaved.glb"]);
}

#[test]
fn asset_publication_yields_to_a_reader_and_rejects_path_escapes() {
    let store = AssetOverlay::default();
    let lock = store.0.lock().unwrap();
    assert!(store.publish(Publication::default()).is_err());
    drop(lock);
    assert!(store.publish(Publication::default()).is_ok());
    for path in [
        "../other/map.json",
        "C:/other/a.glb",
        "scenes/./x",
        "scenes//x",
        "root1://x",
        "scenes/x.glb#Scene0",
        "scenes\\x",
    ] {
        assert!(!valid_path(path), "{path}");
    }
    assert!(valid_path("scenes/当前图/terrain.glb"));
}
