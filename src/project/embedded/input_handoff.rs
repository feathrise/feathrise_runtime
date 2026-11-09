//! 编辑/试玩输入所有权交接；清空完整键鼠与文本输入通道，保留窗口关闭和尺寸等生命周期消息。

use bevy::{
    input::{
        keyboard::{Key, KeyboardInput},
        mouse::{
            AccumulatedMouseMotion, AccumulatedMouseScroll, MouseButtonInput, MouseMotion,
            MouseWheel,
        },
    },
    prelude::*,
    window::{CursorMoved, Ime},
};

pub(super) fn install(app: &mut App) {
    app.add_systems(PreUpdate, clear_suspended
        .after(bevy::input::InputSystems).before(bevy::ui::UiSystems::Focus)
        .before(bevy::picking::PickingSystems::ProcessInput))
        .add_systems(Last, clear_suspended);
}

pub(super) fn clear(world: &mut World) {
    // 固定的输入资源清单；不遍历项目实体或地图。必须使用Messages::clear保留单调序号，
    // 不能换成默认Messages，否则项目现有MessageReader会错过交接后的真正新输入。
    if let Some(mut input) = world.get_resource_mut::<ButtonInput<KeyCode>>() {
        input.reset_all();
    }
    if let Some(mut input) = world.get_resource_mut::<ButtonInput<Key>>() {
        input.reset_all();
    }
    if let Some(mut input) = world.get_resource_mut::<ButtonInput<MouseButton>>() {
        input.reset_all();
    }
    if let Some(mut input) = world.get_resource_mut::<AccumulatedMouseMotion>() {
        input.delta = Vec2::ZERO;
    }
    if let Some(mut input) = world.get_resource_mut::<AccumulatedMouseScroll>() {
        *input = default();
    }
    clear_messages::<KeyboardInput>(world);
    clear_messages::<MouseButtonInput>(world);
    clear_messages::<MouseMotion>(world);
    clear_messages::<MouseWheel>(world);
    clear_messages::<CursorMoved>(world);
    clear_messages::<Ime>(world);
    // 现代拾取在 First 已生成原始指针消息；交接时必须在拾取消费前一并丢弃。
    clear_messages::<bevy::picking::pointer::PointerInput>(world);
}

fn clear_messages<M: bevy::ecs::message::Message>(world: &mut World) {
    if let Some(mut messages) = world.get_resource_mut::<Messages<M>>() {
        if !messages.is_empty() {
            messages.clear();
        }
    }
}

