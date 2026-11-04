//! 原窗口抗锯齿系统必须尊重作者要求，归还时采用玩家最新值且不泄漏法线附件。
use super::*;

#[test]
fn author_prepass_lease_survives_window_msaa_and_restores_latest_settings() {
    let mut app = App::new();
    app.init_resource::<RenderSettings>()
        .init_resource::<GraphicsRuntimeLeaseRegistry>()
        .add_systems(
            Update,
            (
                restore_released_graphics_runtime_leases,
                ApplyDeferred,
                apply_antialiasing,
                ApplyDeferred,
                apply_graphics_runtime_leases,
                ApplyDeferred,
            )
                .chain(),
        );
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .anti_aliasing = AntiAliasingMode::Msaa4;
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            RenderTarget::Window(WindowRef::Primary),
            Msaa::Sample4,
        ))
        .id();
    let other = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            RenderTarget::None { size: UVec2::ONE },
            Msaa::Sample2,
            NormalPrepass,
        ))
        .id();
    let mut request = GraphicsRuntimeRequirements::new(camera);
    request.force_single_sample = true;
    request.require_depth_prepass = true;
    request.require_normal_prepass = true;
    for desired_msaa in [
        AntiAliasingMode::Msaa4,
        AntiAliasingMode::Msaa8,
        AntiAliasingMode::Msaa4,
    ] {
        let ticket = app
            .world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .acquire(request);
        // 连续帧和租约期间改玩家档位，都不能把预通道改回多采样。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .anti_aliasing = desired_msaa;
        for _ in 0..3 {
            app.update();
            assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Off));
            assert!(app.world().get::<DepthPrepass>(camera).is_some());
            assert!(app.world().get::<NormalPrepass>(camera).is_some());
            assert_eq!(app.world().get::<Msaa>(other), Some(&Msaa::Sample2));
        }
        app.world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .release(ticket);
        app.update();
        let expected = if desired_msaa == AntiAliasingMode::Msaa8 {
            Msaa::Sample8
        } else {
            Msaa::Sample4
        };
        assert_eq!(app.world().get::<Msaa>(camera), Some(&expected));
        assert!(app.world().get::<DepthPrepass>(camera).is_none());
        assert!(app.world().get::<NormalPrepass>(camera).is_none());
        assert!(app.world().get::<NormalPrepass>(other).is_some());
        assert!(
            app.world()
                .get::<GraphicsRuntimeLeaseNormalSnapshot>(camera)
                .is_none()
        );
    }
}

#[test]
fn normal_prepass_ownership_merges_and_preserves_authored_normal() {
    let mut app = App::new();
    app.init_resource::<GraphicsRuntimeLeaseRegistry>()
        .add_systems(
            Update,
            (
                restore_released_graphics_runtime_leases,
                ApplyDeferred,
                apply_graphics_runtime_leases,
                ApplyDeferred,
            )
                .chain(),
        );
    let camera = app
        .world_mut()
        .spawn((Camera3d::default(), NormalPrepass))
        .id();
    let mut request = GraphicsRuntimeRequirements::new(camera);
    request.require_normal_prepass = true;
    let first = app
        .world_mut()
        .resource_mut::<GraphicsRuntimeLeaseRegistry>()
        .acquire(request);
    let second = app
        .world_mut()
        .resource_mut::<GraphicsRuntimeLeaseRegistry>()
        .acquire(request);
    app.update();
    app.world_mut()
        .resource_mut::<GraphicsRuntimeLeaseRegistry>()
        .release(first);
    app.update();
    assert!(app.world().get::<NormalPrepass>(camera).is_some());
    app.world_mut()
        .resource_mut::<GraphicsRuntimeLeaseRegistry>()
        .release(second);
    app.update();
    assert!(app.world().get::<NormalPrepass>(camera).is_some());
    assert!(
        app.world()
            .get::<GraphicsRuntimeLeaseNormalSnapshot>(camera)
            .is_none()
    );
}
