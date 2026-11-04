//! 无窗口定向回归：真正按钮处理、显示租约、返回意图和鼠标隔离；不冒充 GPU/实体鼠标验收。

use super::{components::*, resources::*, systems, ui};
use crate::project::embedded::EmbeddedProjectState;
use avian3d::debug_render::PhysicsGizmos;
use bevy::{gizmos::config::{GizmoConfig, GizmoConfigStore}, input::mouse::AccumulatedMouseMotion, prelude::*};
use feathrise_editor_protocol::Mode;
use feathrise_scene::TerrainGridPreview;
use feathrise_scene::author_view::{GridView, ShadingView, PrepassLease};
use feathrise_scene_core::view::PrimarySceneView;

fn fixture() -> App {
    let mut store = GizmoConfigStore::default();
    store.insert(GizmoConfig { enabled: false, ..default() }, PhysicsGizmos::default());
    let mut app = App::new();
    app.insert_resource(EmbeddedProjectState { mode: Mode::Editing, world_id: 91, revision: 7, transitioning: false })
        .init_resource::<PreviewToolbarState>()
        .init_resource::<ToolbarPointerInput>()
        .init_resource::<TerrainGridPreview>()
        .insert_resource(store)
        .add_systems(Startup, ui::spawn)
        .add_systems(Update, (systems::interact, systems::present).chain())
        .add_systems(PostUpdate, systems::synchronize_mesh_wire);
    app.world_mut().spawn((PrimarySceneView, Camera::default()));
    app
}

fn enter(app: &mut App) {
    systems::mode_changed(app.world_mut(), Mode::Playing);
    app.world_mut().resource_mut::<EmbeddedProjectState>().mode = Mode::Playing;
    app.update();
}

fn press(app: &mut App, action: ToolbarAction) {
    let entity = app.world_mut().query::<(Entity, &ToolbarAction)>().iter(app.world())
        .find_map(|(entity, current)| (*current == action).then_some(entity)).unwrap();
    app.world_mut().entity_mut(entity).insert(Interaction::Pressed);
    app.update();
    app.world_mut().entity_mut(entity).insert(Interaction::None);
    app.update();
}

#[test]
fn only_preview_shows_three_real_buttons_and_returns_with_an_intent() {
    let mut app = fixture();
    app.update();
    assert_eq!(app.world_mut().query::<&ToolbarAction>().iter(app.world()).count(), 3);
    assert_eq!(app.world_mut().query_filtered::<&Node, With<ToolbarRoot>>()
        .single(app.world()).unwrap().display, Display::None);
    press(&mut app, ToolbarAction::Return);
    assert!(!systems::take_return_request(app.world_mut()), "隐藏工具条不得申请返回");
    enter(&mut app);
    assert_eq!(app.world_mut().query_filtered::<&Node, With<ToolbarRoot>>()
        .single(app.world()).unwrap().display, Display::Flex);
    press(&mut app, ToolbarAction::Return);
    assert!(systems::take_return_request(app.world_mut()));
    assert!(!systems::take_return_request(app.world_mut()), "一次点击只消费一个意图");
    assert_eq!(app.world().resource::<EmbeddedProjectState>().mode, Mode::Playing,
        "按钮不能伪造 Editing 回执");
    assert_eq!(app.world().resource::<EmbeddedProjectState>().world_id, 91);
}

#[test]
fn toolbar_targets_the_active_game_camera_and_only_uses_fallback_when_it_is_unavailable() {
    let mut app = fixture();
    enter(&mut app);
    let primary = app.world_mut().query_filtered::<Entity, With<PrimarySceneView>>().single(app.world()).unwrap();
    let fallback = app.world_mut().query_filtered::<Entity, With<ToolbarCamera>>().single(app.world()).unwrap();
    let root = app.world_mut().query_filtered::<Entity, With<ToolbarRoot>>().single(app.world()).unwrap();
    assert_eq!(app.world().get::<UiTargetCamera>(root).unwrap().0, primary);
    assert!(!app.world().get::<Camera>(fallback).unwrap().is_active,
        "正常预览只能由主相机合成工具条，独立LDR相机不得覆盖HDR世界");
    app.world_mut().get_mut::<Camera>(primary).unwrap().is_active = false;
    app.update();
    assert_eq!(app.world().get::<UiTargetCamera>(root).unwrap().0, fallback);
    assert!(app.world().get::<Camera>(fallback).unwrap().is_active, "主相机暂不可用时仍可点击返回");
    app.world_mut().get_mut::<Camera>(primary).unwrap().is_active = true;
    app.update();
    assert_eq!(app.world().get::<UiTargetCamera>(root).unwrap().0, primary);
    assert!(!app.world().get::<Camera>(fallback).unwrap().is_active);
}

