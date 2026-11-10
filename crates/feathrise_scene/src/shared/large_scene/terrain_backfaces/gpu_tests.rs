//! 真实GPU验证父/子地形四种材质背面显示；不开原生窗口、不写场景或用户配置。

use super::{components::TerrainBackfacesRenderMaterial, resources::TerrainBackfacesPreview};
use crate::{
    TerrainGridTarget,
    scenes::dynamic_scene::{
        ClipmapBounds, ClipmapFlatHeightPage, ClipmapHeightExtension, ClipmapHeightMaterial,
        ClipmapHeightMaterialPlugin, SparseClipmapSurfaceMaterial,
    },
};
use bevy::camera::{RenderTarget, ScalingMode};
use bevy::prelude::*;
use bevy::render::{
    gpu_readback::{Readback, ReadbackComplete},
    render_resource::{TextureFormat, TextureUsages},
};
use bevy::window::ExitCondition;
use bevy::winit::WinitPlugin;
use feathrise_environment::terrain_surface::{
    TerrainSurfaceExtension, TerrainSurfaceFallbackImages, TerrainSurfaceMaterial,
    TerrainSurfacePlugin,
};

fn frames(app: &mut App, count: usize) {
    for _ in 0..count {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}

fn readback(app: &mut App, image: &Handle<Image>) -> Vec<u8> {
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
            return bytes;
        }
        assert!(deadline.elapsed().as_secs() < 10, "地形背面GPU回读超时");
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}

fn green_in_columns(bytes: &[u8], index: usize) -> usize {
    bytes
        .chunks_exact(4)
        .enumerate()
        .filter(|(pixel, rgba)| {
            let x = pixel % 256;
            x >= index * 64
                && x < (index + 1) * 64
                && rgba[1] > 20
                && u16::from(rgba[1]) > u16::from(rgba[0]) * 2
                && rgba[1] > rgba[2]
        })
        .count()
}

