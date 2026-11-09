//! 用现代拾取来源和实际指针事件核对三态、冒泡、取消和增量 Changed 合同。

use super::*;
use bevy::{camera::RenderTarget, picking::{backend::HitData, events::Pointer, pointer::Location}, window::WindowRef};

fn fixture() -> (App, Entity, Entity) {
    let mut app = App::new();
    app.init_resource::<HoverMap>()
        .init_resource::<ButtonInput<MouseButton>>()
        .add_plugins(UiInteractionPlugin);
    let window = app.world_mut().spawn(Window::default()).id();
    let target = app.world_mut().spawn((UiInteraction::None, Hovered(true), Node::default(), InheritedVisibility::VISIBLE)).id();
    set_hover(&mut app, window, Some(target));
    (app, window, target)
}

fn set_hover(app: &mut App, window: Entity, target: Option<Entity>) {
    let mut hover = app.world_mut().resource_mut::<HoverMap>();
    hover.clear();
    if let Some(target) = target {
        hover.entry(PointerId::Mouse).or_default().insert(target, HitData::new(window, 0., None, None));
    }
}

fn pointer(window: Entity) -> Pointer {
    Pointer::new(PointerId::Mouse, Location {
        target: RenderTarget::Window(WindowRef::Entity(window)).normalize(None).unwrap(),
        position: Vec2::ZERO,
    })
}

fn down(app: &mut App, window: Entity, target: Entity, button: PointerButton) {
    app.world_mut().trigger(PointerPress { entity: target, pointer: pointer(window), button,
        hit: HitData::new(window, 0., None, None), count: 1 });
}

#[test]
fn modern_sources_preserve_left_only_state_and_removed_pressed_edge() {
    let (mut app, window, target) = fixture();
    app.update();
    assert_eq!(app.world().get::<UiInteraction>(target), Some(&UiInteraction::Hovered));
    down(&mut app, window, target, PointerButton::Secondary);
    app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Right);
    app.update();
    assert!(!app.world().entity(target).contains::<Pressed>());
    assert_eq!(app.world().get::<UiInteraction>(target), Some(&UiInteraction::Hovered));
    down(&mut app, window, target, PointerButton::Primary);
    app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Left);
    app.update();
    assert_eq!(app.world().get::<UiInteraction>(target), Some(&UiInteraction::Pressed));
    app.world_mut().resource_mut::<ButtonInput<MouseButton>>().clear();
    app.world_mut().resource_mut::<ButtonInput<MouseButton>>().release(MouseButton::Left);
    app.world_mut().entity_mut(target).remove::<Pressed>();
    app.update();
    assert_eq!(app.world().get::<UiInteraction>(target), Some(&UiInteraction::Hovered));
    assert!(crate::button_released(app.world().get::<UiInteraction>(target).unwrap(),
        app.world().resource::<ButtonInput<MouseButton>>()));
}

#[test]
fn a_complete_primary_click_in_one_frame_keeps_its_transient_press_edge() {
    let (mut app, _, target) = fixture();
    {
        let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
        mouse.press(MouseButton::Left);
        mouse.release(MouseButton::Left);
    }
    app.update();
    assert_eq!(app.world().get::<UiInteraction>(target), Some(&UiInteraction::Pressed));
    assert!(!app.world().entity(target).contains::<Pressed>());
    assert!(crate::button_released(app.world().get::<UiInteraction>(target).unwrap(),
        app.world().resource::<ButtonInput<MouseButton>>()));
    app.world_mut().resource_mut::<ButtonInput<MouseButton>>().clear();
    app.update();
    assert_eq!(app.world().get::<UiInteraction>(target), Some(&UiInteraction::Hovered));
    assert!(!crate::button_released(app.world().get::<UiInteraction>(target).unwrap(),
        app.world().resource::<ButtonInput<MouseButton>>()));
}

#[test]
fn release_helper_rejects_stale_pressed_and_primary_still_held() {
    let mut mouse = ButtonInput::<MouseButton>::default();
    mouse.press(MouseButton::Left);
    mouse.clear(); // 先前帧的按下只保留电平，本帧没有新的按下边沿。
    mouse.release(MouseButton::Left);
    assert!(!crate::button_released(&UiInteraction::Pressed, &mouse));
    assert!(crate::button_released(&UiInteraction::Hovered, &mouse));

    mouse.clear();
    mouse.press(MouseButton::Left);
    mouse.release(MouseButton::Left);
    mouse.press(MouseButton::Left); // 同帧释放后重新按住，不能由中间释放提交按钮。
    assert!(!crate::button_released(&UiInteraction::Pressed, &mouse));
    assert!(!crate::button_released(&UiInteraction::Hovered, &mouse));
    mouse.release(MouseButton::Left);
    assert!(!crate::button_released(&UiInteraction::None, &mouse));
}

