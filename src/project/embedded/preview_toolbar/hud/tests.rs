//! 官方Pointer事件驱动HUD抓手观察器，核对DPI、UI缩放、边界与退出，不创建用户窗口。

use super::*;
use bevy::{
    camera::{ComputedCameraValues, RenderTarget, RenderTargetInfo},
    picking::{backend::HitData, pointer::Location},
};

fn fixture(dpi: f32, scale: f32) -> (App, Entity, Entity, Entity, Entity) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(EmbeddedProjectState { mode: Mode::Playing, world_id: 9, revision: 7, transitioning: false })
        .insert_resource(UiScale(scale))
        .init_resource::<ToolbarPointerInput>();
    install(&mut app);
    let window = app.world_mut().spawn((PrimaryWindow, Window::default())).id();
    let camera = app.world_mut().spawn(Camera {
        computed: ComputedCameraValues { target_info: Some(RenderTargetInfo {
            physical_size: UVec2::new((800. * dpi) as u32, (600. * dpi) as u32), scale_factor: dpi,
        }), ..default() }, ..default()
    }).id();
    let root = app.world_mut().spawn((ToolbarRoot, UiTargetCamera(camera),
        Node { position_type: PositionType::Absolute, left: px(12), top: px(12), ..default() },
        ComputedNode { size: Vec2::new(300., 34.) * dpi * scale,
            inverse_scale_factor: 1.0 / (dpi * scale), ..default() })).id();
    let handle = app.world_mut().spawn((ToolbarDragHandle, ToolbarPointer, Button,
        Node::default(), BackgroundColor(Color::NONE), ChildOf(root)))
        .observe(drag_start).observe(drag).observe(drag_end).id();
    (app, window, camera, root, handle)
}

fn location(window: Entity) -> Location {
    Location { target: RenderTarget::default().normalize(Some(window)).unwrap(), position: Vec2::new(20., 20.) }
}

fn begin(app: &mut App, window: Entity, camera: Entity, handle: Entity) {
    app.update();
    app.world_mut().trigger(Pointer::new(PointerId::Mouse, location(window),
        DragStart { button: PointerButton::Primary, hit: HitData::new(camera, 0., None, None) }, handle));
}

fn move_by(app: &mut App, window: Entity, handle: Entity, distance: Vec2) {
    app.world_mut().trigger(Pointer::new(PointerId::Mouse, location(window),
        Drag { button: PointerButton::Primary, distance, delta: distance }, handle));
    app.update();
}

#[test]
fn hud_pointer_drag_uses_window_logical_pixels_and_preserves_camera_viewport() {
    for dpi in [1.0, 1.5, 2.0] {
        for scale in [1.0, 1.25] {
            let (mut app, window, camera, root, handle) = fixture(dpi, scale);
            let viewport = app.world().get::<Camera>(camera).unwrap().logical_viewport_size();
            begin(&mut app, window, camera, handle);
            let origin = app.world().resource::<ToolbarHudState>().position;
            assert!(origin.distance(Vec2::new((800. / scale - 300.) * 0.5, 12.)) < 0.001);
            assert_eq!(app.world().get::<Node>(root).unwrap().left, percent(50));
            move_by(&mut app, window, handle, Vec2::new(120., 80.));
            let expected = origin + Vec2::new(120., 80.) / scale;
            assert_eq!(app.world().resource::<ToolbarHudState>().position, expected);
            let node = app.world().get::<Node>(root).unwrap();
            assert_eq!(node.left, px(expected.x));
            assert_eq!(node.top, px(expected.y));
            assert_eq!(app.world().get::<Camera>(camera).unwrap().logical_viewport_size(), viewport,
                "HUD移动只能改变自己的绝对位置，不能占据或缩小游戏区域");
            app.world_mut().trigger(Pointer::new(PointerId::Mouse, location(window),
                DragEnd { button: PointerButton::Primary, distance: Vec2::new(120., 80.) }, handle));
            let position = app.world().resource::<ToolbarHudState>().position;
            move_by(&mut app, window, handle, Vec2::splat(300.));
            assert_eq!(app.world().resource::<ToolbarHudState>().position, position,
                "官方DragEnd后不能继续移动HUD");
        }
    }
}

#[test]
fn hud_drag_clamps_after_resize_and_stops_on_focus_loss_or_preview_exit() {
    let (mut app, window, camera, root, handle) = fixture(1.5, 1.0);
    begin(&mut app, window, camera, handle);
    move_by(&mut app, window, handle, Vec2::splat(2000.));
    assert_eq!(app.world().resource::<ToolbarHudState>().position, Vec2::new(494., 560.));
    app.world_mut().get_mut::<Camera>(camera).unwrap().computed.target_info.as_mut().unwrap().physical_size = UVec2::new(300, 180);
    app.update();
    assert_eq!(app.world().get::<Node>(root).unwrap().left, px(0), "极窄窗口仍需从左侧保住抓手");
    assert!(app.world().resource::<ToolbarHudState>().position.y >= 0.);
    app.world_mut().get_mut::<Window>(window).unwrap().focused = false;
    app.update();
    assert!(app.world().resource::<ToolbarHudState>().dragging.is_none());
    assert!(!app.world().resource::<ToolbarPointerInput>().capturing);
    app.world_mut().get_mut::<Window>(window).unwrap().focused = true;
    app.world_mut().resource_mut::<EmbeddedProjectState>().mode = Mode::Editing;
    let position = app.world().resource::<ToolbarHudState>().position;
    begin(&mut app, window, camera, handle);
    move_by(&mut app, window, handle, Vec2::splat(100.));
    assert_eq!(app.world().resource::<ToolbarHudState>().position, position);
    assert!(app.world().resource::<ToolbarHudState>().dragging.is_none());
}
