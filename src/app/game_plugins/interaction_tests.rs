//! 真实公共时间写侧的无窗口调度回归；应用、作者和碰撞来源必须在同帧合并。
use super::*;
use feathrise_scene_core::state::{ScenePhase, SceneResidencyActivity};

#[derive(Resource, Default)]
struct RequestedPause { application: bool, editor: bool, collision: bool }
#[derive(Resource, Default)]
struct ObservedPause(Vec<(bool, bool)>);

fn collect(request: Res<RequestedPause>, mut editor: ResMut<EditorInputState>, mut collision: ResMut<WorldPauseBlockers>) {
    editor.scene_editor_open = request.editor;
    collision.scene_collision_reload = request.collision;
}
fn publish(request: Res<RequestedPause>, mut input: ResMut<InputGateState>) {
    input.time_paused = request.application;
}
fn observe(physics: Res<Time<Physics>>, virtual_time: Res<Time<Virtual>>, mut seen: ResMut<ObservedPause>) {
    seen.0.push((physics.is_paused(), virtual_time.is_paused()));
}
fn app() -> App {
    let mut app = App::new();
    app.insert_resource(State::new(ScenePhase::Active))
        .init_resource::<Time<Physics>>()
        .init_resource::<Time<Virtual>>()
        .init_resource::<RequestedPause>()
        .init_resource::<ObservedPause>()
        .add_plugins(WorldInteractionPlugin)
        .add_systems(Update, collect.in_set(GamePauseSystemSet::CollectBlockers))
        .add_systems(Update, publish.in_set(GamePauseSystemSet::PublishGateFacts))
        .add_systems(Update, observe.after(GamePauseSystemSet::Apply));
    app
}

#[test]
fn same_frame_application_pause_and_resume_reach_both_clocks() {
    let mut app = app();
    for paused in [false, true, true, false] {
        app.world_mut().resource_mut::<RequestedPause>().application = paused;
        app.update();
    }
    assert_eq!(app.world().resource::<ObservedPause>().0,
        [(false, false), (true, true), (true, true), (false, false)]);
}

#[test]
fn clearing_one_source_never_releases_another_owners_pause() {
    let mut app = app();
    for (application, editor, collision) in [
        (true, true, true), (false, true, true), (false, false, true),
        (false, true, false), (false, false, false),
    ] {
        app.insert_resource(RequestedPause { application, editor, collision });
        app.update();
        let paused = application || editor || collision;
        assert_eq!(app.world().resource::<Time<Physics>>().is_paused(), paused);
        assert_eq!(app.world().resource::<Time<Virtual>>().is_paused(), paused);
    }
}

#[test]
fn input_only_capture_does_not_implicitly_pause_time() {
    let mut app = app();
    app.world_mut().resource_mut::<InputGateState>().input_allowed = false;
    app.update();
    assert!(!app.world().resource::<Time<Physics>>().is_paused());
    assert!(!app.world().resource::<Time<Virtual>>().is_paused());
}

#[test]
fn non_gameplay_and_resident_switch_preserve_existing_time_owner_until_return() {
    let mut app = app();
    app.world_mut().resource_mut::<Time<Physics>>().pause();
    app.world_mut().resource_mut::<Time<Virtual>>().pause();
    for scene in [ScenePhase::Idle, ScenePhase::Loading] {
        app.insert_resource(State::new(scene));
        app.update();
        assert!(app.world().resource::<Time<Physics>>().is_paused());
        assert!(app.world().resource::<Time<Virtual>>().is_paused());
    }
    app.insert_resource(State::new(ScenePhase::Active));
    app.insert_resource(SceneResidencyActivity { switching: true, ..default() });
    app.update();
    assert!(app.world().resource::<Time<Physics>>().is_paused());
    assert!(app.world().resource::<Time<Virtual>>().is_paused());
    app.world_mut().resource_mut::<SceneResidencyActivity>().switching = false;
    app.update();
    assert!(!app.world().resource::<Time<Physics>>().is_paused());
    assert!(!app.world().resource::<Time<Virtual>>().is_paused());
}
