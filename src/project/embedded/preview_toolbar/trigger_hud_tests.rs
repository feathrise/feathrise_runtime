//! 接上实际试走工具条与 Bevy UI 布局，分别验证触发消息的显示寿命和普通工程的装配边界。

use super::{components::ToolbarRoot, mode_changed, ui::MotionTriggerLabel};
use crate::project::embedded::{self, EmbeddedProjectState};
use bevy::{camera::{ComputedCameraValues, RenderTargetInfo, Viewport}, prelude::*, window::{ExitCondition, PrimaryWindow, WindowPlugin}};
use feathrise_editor_protocol::Mode;
use feathrise_scene::{SceneMotionAction, SceneMotionTriggerHit, SceneMotionTriggerTrace};
use feathrise_scene_core::view::PrimarySceneView;

fn mode(app: &mut App, mode: Mode) {
    app.world_mut().resource_mut::<EmbeddedProjectState>().mode = mode;
    mode_changed(app.world_mut(), mode);
    app.update();
}

fn assert_label(app: &mut App, show_toolbar: bool, caption: Option<&str>) {
    let world = app.world_mut();
    let (root, toolbar) = world.query_filtered::<(Entity, &Node), With<ToolbarRoot>>().single(world).unwrap();
    assert_eq!(toolbar.display == Display::Flex, show_toolbar);
    let (parent, node, text, layout) = world.query_filtered::<(&ChildOf, &Node, &Text, &ComputedNode), With<MotionTriggerLabel>>().single(world).unwrap();
    assert_eq!(parent.parent(), root);
    assert_eq!(node.display == Display::Flex, caption.is_some());
    if let Some(caption) = caption {
        assert_eq!(text.as_str(), caption);
        assert!(layout.size().min_element() > 0.0, "实际提示必须占有可显示的布局");
    }
}

#[test]
fn motion_trigger_hud_shows_enter_leave_actions_and_clears_on_next_preview() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default(), TransformPlugin, bevy::input::InputPlugin,
        WindowPlugin { primary_window: None, exit_condition: ExitCondition::DontExit, ..default() },
        bevy::a11y::AccessibilityPlugin, bevy::image::ImagePlugin::default(), bevy::mesh::MeshPlugin,
        bevy::image::TextureAtlasPlugin, bevy::camera::CameraPlugin, bevy::text::TextPlugin,
        bevy::input_focus::InputFocusPlugin, bevy::input_focus::InputDispatchPlugin, bevy::picking::DefaultPickingPlugins, bevy::ui::UiPlugin));
    app.insert_resource(EmbeddedProjectState { mode: Mode::Editing, world_id: 91, revision: 7, transitioning: false });
    super::install(&mut app);
    app.world_mut().spawn((PrimaryWindow, Window::default()));
    let size = UVec2::new(800, 600);
    app.world_mut().spawn((PrimarySceneView, Camera3d::default(), Camera {
        computed: ComputedCameraValues { target_info: Some(RenderTargetInfo { physical_size: size, scale_factor: 1.0 }), ..default() },
        viewport: Some(Viewport { physical_size: size, ..default() }), ..default()
    }));
    app.finish(); app.cleanup(); app.update();
    assert_label(&mut app, false, None);
    mode(&mut app, Mode::Playing);
    assert_label(&mut app, true, None);
    for (entering, action, expected) in [(true, SceneMotionAction::Play, "进入 · 山门 · 正向播放"),
        (false, SceneMotionAction::Reverse, "离开 · 山门 · 反向播放")] {
        // 输入源是场景实际公开的触发回执；工具条自行生成文字、打开节点并完成布局。
        app.world_mut().resource_mut::<SceneMotionTriggerTrace>().last = Some(SceneMotionTriggerHit { clip_name: "山门".into(), entering, action });
        app.update();
        assert_label(&mut app, true, Some(expected));
        mode(&mut app, Mode::Playing);
        assert_label(&mut app, true, Some(expected)); // 当前试走中的重复回执不能擦掉最近一次触发。
    }
    mode(&mut app, Mode::Editing);
    assert!(app.world().resource::<SceneMotionTriggerTrace>().last.is_none());
    assert_label(&mut app, false, None);
    mode(&mut app, Mode::Playing);
    assert_label(&mut app, true, None);
}

#[test]
fn motion_trigger_hud_is_absent_from_ordinary_game_installation() {
    let mut app = App::new();
    // 正式游戏装配也经过这个入口。缺少嵌入握手时只安装玩法集合，不安装 HUD 与诊断资源。
    embedded::install(&mut app);
    app.update();
    let world = app.world_mut();
    assert!(!world.contains_resource::<SceneMotionTriggerTrace>());
    assert!(world.query_filtered::<Entity, Or<(With<ToolbarRoot>, With<MotionTriggerLabel>)>>().iter(world).next().is_none());
}
