//! 无窗口真实GPU回归：保留同一批线框，反复改变多个相机的预通道、天空、雾和MSAA。

use super::WireframeViewSpecializationPlugin;
use bevy::{
    camera::RenderTarget,
    core_pipeline::prepass::DepthPrepass,
    light::{Atmosphere, atmosphere::ScatteringMedium},
    pbr::{
        AtmosphereSettings, DistanceFog,
        wireframe::{Wireframe, WireframeColor, WireframePlugin},
    },
    prelude::*,
    render::{
        Render, RenderApp, RenderPlugin, RenderSystems,
        error_handler::RenderErrorHandler,
        gpu_readback::{Readback, ReadbackComplete},
        render_resource::{
            CachedPipelineState, PipelineCache, TextureFormat, TextureUsages, WgpuFeatures,
        },
        settings::WgpuSettings,
    },
    window::ExitCondition,
    winit::WinitPlugin,
};

fn frames(app: &mut App, count: usize) {
    for _ in 0..count {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}

fn yellow_pixels(app: &mut App, image: &Handle<Image>) -> usize {
    let (send, receive) = std::sync::mpsc::channel();
    app.world_mut()
        .spawn(Readback::texture(image.clone()))
        .observe(move |event: On<ReadbackComplete>, mut commands: Commands| {
            let _ = send.send(event.data.clone());
            commands.entity(event.entity).try_despawn();
        });
    let deadline = std::time::Instant::now();
    loop {
        app.update();
        if let Ok(bytes) = receive.try_recv() {
            return bytes
                .chunks_exact(4)
                .filter(|pixel| {
                    pixel[0] > 80 && pixel[1] > 60 && u16::from(pixel[2]) * 2 < u16::from(pixel[1])
                })
                .count();
        }
        assert!(deadline.elapsed().as_secs() < 20, "线框GPU回读超时");
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}

fn assert_wireframe_visible(app: &mut App, image: &Handle<Image>) {
    let deadline = std::time::Instant::now();
    loop {
        if yellow_pixels(app, image) > 10 {
            return;
        }
        assert!(
            deadline.elapsed().as_secs() < 20,
            "布局换代后线框未实际绘制"
        );
        frames(app, 4); // 首次专用化可异步编译；等待真实像素，不把冷启动延迟当作绘制失败。
    }
}

#[test]
#[ignore = "需要真实GPU；无窗口验证线框在多相机布局换代后仍实际绘制"]
fn gpu_wireframes_survive_camera_layout_changes() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..default()
            })
            .set(RenderPlugin {
                render_creation: WgpuSettings {
                    features: WgpuFeatures::POLYGON_MODE_LINE | WgpuFeatures::IMMEDIATES,
                    ..default()
                }
                .into(),
                ..default()
            })
            .disable::<WinitPlugin>()
            .disable::<bevy::log::LogPlugin>(),
    )
    .add_plugins((
        WireframePlugin::default(),
        WireframeViewSpecializationPlugin,
        feathrise_atmosphere::WorldTimePlugin,
    ))
    .init_resource::<feathrise_settings::RenderSettings>()
    .init_state::<feathrise_scene_core::state::ScenePhase>()
    .insert_resource(RenderErrorHandler(|error, _, _| {
        panic!("线框GPU绘制验证失败：{error:?}");
    })); // GPU验证错误直接令测试失败，不能被默认退出策略误当作成功。
    app.sub_app_mut(RenderApp).add_systems(
        Render,
        (|cache: Res<PipelineCache>| {
            for pipeline in cache.pipelines() {
                if let CachedPipelineState::Err(error) = &pipeline.state {
                    if matches!(
                        error,
                        bevy::shader::ShaderCacheError::ShaderNotLoaded(_)
                            | bevy::shader::ShaderCacheError::ShaderImportNotYetAvailable
                    ) {
                        continue;
                    }
                    panic!("线框GPU管线创建失败：{error:?}");
                }
            }
        })
        .in_set(RenderSystems::Cleanup),
    );
    app.finish();
    app.cleanup();

    let medium = app
        .world_mut()
        .resource_mut::<Assets<ScatteringMedium>>()
        .add(ScatteringMedium::earth(32, 32));
    app.world_mut().spawn(Atmosphere::earth(medium));
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::new(2.0, 2.0, 2.0));
    let material = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: Color::BLACK,
            unlit: true,
            ..default()
        });
    app.world_mut().spawn((
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Wireframe,
        WireframeColor {
            color: Color::srgb(1.0, 0.8, 0.0),
        },
        Transform::default(),
    )); // 全程不改网格或线框组件，避免实体变更意外掩盖视图缓存遗漏。

    let cameras: Vec<_> = (0..3)
        .map(|index| {
            let mut target = Image::new_target_texture(64, 64, TextureFormat::Rgba8UnormSrgb, None);
            target.texture_descriptor.usage |= TextureUsages::COPY_SRC;
            let image = app.world_mut().resource_mut::<Assets<Image>>().add(target);
            let entity = app
                .world_mut()
                .spawn((
                    Camera3d::default(),
                    Camera {
                        order: index,
                        clear_color: ClearColorConfig::Custom(Color::BLACK),
                        ..default()
                    },
                    RenderTarget::from(image.clone()),
                    Transform::from_xyz(4.0, 3.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
                    Msaa::Sample4,
                ))
                .id();
            (entity, image)
        })
        .collect();
    // 复用正式世界时间插件的大气移除兼容层，并沿用玩法视图先初始化、再增加预览的顺序。
    // 上游共享大气缓冲的首次写入要求单个视图；此处不把其冷启动限制混入线框缓存回归。
    app.world_mut()
        .entity_mut(cameras[0].0)
        .insert(AtmosphereSettings::default());
    frames(&mut app, 80);
    for (_, image) in &cameras {
        assert_wireframe_visible(&mut app, image);
    }
    eprintln!("线框GPU初始化完成，开始轮换三个已绘制视图的布局");
    // 对应日志的三种布局同时存在，然后轮换它们并反向移除；同一相机身份持续复用。
    for pass in 0..3 {
        eprintln!("线框GPU布局轮换：{pass}");
        for (index, (entity, _)) in cameras.iter().enumerate() {
            let layout = (index + pass) % 3;
            let mut camera = app.world_mut().entity_mut(*entity);
            camera.remove::<(DepthPrepass, DistanceFog, AtmosphereSettings)>();
            if layout != 0 {
                camera.insert(AtmosphereSettings::default());
            }
            if layout == 2 {
                camera.insert((DepthPrepass, DistanceFog::default()));
            }
        }
        frames(&mut app, 40);
        for (_, image) in &cameras {
            assert_wireframe_visible(&mut app, image);
        }
    }
    for (entity, _) in &cameras {
        app.world_mut()
            .entity_mut(*entity)
            .remove::<(DepthPrepass, DistanceFog, AtmosphereSettings)>()
            .insert(Msaa::Off);
    }
    frames(&mut app, 40);
    for (_, image) in &cameras {
        assert_wireframe_visible(&mut app, image);
    }
    frames(&mut app, 4); // 留出GPU错误传回主世界的帧，避免最后一次回读结束得过早。
}
