//! 已初始化的玩法相机在Disabled期间错过窗口事件，必须由完整暖采用恢复当前窗口投影。
use super::*;
use bevy::{
    asset::AssetEvent,
    render::{camera::camera_system, texture::ManualTextureViews},
    window::{
        PrimaryWindow, WindowCreated, WindowResized, WindowResolution, WindowScaleFactorChanged,
    },
};

fn assert_view(app: &App, camera: Entity, size: UVec2, aspect: f32) {
    let camera_data = app.world().get::<Camera>(camera).unwrap();
    assert!(
        camera_data.viewport.is_none(),
        "回归必须使用真实Window目标的自动尺寸"
    );
    assert_eq!(camera_data.physical_viewport_size(), Some(size));
    let Projection::Perspective(projection) = app.world().get::<Projection>(camera).unwrap() else {
        panic!("玩法相机应保持透视投影");
    };
    assert!((projection.aspect_ratio - aspect).abs() < 0.00001);
    assert_eq!(projection.fov, 0.9, "恢复不能重置镜头设定");
}

#[test]
fn warm_window_camera_reinitializes_after_disabled_scene_misses_resize_events() {
    let mut app = fixture();
    app.add_message::<WindowCreated>()
        .add_message::<WindowResized>()
        .add_message::<WindowScaleFactorChanged>()
        .add_message::<AssetEvent<Image>>()
        .init_resource::<ManualTextureViews>()
        .add_systems(PostUpdate, camera_system);
    let window = app
        .world_mut()
        .spawn((
            Window {
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ))
        .id();
    let (a, b) = (id("MAP-camera-warm-a"), id("MAP-camera-warm-b"));
    let root = enter_first(&mut app, &a);
    let camera = app.world().get::<Children>(root).unwrap()[0];
    app.world_mut().entity_mut(camera).insert((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 0.9,
            aspect_ratio: 3.0,
            ..default()
        }),
    ));
    app.world_mut().write_message(WindowCreated { window });
    app.update();
    assert_view(&app, camera, UVec2::new(800, 600), 4.0 / 3.0);
    assert_eq!(prepare(&mut app, &b), ResidentScenePreparation::ColdReady);
    adopt(&mut app, &b, false);
    assert!(app.world().get::<Disabled>(camera).is_some());
    assert!(!app.world().get::<Camera>(camera).unwrap().is_active);
    app.world_mut()
        .get_mut::<Window>(window)
        .unwrap()
        .resolution
        .set_physical_resolution(1600, 900);
    app.world_mut().write_message(WindowResized {
        window,
        width: 1600.0,
        height: 900.0,
    });
    for _ in 0..4 {
        app.update();
    } // camera_system实际消费事件；禁用的旧相机错过事件并保留旧Some摘要。
    assert_view(&app, camera, UVec2::new(800, 600), 4.0 / 3.0);
    assert_eq!(prepare(&mut app, &a), ResidentScenePreparation::WarmReady);
    assert!(
        !app.world().get::<Camera>(camera).unwrap().is_active,
        "部分恢复不能提前启用相机"
    );
    assert_view(&app, camera, UVec2::new(800, 600), 4.0 / 3.0);
    adopt(&mut app, &a, true); // 正式finish_scene_transition负责失效，测试不手清computed或改投影。
    assert!(app.world().get::<Camera>(camera).unwrap().is_active);
    assert!(app.world().get::<Disabled>(camera).is_none());
    assert_view(&app, camera, UVec2::new(1600, 900), 16.0 / 9.0);
    assert_eq!(
        app.world().resource::<ConstructionCounts>().entered,
        2,
        "暖恢复复用原相机，不能冷重建"
    );
}
