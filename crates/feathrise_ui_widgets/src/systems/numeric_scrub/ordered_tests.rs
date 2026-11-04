//! 真实窗口消息经官方鼠标输入生产者与原控件，不能直接注入业务Changed/Committed冒充手势。

use super::*;
use bevy::{camera::RenderTarget, ecs::message::MessageUpdateSystems,
    input::{InputPlugin, ButtonState, keyboard::{Key, KeyboardInput}, mouse::MouseButtonInput},
    picking::input::mouse_pick_events, ui::UiStack,
    window::{CursorMoved, PrimaryWindow, WindowEvent, WindowRef}};

fn fixture() -> (App, Entity, Entity) {
    let (mut app, root) = feedback_fixture(NumericValue::F32(1.), NumericInputSpec::f32(3), "m");
    app.add_plugins(InputPlugin)
        .add_message::<WindowEvent>()
        .add_systems(First, mouse_pick_events.after(MessageUpdateSystems));
    let window = app.world_mut().spawn((Window::default(), PrimaryWindow)).id();
    let camera = app.world_mut().spawn((Camera::default(), RenderTarget::Window(WindowRef::Entity(window)))).id();
    app.world_mut().entity_mut(root).insert((UiGlobalTransform::from_xy(100., 100.), UiTargetCamera(camera)));
    app.insert_resource(UiStack { partition: vec![0..1], uinodes: vec![root] });
    (app, root, window)
}

fn cursor(app: &mut App, window: Entity, x: f32) {
    let event = CursorMoved { window, position: Vec2::new(x, 100.), delta: None };
    app.world_mut().write_message(WindowEvent::CursorMoved(event));
}

fn button(app: &mut App, window: Entity, state: ButtonState) {
    let event = MouseButtonInput { window, button: MouseButton::Left, state };
    app.world_mut().write_message(event);
    app.world_mut().write_message(WindowEvent::MouseButtonInput(event));
}

fn escape(app: &mut App, window: Entity) {
    app.world_mut().write_message(KeyboardInput { window, key_code: KeyCode::Escape,
        logical_key: Key::Escape, state: ButtonState::Pressed, text: None, repeat: false });
}

#[test]
fn numeric_scrub_ordered_same_frame_press_move_release_commits_once() {
    let (mut app, root, window) = fixture();
    // 整笔发生在一次Update之间，帧末已经离开字段；不能靠Hover或末坐标补造按下点。
    app.world_mut().entity_mut(root).insert((Interaction::None, RelativeCursorPosition::default()));
    cursor(&mut app, window, 100.);
    button(&mut app, window, ButtonState::Pressed);
    cursor(&mut app, window, 200.);
    button(&mut app, window, ButtonState::Released);
    app.update();
    assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, NumericValue::F32(11.));
    let messages = app.world().resource::<FeedbackMessages>();
    assert_eq!(messages.business_changed, 1);
    assert_eq!(messages.business_committed.len(), 1);
    assert_eq!(messages.business_committed[0].outcome, NumericInputCommitOutcome::Accepted);
    assert_eq!(messages.business_committed[0].source, NumericInputCommitSource::Scrub);
    assert!(app.world().resource::<ActiveNumericInputScrub>().entity.is_none());
    app.update();
    assert_eq!(app.world().resource::<FeedbackMessages>().business_committed.len(), 1);
}

#[test]
fn numeric_scrub_ordered_press_outside_cannot_start_from_frame_final_hover() {
    let (mut app, root, window) = fixture();
    cursor(&mut app, window, 200.);
    button(&mut app, window, ButtonState::Pressed);
    cursor(&mut app, window, 100.);
    button(&mut app, window, ButtonState::Released);
    app.update();
    assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, NumericValue::F32(1.));
    assert!(app.world().resource::<FeedbackMessages>().business_committed.is_empty());
}