#[test]
fn debug_buttons_use_production_resources_and_restore_each_preview_baseline() {
    let mut app = fixture();
    app.update();
    let world_id = app.world().id();
    let camera = app.world_mut().query_filtered::<Entity, With<PrimarySceneView>>().single(app.world()).unwrap();
    enter(&mut app);
    press(&mut app, ToolbarAction::Grid);
    press(&mut app, ToolbarAction::Collision);
    assert!(app.world().get::<ShadingView>(camera).copied() == Some(ShadingView::mesh_triangles()));
    assert!(app.world().get::<PrepassLease>(camera).is_some());
    assert!(app.world().get::<GridView>(camera).is_none(), "模型线框开关不得安装无限参考网格");
    assert!(!app.world().resource::<TerrainGridPreview>().visible, "模型网格只租用相机显示，不修改作者地形配置");
    assert!(app.world().resource::<GizmoConfigStore>().config::<PhysicsGizmos>().0.enabled);
    systems::mode_changed(app.world_mut(), Mode::Editing);
    app.world_mut().resource_mut::<EmbeddedProjectState>().mode = Mode::Editing;
    app.update();
    assert!(app.world().get::<GridView>(camera).is_none());
    assert!(app.world().get::<ShadingView>(camera).is_none());
    assert!(app.world().get::<PrepassLease>(camera).is_none());
    assert!(!app.world().resource::<GizmoConfigStore>().config::<PhysicsGizmos>().0.enabled);
    let original = ShadingView { options: Vec4::new(3.0, 1.0, 0.0, 1.0), wire: Vec4::ZERO };
    app.world_mut().entity_mut(camera).insert(original);
    app.world_mut().resource_mut::<GizmoConfigStore>().config_mut::<PhysicsGizmos>().0.enabled = true;
    enter(&mut app);
    press(&mut app, ToolbarAction::Grid);
    press(&mut app, ToolbarAction::Collision);
    assert!(!app.world().resource::<GizmoConfigStore>().config::<PhysicsGizmos>().0.enabled);
    systems::mode_changed(app.world_mut(), Mode::Editing);
    let restored = app.world().get::<ShadingView>(camera).unwrap();
    assert!(restored == &original, "原相机显示组件必须归还");
    assert!(app.world().resource::<GizmoConfigStore>().config::<PhysicsGizmos>().0.enabled,
        "第二次返回恢复自己的进入基线，不能沿用第一次默认值");
    assert_eq!(app.world().id(), world_id);
    assert_eq!(app.world().resource::<EmbeddedProjectState>().revision, 7);
}

#[test]
fn toolbar_mouse_cannot_reach_gameplay_and_f12_has_no_toolbar_action() {
    let mut app = fixture();
    app.init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<AccumulatedMouseMotion>()
        .add_systems(PreUpdate, systems::capture_pointer);
    app.update();
    enter(&mut app);
    let root = app.world_mut().query_filtered::<Entity, With<ToolbarRoot>>().single(app.world()).unwrap();
    app.world_mut().entity_mut(root).insert(Interaction::Hovered);
    app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Left);
    app.world_mut().resource_mut::<AccumulatedMouseMotion>().delta = Vec2::ONE;
    app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::F12);
    app.update();
    assert!(!app.world().resource::<ButtonInput<MouseButton>>().pressed(MouseButton::Left));
    assert_eq!(app.world().resource::<AccumulatedMouseMotion>().delta, Vec2::ZERO);
    assert!(!systems::take_return_request(app.world_mut()));
    assert!(!app.world().resource::<TerrainGridPreview>().visible);
    assert!(!app.world().resource::<GizmoConfigStore>().config::<PhysicsGizmos>().0.enabled);
    assert_eq!(app.world().resource::<EmbeddedProjectState>().mode, Mode::Playing);
}