pub(super) fn clear_suspended(world: &mut World) {
    // First已观察失焦/隐藏；在InputSystems汇总新事件之后、UI和控制器之前清掉，
    // 否则Last再清无法撤销同帧Alt切换、原始环视或真实时间相机惯性。
    let suspended = world.get_non_send::<super::resources::Runtime>().is_some_and(|runtime| runtime.input_suspended)
        || world.get_resource::<super::EmbeddedProjectState>().is_some_and(|state| state.transitioning);
    if suspended {
        clear(world);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{EmbeddedProjectState, ProjectGameplay};
    use super::*;
    use bevy::input::ButtonState;
    use feathrise_editor_protocol::Mode;

    fn key(code: KeyCode, state: ButtonState, repeat: bool) -> KeyboardInput {
        KeyboardInput {
            key_code: code,
            logical_key: Key::Character("旧文本".into()),
            state,
            text: Some("旧文本".into()),
            repeat,
            window: Entity::PLACEHOLDER,
        }
    }

    #[test]
    fn opening_gameplay_in_the_same_update_cannot_consume_author_motion_or_text() {
        fn open_gameplay(world: &mut World) {
            super::super::systems::reset_mode_input(world, Mode::Playing);
            world.resource_mut::<EmbeddedProjectState>().mode = Mode::Playing;
        }
        fn game_input(world: &mut World) {
            assert_eq!(world.resource::<AccumulatedMouseMotion>().delta, Vec2::ZERO);
            assert_eq!(world.resource::<AccumulatedMouseScroll>().delta, Vec2::ZERO);
            assert!(
                !world
                    .resource::<ButtonInput<KeyCode>>()
                    .pressed(KeyCode::KeyW)
            );
            assert!(
                world
                    .resource::<ButtonInput<Key>>()
                    .get_pressed()
                    .next()
                    .is_none()
            );
            assert!(
                !world
                    .resource::<ButtonInput<MouseButton>>()
                    .pressed(MouseButton::Left)
            );
            assert!(world.resource::<Messages<KeyboardInput>>().is_empty());
            assert!(world.resource::<Messages<MouseMotion>>().is_empty());
            assert!(world.resource::<Messages<MouseWheel>>().is_empty());
            assert!(world.resource::<Messages<MouseButtonInput>>().is_empty());
            assert!(world.resource::<Messages<Ime>>().is_empty());
            assert_eq!(
                world
                    .resource::<Messages<bevy::window::WindowCloseRequested>>()
                    .len(),
                1
            );
        }
        let mut app = App::new();
        app.add_plugins(bevy::input::InputPlugin)
            .add_message::<Ime>()
            .add_message::<bevy::window::WindowCloseRequested>()
            .insert_resource(EmbeddedProjectState {
                mode: Mode::Editing,
                world_id: 1,
                revision: 1,
                transitioning: false,
            })
            .add_systems(Update, open_gameplay.before(ProjectGameplay))
            .add_systems(Update, game_input.in_set(ProjectGameplay));
        super::super::install(&mut app);
        app.world_mut()
            .write_message(key(KeyCode::KeyW, ButtonState::Pressed, false));
        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Left,
            state: ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
        });
        app.world_mut().write_message(MouseMotion {
            delta: Vec2::new(120., -40.),
        });
        app.world_mut().write_message(MouseWheel {
            unit: bevy::input::mouse::MouseScrollUnit::Line,
            x: 2.,
            y: 3.,
            window: Entity::PLACEHOLDER,
            phase: bevy::input::touch::TouchPhase::Moved,
        });
        app.world_mut().write_message(Ime::Commit {
            window: Entity::PLACEHOLDER,
            value: "上一编辑输入".into(),
        });
        app.world_mut()
            .write_message(bevy::window::WindowCloseRequested {
                window: Entity::PLACEHOLDER,
            });
        app.update();
    }

    #[test]
    fn suspended_input_is_cleared_after_aggregation_before_ui_and_resumes_with_new_edges() {
        #[derive(Resource, Default)]
        struct Probe { alt_edges: u32, motion: Vec2 }
        fn consume(keys: Res<ButtonInput<KeyCode>>, motion: Res<AccumulatedMouseMotion>, mut probe: ResMut<Probe>) {
            if keys.just_pressed(KeyCode::AltLeft) { probe.alt_edges += 1; }
            probe.motion += motion.delta;
        }
        let mut app = App::new();
        app.add_plugins(bevy::input::InputPlugin)
            .add_message::<bevy::window::WindowCloseRequested>()
            .insert_resource(EmbeddedProjectState {
                mode: Mode::Playing, world_id: 1, revision: 1, transitioning: true,
            })
            .init_resource::<Probe>()
            .add_systems(PreUpdate, consume.in_set(bevy::ui::UiSystems::Focus));
        // 使用生产注册顺序；若清理被移回Last，probe会先消费本帧Alt和环视。
        super::install(&mut app);
        app.world_mut().write_message(key(KeyCode::AltLeft, ButtonState::Pressed, false));
        app.world_mut().write_message(MouseMotion { delta: Vec2::new(120., -40.) });
        app.world_mut().write_message(bevy::window::WindowCloseRequested { window: Entity::PLACEHOLDER });
        app.update();
        let probe = app.world().resource::<Probe>();
        assert_eq!(probe.alt_edges, 0);
        assert_eq!(probe.motion, Vec2::ZERO);
        assert_eq!(app.world().resource::<Messages<bevy::window::WindowCloseRequested>>().len(), 1);
        app.world_mut().resource_mut::<EmbeddedProjectState>().transitioning = false;
        app.world_mut().write_message(key(KeyCode::AltLeft, ButtonState::Pressed, false));
        app.world_mut().write_message(MouseMotion { delta: Vec2::new(3., 4.) });
        app.update();
        let probe = app.world().resource::<Probe>();
        assert_eq!(probe.alt_edges, 1);
        assert_eq!(probe.motion, Vec2::new(3., 4.));
    }
}