#[test]
fn numeric_scrub_ordered_escape_restores_once_and_late_release_cannot_commit() {
    let (mut app, root, window) = fixture();
    cursor(&mut app, window, 100.);
    button(&mut app, window, ButtonState::Pressed);
    app.update();
    cursor(&mut app, window, 140.);
    app.update();
    assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, NumericValue::F32(5.));
    escape(&mut app, window);
    app.update();
    assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, NumericValue::F32(1.));
    assert!(app.world().resource::<ActiveNumericInputScrub>().entity.is_none());
    cursor(&mut app, window, 220.);
    app.update(); // Esc后仍按住也不继续改值或重开捕获。
    button(&mut app, window, ButtonState::Released);
    app.update();
    app.update();
    let messages = app.world().resource::<FeedbackMessages>();
    assert_eq!(messages.business_changed, 1);
    assert_eq!(messages.business_committed.len(), 1);
    assert_eq!(messages.business_committed[0].value, NumericValue::F32(1.));
    assert_eq!(messages.business_committed[0].outcome, NumericInputCommitOutcome::Canceled);
    assert_eq!(messages.business_committed[0].source, NumericInputCommitSource::Scrub);
}

#[test]
fn numeric_scrub_ordered_event_overflow_cancels_and_restores_instead_of_losing_release() {
    let (mut app, root, window) = fixture();
    cursor(&mut app, window, 100.);
    button(&mut app, window, ButtonState::Pressed);
    app.update();
    cursor(&mut app, window, 140.);
    app.update();
    for i in 0..257 { cursor(&mut app, window, 140. + i as f32); }
    button(&mut app, window, ButtonState::Released);
    app.update();
    app.update();
    assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, NumericValue::F32(1.));
    assert!(app.world().resource::<ActiveNumericInputScrub>().entity.is_none());
    let messages = app.world().resource::<FeedbackMessages>();
    assert_eq!(messages.business_committed.len(), 1);
    assert_eq!(messages.business_committed[0].outcome, NumericInputCommitOutcome::Canceled);
}

#[test]
fn numeric_scrub_ordered_return_to_start_preserves_drag_intent_without_a_net_edit() {
    let (mut app, _, window) = fixture();
    cursor(&mut app, window, 100.);
    button(&mut app, window, ButtonState::Pressed);
    cursor(&mut app, window, 140.);
    cursor(&mut app, window, 100.);
    button(&mut app, window, ButtonState::Released);
    app.update();
    let messages = app.world().resource::<FeedbackMessages>();
    assert_eq!(messages.business_changed, 0);
    assert_eq!(messages.business_committed.len(), 1);
    assert!(!messages.business_committed[0].changed);
}

#[test]
fn numeric_scrub_ordered_layout_motion_does_not_invent_pointer_distance() {
    for scale in [1., 1.5] {
    let (mut app, root, window) = fixture();
    app.world_mut().get_mut::<Window>(window).unwrap().resolution.set_scale_factor_override(Some(scale));
    let camera = app.world().get::<UiTargetCamera>(root).unwrap().0;
    app.world_mut().get_mut::<Camera>(camera).unwrap().computed.target_info = Some(bevy::camera::RenderTargetInfo {
        physical_size: UVec2::new(1600, 900), scale_factor: scale,
    });
    app.world_mut().entity_mut(root).insert(UiGlobalTransform::from_xy(100., 100. * scale));
    cursor(&mut app, window, 100. / scale);
    button(&mut app, window, ButtonState::Pressed);
    app.update();
    cursor(&mut app, window, 120. / scale);
    app.update();
    assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, NumericValue::F32(3.));
    // 实际布局可随数值文本和面板宽度重排。仅移动半像素就曾让目标3变成2.95；缩放同样不能改手感。
    app.world_mut().entity_mut(root).insert((UiGlobalTransform::from_xy(100.5, 100. * scale),
        ComputedNode { size: Vec2::new(180., 26.), ..default() }));
    app.update(); // 鼠标没有任何新事件，必须保持已采用的3。
    assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, NumericValue::F32(3.));
    app.world_mut().entity_mut(root).insert(UiGlobalTransform::from(
        bevy::math::Affine2::from_scale_angle_translation(Vec2::splat(2.), 0., Vec2::new(110., 100. * scale))));
    app.update();
    assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, NumericValue::F32(3.));
    cursor(&mut app, window, 150. / scale);
    app.update();
    assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, NumericValue::F32(6.));
    button(&mut app, window, ButtonState::Released);
    app.update();
    let messages = app.world().resource::<FeedbackMessages>();
    assert_eq!(messages.business_changed, 2, "仅两次真实鼠标位移可以产生业务变化");
    assert_eq!(messages.business_committed.len(), 1);
    assert_eq!(messages.business_committed[0].value, NumericValue::F32(6.));
    }
}