#[test]
#[ignore = "需要真实GPU；四种地形材质从背面验证显示/关闭/热更/原身份，不打开窗口"]
fn terrain_backfaces_gpu_four_material_paths_and_restore() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..default()
            })
            .disable::<WinitPlugin>()
            .disable::<bevy::log::LogPlugin>(),
    )
    .add_plugins((TerrainSurfacePlugin, ClipmapHeightMaterialPlugin))
    .init_resource::<feathrise_scene_content::ActiveDynamicScene>()
    .init_resource::<crate::DynamicClipmapGroundSurface>(); // 高度插件自带覆盖系统的合法空场景上下文。
    super::install(&mut app);
    // 默认管线渲染会把RenderApp移到渲染线程，必须在启动前安装同线程的管线错误检查。
    app.sub_app_mut(bevy::render::RenderApp).add_systems(
        bevy::render::Render,
        (|cache: Res<bevy::render::render_resource::PipelineCache>| {
            for pipeline in cache.pipelines() {
                if let bevy::render::render_resource::CachedPipelineState::Err(error) = &pipeline.state {
                    if matches!(error, bevy::shader::ShaderCacheError::ShaderNotLoaded(_)
                        | bevy::shader::ShaderCacheError::ShaderImportNotYetAvailable) { continue; }
                    // 内置Shader首次进入RenderApp也会短暂等待加载；仅不可恢复的编译/验证错误失败。
                    panic!("地形背面管线创建失败：{error:?}");
                }
            }
        })
        .in_set(bevy::render::RenderSystems::Cleanup),
    );
    app.finish();
    app.cleanup();
    let mut target = Image::new_target_texture(256, 128, TextureFormat::Rgba8UnormSrgb, None);
    target.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(target);
    app.world_mut().spawn((
        Camera3d::default(),
        RenderTarget::from(target.clone()),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::Fixed {
                width: 14.0,
                height: 7.0,
            },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0.0, -10.0, 0.0).looking_at(Vec3::ZERO, Vec3::Z),
        Msaa::Off,
    ));
    let base = StandardMaterial {
        base_color: Color::srgb(0.1, 0.8, 0.2),
        unlit: true,
        ..default()
    };
    let mesh = Plane3d::default().mesh().size(2.5, 2.5).build();
    let count = mesh.count_vertices();
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(mesh.with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, vec![[0.0, 0.0]; count]));
    let surface = TerrainSurfaceMaterial {
        base: base.clone(),
        extension: TerrainSurfaceExtension::new_passthrough(
            app.world().resource::<TerrainSurfaceFallbackImages>(),
        ),
    };
    let height = ClipmapHeightExtension::new_flat(
        ClipmapBounds::new(Vec2::splat(-20.0), Vec2::splat(20.0)).unwrap(),
        UVec2::splat(40),
        app.world().resource::<ClipmapFlatHeightPage>(),
    )
    .unwrap();
    let standard = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(base.clone());
    let terrain = app
        .world_mut()
        .resource_mut::<Assets<TerrainSurfaceMaterial>>()
        .add(surface.clone());
    let fallback = app
        .world_mut()
        .resource_mut::<Assets<ClipmapHeightMaterial>>()
        .add(ClipmapHeightMaterial {
            base,
            extension: height.clone(),
        });
    let sparse = app
        .world_mut()
        .resource_mut::<Assets<SparseClipmapSurfaceMaterial>>()
        .add(SparseClipmapSurfaceMaterial {
            base: surface,
            extension: height,
        });
    let mut entities = Vec::new();
    for x in [-4.5, -1.5, 1.5, 4.5] {
        entities.push(
            app.world_mut()
                .spawn((
                    TerrainGridTarget,
                    Mesh3d(mesh.clone()),
                    Transform::from_xyz(x, 0.0, 0.0),
                ))
                .id(),
        );
    }
    app.world_mut()
        .entity_mut(entities[0])
        .insert(MeshMaterial3d(standard.clone()));
    app.world_mut()
        .entity_mut(entities[1])
        .insert(MeshMaterial3d(terrain.clone()));
    app.world_mut()
        .entity_mut(entities[2])
        .insert(MeshMaterial3d(fallback.clone()));
    app.world_mut()
        .entity_mut(entities[3])
        .insert(MeshMaterial3d(sparse.clone()));
    // 模拟共享同一普通材质的非地形资产；编辑器覆盖不能修改其源材质。
    let prop = app.world_mut().spawn(MeshMaterial3d(standard.clone())).id();
    frames(&mut app, 30);
    let before = readback(&mut app, &target);
    for index in 0..4 {
        assert_eq!(
            green_in_columns(&before, index),
            0,
            "单面材质的背面应被剔除：{index}"
        );
    }
    app.world_mut()
        .resource_mut::<TerrainBackfacesPreview>()
        .enabled = true;
    frames(&mut app, 45);
    let shown = readback(&mut app, &target);
    image::save_buffer(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/terrain-backfaces-gpu-on.png"),
        &shown,
        256,
        128,
        image::ColorType::Rgba8,
    )
    .unwrap();
    for index in 0..4 {
        let green = green_in_columns(&shown, index);
        eprintln!("地形背面正式材质路径{index}：绿色像素{green}");
    }
    for index in 0..4 {
        let green = green_in_columns(&shown, index);
        assert!(green > 300, "显示地形背面未进入GPU实际绘制：{index}");
    }
    assert!(
        app.world()
            .get::<TerrainBackfacesRenderMaterial>(prop)
            .is_none()
    );
    assert_eq!(
        app.world()
            .get::<MeshMaterial3d<StandardMaterial>>(entities[0])
            .unwrap()
            .0,
        standard
    );
    assert_eq!(
        app.world()
            .get::<MeshMaterial3d<TerrainSurfaceMaterial>>(entities[1])
            .unwrap()
            .0,
        terrain
    );
    assert_eq!(
        app.world()
            .get::<MeshMaterial3d<ClipmapHeightMaterial>>(entities[2])
            .unwrap()
            .0,
        fallback
    );
    assert_eq!(
        app.world()
            .get::<MeshMaterial3d<SparseClipmapSurfaceMaterial>>(entities[3])
            .unwrap()
            .0,
        sparse
    );
    // 子图移动时生产系统仍更新原材质；临时材质必须接到同一实例矩阵，不能保持旧位置。
    let moved = Mat4::from_translation(Vec3::new(0.0, 3.0, 0.0));
    app.world_mut()
        .resource_mut::<Assets<TerrainSurfaceMaterial>>()
        .get_mut(&terrain)
        .unwrap()
        .extension
        .set_source_frame(moved);
    frames(&mut app, 8);
    let displayed = app
        .world()
        .get::<TerrainBackfacesRenderMaterial>(entities[1])
        .unwrap()
        .displayed
        .typed::<TerrainSurfaceMaterial>();
    assert_eq!(
        app.world()
            .resource::<Assets<TerrainSurfaceMaterial>>()
            .get(displayed)
            .unwrap()
            .extension
            .source_frame(),
        moved
    );
    app.world_mut()
        .resource_mut::<TerrainBackfacesPreview>()
        .enabled = false;
    frames(&mut app, 15);
    let restored = readback(&mut app, &target);
    for index in 0..4 {
        assert_eq!(
            green_in_columns(&restored, index),
            0,
            "关闭地形背面开关后仍显示背面：{index}"
        );
    }
    assert!(
        !app.world()
            .resource::<TerrainBackfacesPreview>()
            .is_restoring()
    );
}
