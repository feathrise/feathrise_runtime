use super::*; // 纯规则与相机所有权夹具共用父模块私有入口。
use bevy::{camera::RenderTargetInfo, window::WindowRef}; // 构造具备真实物理尺寸的主窗口目标。

#[test]
fn all_persisted_modes_scene_to_fixed_runtime_qualities() {
    assert_eq!(fsr_quality(FsrMode::Off), None);
    assert_eq!(fsr_quality(FsrMode::Quality), Some(Fsr3Quality::Quality));
    assert_eq!(fsr_quality(FsrMode::Balanced), Some(Fsr3Quality::Balanced));
    assert_eq!(
        fsr_quality(FsrMode::Performance),
        Some(Fsr3Quality::Performance)
    );
    assert_eq!(
        fsr_quality(FsrMode::UltraPerformance),
        Some(Fsr3Quality::UltraPerformance)
    );
}

#[test]
fn camera_and_output_filters_keep_unsupported_views_native() {
    let mut camera = Camera::default();
    let perspective = Projection::Perspective(Default::default());
    let orthographic = Projection::Orthographic(OrthographicProjection::default_3d());
    let window = RenderTarget::Window(WindowRef::Primary);
    let none = RenderTarget::None { size: UVec2::ONE };
    assert!(fsr_camera_is_eligible(&camera, &perspective, &window));
    assert!(fsr_output_is_supported(UVec2::splat(2)));
    assert!(!fsr_output_is_supported(UVec2::ZERO));
    assert!(!fsr_output_is_supported(UVec2::ONE));
    camera.viewport = Some(Default::default());
    assert!(!fsr_camera_is_eligible(&camera, &perspective, &window));
    camera.viewport = None;
    assert!(!fsr_camera_is_eligible(&camera, &orthographic, &window));
    assert!(!fsr_camera_is_eligible(&camera, &perspective, &none));
    let invalid_perspective = PerspectiveProjection {
        near: 0.01,
        ..Default::default()
    };
    assert!(!fsr_camera_is_eligible(
        &camera,
        &Projection::Perspective(invalid_perspective),
        &window,
    )); // 不得先降分辨率再让上游因近裁面参数拒绝 dispatch。
}

#[test]
fn opening_and_closing_fsr_releases_lease_and_restores_authored_jitter_and_mip() {
    let mut app = App::new();
    app.init_resource::<RenderSettings>()
        .init_resource::<Fsr3RuntimeStatus>()
        .init_resource::<GraphicsRuntimeLeaseRegistry>()
        .init_resource::<Fsr3LeaseIndex>()
        .insert_resource(Fsr3BackendSupported)
        .insert_resource(Fsr3RuntimeTelemetry::default())
        .add_systems(Update, (maintain_fsr3_upscaling, ApplyDeferred).chain());

    let authored_jitter = Vec2::new(0.25, -0.125);
    let authored_mip = 0.375;
    let mut camera = Camera::default();
    camera.computed.target_info = Some(RenderTargetInfo {
        physical_size: UVec2::new(1920, 1080),
        scale_factor: 1.0,
    });
    let entity = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            camera,
            Projection::Perspective(Default::default()),
            RenderTarget::Window(WindowRef::Primary),
            TemporalJitter {
                offset: authored_jitter,
            },
            MipBias(authored_mip),
        ))
        .id();
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .set_fsr_mode(FsrMode::Quality);
    app.update();

    assert!(app.world().get::<Fsr3Upscaling>(entity).is_some());
    assert!(app.world().get::<Fsr3UpscalingOwnership>(entity).is_some());
    assert!(app.world().get::<Fsr3PostProcessTarget>(entity).is_some());
    let lease = app.world().resource::<Fsr3LeaseIndex>().leases[&entity];
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        1
    );

    app.update(); // 稳定帧不得释放并重新申请租约。
    assert_eq!(
        app.world().resource::<Fsr3LeaseIndex>().leases[&entity],
        lease
    );
    app.world_mut()
        .get_mut::<Fsr3Upscaling>(entity)
        .expect("FSR 组件已接管")
        .reset = true; // 模拟同帧相机切镜已经请求清理时间历史。
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .fsr_sharpening_percent = 25;
    app.update();
    assert!(
        app.world()
            .get::<Fsr3Upscaling>(entity)
            .expect("锐化更新后 FSR 仍存在")
            .reset
    ); // 设置同步不得覆盖历史系统已经提出的 reset。

    app.world_mut()
        .get_mut::<Fsr3Upscaling>(entity)
        .expect("FSR 组件已接管")
        .reset = false; // 模拟 Render Extract 已消费历史重置。
    app.world_mut()
        .get_mut::<Camera>(entity)
        .expect("测试相机存在")
        .computed
        .target_info = Some(RenderTargetInfo {
        physical_size: UVec2::new(2560, 1440),
        scale_factor: 1.0,
    });
    app.update();
    assert_eq!(
        app.world().resource::<Fsr3LeaseIndex>().leases[&entity],
        lease
    );
    assert_eq!(
        app.world()
            .get::<Fsr3UpscalingOwnership>(entity)
            .expect("窗口变化后仍由同一会话接管")
            .output_resolution,
        UVec2::new(2560, 1440)
    );
    assert!(
        app.world()
            .get::<Fsr3Upscaling>(entity)
            .expect("窗口变化后 FSR 仍存在")
            .reset
    ); // 尺寸变化必须请求一次新历史。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .set_fsr_mode(FsrMode::Off);
    app.update();

    assert!(app.world().get::<Fsr3Upscaling>(entity).is_none());
    assert!(app.world().get::<Fsr3UpscalingOwnership>(entity).is_none());
    assert!(app.world().get::<Fsr3PostProcessTarget>(entity).is_none());
    assert_eq!(
        app.world()
            .get::<TemporalJitter>(entity)
            .expect("作者抖动必须恢复")
            .offset,
        authored_jitter
    );
    assert_eq!(
        app.world()
            .get::<MipBias>(entity)
            .expect("作者 MipBias 必须恢复")
            .0,
        authored_mip
    );
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        0
    );
}

#[test]
fn despawned_fsr_camera_releases_orphaned_lease() {
    let mut app = App::new();
    app.init_resource::<RenderSettings>()
        .init_resource::<Fsr3RuntimeStatus>()
        .init_resource::<GraphicsRuntimeLeaseRegistry>()
        .init_resource::<Fsr3LeaseIndex>()
        .insert_resource(Fsr3BackendSupported)
        .insert_resource(Fsr3RuntimeTelemetry::default())
        .add_systems(Update, (maintain_fsr3_upscaling, ApplyDeferred).chain());
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .set_fsr_mode(FsrMode::Performance);
    let mut camera = Camera::default();
    camera.computed.target_info = Some(RenderTargetInfo {
        physical_size: UVec2::new(1920, 1080),
        scale_factor: 1.0,
    });
    let entity = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            camera,
            Projection::Perspective(Default::default()),
            RenderTarget::Window(WindowRef::Primary),
        ))
        .id();
    app.update();
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        1
    );

    app.world_mut().despawn(entity);
    app.update();
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        0
    );
    assert!(app.world().resource::<Fsr3LeaseIndex>().leases.is_empty());
}