#[test]
fn projected_primary_release_steps_numeric_input_once() {
    use crate::{NumericInputConfig, NumericInputEditState, NumericInputRoot, NumericInputSpec,
        NumericInputStepButton, NumericInputValue, NumericStepDirection, NumericValue};

    let (mut app, _, button) = fixture();
    app.init_resource::<ButtonInput<KeyCode>>().add_plugins(crate::UiWidgetsPlugin);
    // 保留正式数字模型和步进按钮，由生产投影与 numeric_input_system 执行实际加一。
    let initial = NumericValue::U32(10);
    let root = app.world_mut().spawn((NumericInputRoot, NumericInputValue(initial),
        NumericInputConfig(NumericInputSpec::u32()), NumericInputEditState::new(initial, false))).id();
    app.world_mut().entity_mut(button).insert(NumericInputStepButton {
        root, direction: NumericStepDirection::Increase,
    });
    app.update();

    {
        let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
        mouse.press(MouseButton::Left);
        mouse.release(MouseButton::Left);
    }
    app.update();
    assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, NumericValue::U32(11));
    app.world_mut().resource_mut::<ButtonInput<MouseButton>>().clear();
    app.update();
    assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, NumericValue::U32(11));

    {
        let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
        mouse.press(MouseButton::Left);
        mouse.release(MouseButton::Left);
        mouse.press(MouseButton::Left);
    }
    app.update();
    assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, NumericValue::U32(11));
    {
        let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
        mouse.clear();
        mouse.release(MouseButton::Left); // 下一帧真正松开仍走普通 Hovered 步进。
    }
    app.update();
    assert_eq!(app.world().get::<UiInteraction>(button), Some(&UiInteraction::Hovered));
    assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, NumericValue::U32(12));
    app.world_mut().resource_mut::<ButtonInput<MouseButton>>().clear();
    app.update();
    assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, NumericValue::U32(12));
}

#[test]
fn child_hit_resolves_nearest_control_and_does_not_press_ancestor() {
    let (mut app, window, parent) = fixture();
    let button = app.world_mut().spawn((UiInteraction::None, Hovered(true), Node::default(), InheritedVisibility::VISIBLE, ChildOf(parent))).id();
    let text = app.world_mut().spawn(ChildOf(button)).id();
    set_hover(&mut app, window, Some(text));
    down(&mut app, window, text, PointerButton::Primary);
    app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Left);
    app.update();
    assert_eq!(app.world().get::<UiInteraction>(button), Some(&UiInteraction::Pressed));
    assert_eq!(app.world().get::<UiInteraction>(parent), Some(&UiInteraction::None));
    assert!(!app.world().entity(parent).contains::<Pressed>());
    set_hover(&mut app, window, None);
    app.update();
    assert_eq!(app.world().get::<UiInteraction>(button), Some(&UiInteraction::None));
    assert!(!app.world().entity(button).contains::<Pressed>());
}

#[test]
fn cancellation_focus_and_disabled_visibility_do_not_replay_held_press() {
    let (mut app, window, target) = fixture();
    app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Left);
    down(&mut app, window, target, PointerButton::Primary);
    app.update();
    app.world_mut().resource_mut::<ButtonInput<MouseButton>>().clear();
    app.world_mut().trigger(PointerCancel { entity: target, pointer: pointer(window), hit: HitData::new(window, 0., None, None) });
    app.update();
    assert_eq!(app.world().get::<UiInteraction>(target), Some(&UiInteraction::None));
    assert!(!crate::button_released(app.world().get::<UiInteraction>(target).unwrap(),
        app.world().resource::<ButtonInput<MouseButton>>()));
    app.update();
    assert_eq!(app.world().get::<UiInteraction>(target), Some(&UiInteraction::None));
    app.world_mut().resource_mut::<ButtonInput<MouseButton>>().release(MouseButton::Left);
    app.update();
    assert_eq!(app.world().get::<UiInteraction>(target), Some(&UiInteraction::Hovered));
    down(&mut app, window, target, PointerButton::Primary);
    app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Left);
    app.update();
    app.world_mut().get_mut::<Window>(window).unwrap().focused = false;
    app.update();
    assert_eq!(app.world().get::<UiInteraction>(target), Some(&UiInteraction::None));
    app.world_mut().get_mut::<Window>(window).unwrap().focused = true;
    app.update();
    assert_eq!(app.world().get::<UiInteraction>(target), Some(&UiInteraction::None));
    app.world_mut().resource_mut::<ButtonInput<MouseButton>>().release(MouseButton::Left);
    app.update();
    app.world_mut().entity_mut(target).insert(InteractionDisabled);
    app.update();
    assert_eq!(app.world().get::<UiInteraction>(target), Some(&UiInteraction::None));
    assert!(!crate::button_released(app.world().get::<UiInteraction>(target).unwrap(),
        app.world().resource::<ButtonInput<MouseButton>>()));
    app.world_mut().entity_mut(target).remove::<InteractionDisabled>();
    app.world_mut().get_mut::<Node>(target).unwrap().display = Display::None;
    app.update();
    assert_eq!(app.world().get::<UiInteraction>(target), Some(&UiInteraction::None));
    assert!(!crate::button_released(app.world().get::<UiInteraction>(target).unwrap(),
        app.world().resource::<ButtonInput<MouseButton>>()));
}

#[derive(Resource, Default)]
struct Changes(usize);

fn count_changes(query: Query<(), Changed<UiInteraction>>, mut changes: ResMut<Changes>) {
    changes.0 += query.iter().count();
}

#[test]
fn stable_frames_do_not_publish_changed_and_shared_plugin_install_is_idempotent() {
    let (mut app, _, _) = fixture();
    app.add_plugins(crate::UiWidgetsPlugin)
        .init_resource::<Changes>()
        .init_resource::<ButtonInput<KeyCode>>()
        .add_systems(Update, count_changes);
    app.update();
    let changes = app.world().resource::<Changes>().0;
    app.update();
    app.update();
    assert_eq!(app.world().resource::<Changes>().0, changes);
}