#[test]
fn official_input_and_ui_support_repeated_toolbar_clicks_without_leaking_a_drag() {
    use bevy::{
        camera::{ComputedCameraValues, RenderTargetInfo, Viewport},
        input::{ButtonState, InputPlugin, mouse::MouseButtonInput},
        window::{ExitCondition, PrimaryWindow, WindowPlugin},
    };
    // 真实InputPlugin/UiPlugin负责边沿、布局、命中和Interaction；不创建OS窗口或RenderApp。
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins, bevy::asset::AssetPlugin::default(), TransformPlugin, InputPlugin,
        WindowPlugin { primary_window: None, exit_condition: ExitCondition::DontExit, ..default() },
        bevy::a11y::AccessibilityPlugin, bevy::image::ImagePlugin::default(), bevy::mesh::MeshPlugin,
        // UiPlugin的图像布局系统即使没有ImageNode也读取图集资产；无SpritePlugin夹具需显式安装。
        bevy::image::TextureAtlasPlugin,
        bevy::camera::CameraPlugin, bevy::text::TextPlugin,
        bevy::input_focus::InputFocusPlugin, bevy::input_focus::InputDispatchPlugin,
        bevy::picking::DefaultPickingPlugins, bevy::ui::UiPlugin,
    ));
    let mut store = GizmoConfigStore::default();
    store.insert(GizmoConfig { enabled: false, ..default() }, PhysicsGizmos::default());
    app.insert_resource(store)
        .insert_resource(EmbeddedProjectState { mode: Mode::Editing, world_id: 91, revision: 7, transitioning: false });
    super::install(&mut app);
    let window = app.world_mut().spawn((PrimaryWindow, Window::default())).id();
    let size = UVec2::new(800, 600);
    app.world_mut().spawn((PrimarySceneView, Camera3d::default(), Camera {
        computed: ComputedCameraValues { target_info: Some(RenderTargetInfo {
            physical_size: size, scale_factor: 1.,
        }), ..default() },
        viewport: Some(Viewport { physical_size: size, ..default() }), ..default()
    })); // 无GPU夹具只提供固定渲染目标尺寸，按钮位置仍来自真实布局。
    app.finish(); app.cleanup();
    enter(&mut app);
    app.update();
    for (action, grid, collision) in [
        (ToolbarAction::Grid, true, false), (ToolbarAction::Collision, true, true),
        (ToolbarAction::Grid, false, true), (ToolbarAction::Collision, false, false),
    ] {
        let (button, center) = app.world_mut().query::<(Entity, &ToolbarAction, &UiGlobalTransform, &ComputedNode)>()
            .iter(app.world()).find_map(|(entity, current, transform, node)| {
                (*current == action && !node.is_empty()).then_some((entity, transform.translation))
            }).unwrap();
        app.world_mut().get_mut::<Window>(window).unwrap().set_cursor_position(Some(center));
        app.world_mut().write_message(MouseButtonInput { button: MouseButton::Left, state: ButtonState::Pressed, window });
        app.update();
        assert_eq!(app.world().get::<Interaction>(button), Some(&Interaction::Pressed));
        let state = app.world().resource::<PreviewToolbarState>();
        assert_eq!((state.grid, state.collision), (grid, collision));
        assert!(!app.world().resource::<ButtonInput<MouseButton>>().pressed(MouseButton::Left));
        app.world_mut().get_mut::<Window>(window).unwrap().set_cursor_position(Some(Vec2::new(700., 300.)));
        app.update(); // 按住拖出UI后，仍不能让游戏接到该次按住。
        assert!(!app.world().resource::<ButtonInput<MouseButton>>().pressed(MouseButton::Left));
        assert!(app.world().resource::<ToolbarPointerInput>().capturing);
        app.world_mut().write_message(MouseButtonInput { button: MouseButton::Left, state: ButtonState::Released, window });
        app.update();
        assert_ne!(app.world().get::<Interaction>(button), Some(&Interaction::Pressed),
            "真正松开必须由UiFocus解除Pressed，第二次点击同一按钮才能再次生效");
        assert!(!app.world().resource::<ToolbarPointerInput>().capturing);
        assert!(app.world().resource::<Messages<MouseButtonInput>>().is_empty());
    }
    systems::mode_changed(app.world_mut(), Mode::Editing);
    assert!(app.world().resource::<ToolbarPointerInput>().saved.is_none());
    assert!(!app.world().resource::<ToolbarPointerInput>().capturing);
    assert_eq!(app.world().resource::<EmbeddedProjectState>().world_id, 91);
}
