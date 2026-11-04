//! 辅助 PBR 视图租约身份、稳定帧与回退回归；不启动 RenderApp 或 GPU 管线。

use bevy::{
    asset::AssetPlugin,
    camera::RenderTarget,
    core_pipeline::prepass::{DeferredPrepass, DepthPrepass},
    pbr::deferred::PbrDeferredLightingDepthId,
    prelude::*,
};
use feathrise_scene_core::view::{DeferredRasterAuxiliaryCamera, PrimarySceneView};
use feathrise_settings::{
    GraphicsRuntimeLeaseRegistry, GraphicsRuntimeLeaseSystems, GraphicsSettingsPlugin,
};

use super::maintain_deferred_auxiliary_camera_leases;
use crate::resources::{
    DeferredAuxiliaryCameraLeaseState, DeferredRasterBridgeState, RaytracingRuntime,
};

fn test_app() -> App {
    let mut app = App::new();
    app.register_required_components::<Camera, Msaa>(); // 复现DefaultPlugins中RenderPlugin为每台相机注册的采样数组件，不启动GPU子应用。
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        GraphicsSettingsPlugin,
    ))
    .init_resource::<DeferredRasterBridgeState>()
    .init_resource::<DeferredAuxiliaryCameraLeaseState>()
    .init_resource::<RaytracingRuntime>()
    .add_systems(
        PostUpdate,
        maintain_deferred_auxiliary_camera_leases
            .before(GraphicsRuntimeLeaseSystems::RestoreReleased), // 与生产调度一致，先登记票据再恢复/应用租约。
    );
    app
}

fn image_target() -> RenderTarget {
    RenderTarget::Image(Handle::<Image>::default().into()) // 句柄身份足以验证主世界相机筛选。
}

#[test]
fn bridge_leases_only_explicit_image_auxiliary_views_and_is_stable() {
    let mut app = test_app();
    app.world_mut()
        .resource_mut::<DeferredRasterBridgeState>()
        .installed = true;
    let auxiliary = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            image_target(),
            DeferredRasterAuxiliaryCamera,
        ))
        .id();
    app.world_mut().spawn((Camera3d::default(), image_target())); // 未授权纹理相机。
    app.world_mut().spawn((
        Camera3d::default(),
        RenderTarget::Window(bevy::window::WindowRef::Primary),
        DeferredRasterAuxiliaryCamera,
    )); // 即使误标记，窗口目标也不得取得辅助租约。
    app.world_mut().spawn((
        Camera3d::default(),
        image_target(),
        DeferredRasterAuxiliaryCamera,
        PrimarySceneView,
    )); // 玩法主相机身份永远不能走辅助路径。

    app.update();
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        1
    );
    let first = app
        .world()
        .resource::<DeferredAuxiliaryCameraLeaseState>()
        .leases[&auxiliary];

    app.world_mut().clear_trackers();
    app.update();
    assert_eq!(
        app.world()
            .resource::<DeferredAuxiliaryCameraLeaseState>()
            .leases[&auxiliary],
        first
    ); // 稳定帧沿用同一票据，不触发租约资源或相机附件重写。
    assert!(
        !app.world()
            .is_resource_changed::<DeferredAuxiliaryCameraLeaseState>()
    );
    assert!(
        !app.world()
            .is_resource_changed::<GraphicsRuntimeLeaseRegistry>()
    ); // 收敛后连资源change tick也保持静止。
    assert!(
        !app.world()
            .entity(auxiliary)
            .get_ref::<Msaa>()
            .unwrap()
            .is_changed()
    );
    assert!(
        !app.world()
            .entity(auxiliary)
            .get_ref::<DepthPrepass>()
            .unwrap()
            .is_changed()
    );
    assert!(
        !app.world()
            .entity(auxiliary)
            .get_ref::<DeferredPrepass>()
            .unwrap()
            .is_changed()
    );
    assert!(
        !app.world()
            .entity(auxiliary)
            .get_ref::<PbrDeferredLightingDepthId>()
            .unwrap()
            .is_changed()
    ); // 相机附件和Deferred pass id在稳定帧都不重复触发专用化。
}

fn spawn_dynamic_auxiliary_camera(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        image_target(),
        DeferredRasterAuxiliaryCamera,
    )); // 模拟角色预览等在Update中通过延迟命令创建RTT。
}

#[test]
fn update_spawned_auxiliary_camera_is_leased_before_same_frame_extraction() {
    let mut app = test_app();
    app.world_mut()
        .resource_mut::<DeferredRasterBridgeState>()
        .installed = true;
    app.add_systems(Update, spawn_dynamic_auxiliary_camera.run_if(run_once));

    app.update();

    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        1
    ); // Update命令在PostUpdate前已落地，新RTT不会先以纯Forward路径提取一帧。
    let camera = app
        .world_mut()
        .query_filtered::<Entity, With<DeferredRasterAuxiliaryCamera>>()
        .single(app.world())
        .expect("动态辅助相机应在同一Update末落地");
    assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Off));
    assert!(app.world().get::<DepthPrepass>(camera).is_some());
    assert!(app.world().get::<DeferredPrepass>(camera).is_some());
    assert!(
        app.world()
            .get::<PbrDeferredLightingDepthId>(camera)
            .is_some()
    ); // PostUpdate同帧完成完整可见路径，而不只是登记一张尚未应用的票据。
}

#[test]
fn bridge_releases_on_rollback_marker_removal_and_despawn() {
    let mut app = test_app();
    app.world_mut()
        .resource_mut::<DeferredRasterBridgeState>()
        .installed = true;
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            image_target(),
            DeferredRasterAuxiliaryCamera,
        ))
        .id();
    app.update();
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        1
    );

    app.world_mut()
        .entity_mut(camera)
        .remove::<DeferredRasterAuxiliaryCamera>();
    app.update();
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        0
    );

    app.world_mut()
        .entity_mut(camera)
        .insert(DeferredRasterAuxiliaryCamera);
    app.update();
    app.world_mut().despawn(camera);
    app.update();
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        0
    );

    let replacement = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            image_target(),
            DeferredRasterAuxiliaryCamera,
        ))
        .id();
    app.update();
    assert!(
        app.world()
            .resource::<DeferredAuxiliaryCameraLeaseState>()
            .leases
            .contains_key(&replacement)
    );
    app.world_mut()
        .resource_mut::<DeferredRasterBridgeState>()
        .installed = false;
    app.update();
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        0
    ); // 材质桥回退帧释放全部辅助票据，不泄漏到下一会话。
}

#[test]
fn auxiliary_camera_changing_away_from_image_restores_author_state_same_frame() {
    let mut app = test_app();
    app.world_mut()
        .resource_mut::<DeferredRasterBridgeState>()
        .installed = true;
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            image_target(),
            DeferredRasterAuxiliaryCamera,
        ))
        .id();
    app.update();
    assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Off));

    app.world_mut()
        .entity_mut(camera)
        .insert(RenderTarget::Window(bevy::window::WindowRef::Primary)); // 模拟页面重建或目标换代，不移除公开marker。
    app.update();

    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        0
    );
    assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Sample4));
    assert!(app.world().get::<DepthPrepass>(camera).is_none());
    assert!(app.world().get::<DeferredPrepass>(camera).is_none());
    assert!(
        app.world()
            .get::<PbrDeferredLightingDepthId>(camera)
            .is_none()
    ); // 目标资格失效的同一PostUpdate帧释放票据并恢复完整作者状态。
}
