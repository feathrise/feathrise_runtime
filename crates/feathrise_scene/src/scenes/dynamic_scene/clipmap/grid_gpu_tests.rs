//! 离屏验证普通三角线与两个独立Clipmap的真实绘制、GPU高度位移和关闭恢复。
use super::*;
use bevy::{camera::{RenderTarget, ScalingMode}, render::{
    gpu_readback::{Readback, ReadbackComplete},
    render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages, WgpuFeatures},
    settings::{RenderCreation, WgpuSettings}, RenderPlugin,
}};
use feathrise_scene_core::residency::{initialize_instance_context, InstanceResourceTable, SceneInstanceOwner, SceneRuntimeScope};
use super::super::{ClipmapBounds, ClipmapFlatHeightPage, ClipmapHeightAtlas, ClipmapHeightExtension, ClipmapHeightMaterial, ClipmapHeightMaterialPlugin};

fn frames(app: &mut App, count: usize) {
    for _ in 0..count { app.update(); std::thread::sleep(std::time::Duration::from_millis(16)); }
}
fn pixels(app: &mut App, target: &Handle<Image>) -> Vec<u8> {
    let (send, receive) = std::sync::mpsc::channel();
    app.world_mut().spawn(Readback::texture(target.clone())).observe(
        move |event: On<ReadbackComplete>, mut commands: Commands| {
            let _ = send.send(event.data.clone()); commands.entity(event.entity).try_despawn();
        });
    let start = std::time::Instant::now();
    loop {
        app.update();
        if let Ok(bytes) = receive.try_recv() { return bytes; }
        assert!(start.elapsed().as_secs() < 10, "三角线GPU回读超时");
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}
fn gold_by_region(bytes: &[u8]) -> [usize; 3] {
    let mut count = [0; 3];
    for (pixel, rgba) in bytes.chunks_exact(4).enumerate() {
        if rgba[0] > 120 && rgba[1] > 80 && u16::from(rgba[0]) > u16::from(rgba[2]) * 3 / 2 {
            count[(pixel % 768) / 256] += 1;
        }
    }
    count
}

fn gpu_app() -> App {
    let mut app = App::new();
    // 手动安装正式三维渲染链，避免测试无关的DLSS项目身份、音频和原生窗口。
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), TransformPlugin,
        WindowPlugin { primary_window: None, exit_condition: bevy::window::ExitCondition::DontExit, ..default() },
        RenderPlugin { render_creation: RenderCreation::Automatic(Box::new(WgpuSettings {
            features: WgpuFeatures::POLYGON_MODE_LINE | WgpuFeatures::IMMEDIATES, ..default()
        })), ..default() },
        bevy::image::ImagePlugin::default(), bevy::mesh::MeshPlugin,
        bevy::camera::CameraPlugin, bevy::light::LightPlugin,
        bevy::core_pipeline::CorePipelinePlugin, bevy::gltf::GltfPlugin::default(), bevy::pbr::PbrPlugin::default(),
        bevy::state::app::StatesPlugin));
    app.add_plugins((bevy::gizmos::GizmoPlugin, bevy::gizmos_render::GizmoRenderPlugin));
    app.init_state::<ScenePhase>()
        .add_plugins((bevy::pbr::wireframe::WireframePlugin::default(),
            feathrise_environment::terrain_surface::TerrainSurfacePlugin, ClipmapHeightMaterialPlugin, ClipmapTerrainGridPlugin))
        .init_resource::<feathrise_scene_content::ActiveDynamicScene>()
        .init_resource::<crate::DynamicClipmapGroundSurface>()
        .init_resource::<crate::scenes::dynamic_scene::sparse_runtime::SparseClipmapRuntime>() // 高度插件附带贴地覆盖系统；空运行时保持该无关通道关闭。
        .init_instance_resource::<SparseHeightPageRuntime>()
        .add_systems(Update, (
            crate::shared::large_scene::sync_regular_terrain_grid_visibility,
            crate::shared::large_scene::sync_changed_regular_terrain_grid_targets,
        ).chain());
    app.sub_app_mut(bevy::render::RenderApp).add_systems(bevy::render::Render,
        (|cache: Res<bevy::render::render_resource::PipelineCache>| {
            for pipeline in cache.pipelines() {
                if let bevy::render::render_resource::CachedPipelineState::Err(error) = &pipeline.state {
                    if matches!(error, bevy::shader::ShaderCacheError::ShaderNotLoaded(_)
                        | bevy::shader::ShaderCacheError::ShaderImportNotYetAvailable) { continue; }
                    panic!("三角线GPU管线失败：{error:?}");
                }
            }
        }).in_set(bevy::render::RenderSystems::Cleanup));
    app.finish(); app.cleanup();
    app
}

#[test]
#[ignore = "真实GPU无窗口验证普通Mesh及独立Clipmap三角线，不触碰用户场景和编辑器"]
fn terrain_grid_gpu_renders_regular_and_two_displaced_instances() {
    let mut app = gpu_app();
    let mut image = Image::new_target_texture(768, 384, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().spawn((Camera3d::default(), RenderTarget::from(target.clone()),
        Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() },
        Projection::Orthographic(OrthographicProjection { scaling_mode: ScalingMode::Fixed { width: 12.0, height: 6.0 },
            ..OrthographicProjection::default_3d() }),
        Transform::from_xyz(0.0, 9.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y), Msaa::Off));
    let plane = Plane3d::default().mesh().size(2.5, 2.5).subdivisions(8).build();
    let vertices = plane.count_vertices();
    let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(
        plane.with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, vec![[0.0, 0.0]; vertices]));
    let base = StandardMaterial { base_color: Color::srgb(0.05, 0.18, 0.32), unlit: true, ..default() };
    let regular_material = app.world_mut().resource_mut::<Assets<StandardMaterial>>().add(base.clone());
    app.world_mut().spawn((TerrainGridTarget, Mesh3d(mesh.clone()), MeshMaterial3d(regular_material), Transform::from_xyz(-4.0, 0.0, 0.0)));
    // 两层数组与正式页格式一致；倾斜高度0～1m确保线框不能以Y=0平面蒙混通过。
    let mut heights = Vec::with_capacity(257 * 257 * 4);
    for _ in 0..2 { for z in 0..257 { for x in 0..257 {
        heights.extend_from_slice(&(((x + z) * 65535 / 512) as u16).to_le_bytes());
    } } }
    let height_image = Image::new(Extent3d { width: 257, height: 257, depth_or_array_layers: 2 }, TextureDimension::D2,
        heights, TextureFormat::R16Unorm, bevy::asset::RenderAssetUsages::default());
    let entry = super::super::encode_page_table_entry(0, 0.0, 1.0).unwrap();
    let table_image = Image::new(Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, TextureDimension::D2,
        entry.into_iter().flat_map(u32::to_le_bytes).collect(), TextureFormat::Rgba32Uint, bevy::asset::RenderAssetUsages::default());
    let heights = app.world_mut().resource_mut::<Assets<Image>>().add(height_image);
    let table = app.world_mut().resource_mut::<Assets<Image>>().add(table_image);
    let mut instances = Vec::new();
    for x in [0.0, 4.0] {
        let instance = app.world_mut().spawn_empty().id();
        initialize_instance_context(app.world_mut(), instance);
        let pose = Transform::from_xyz(x, 0.0, 0.0);
        let mut extension = ClipmapHeightExtension::new_flat(
            ClipmapBounds::new(Vec2::splat(-1.25), Vec2::splat(1.25)).unwrap(), UVec2::splat(256),
            app.world().resource::<ClipmapFlatHeightPage>()).unwrap();
        extension.set_source_frame(pose.to_matrix());
        extension.set_atlas(ClipmapHeightAtlas { heights: heights.clone(), page_table: table.clone(), max_mip: 0 }).unwrap();
        let material = app.world_mut().resource_mut::<Assets<ClipmapHeightMaterial>>().add(
            ExtendedMaterial { base: base.clone(), extension: extension.clone() });
        app.world_mut().spawn((SceneInstanceOwner(instance), TerrainGridTarget, DynamicSparseClipmapPrimitive,
            Mesh3d(mesh.clone()), MeshMaterial3d(material), pose, NoFrustumCulling));
        let mut pages = app.world_mut().resource_mut::<InstanceResourceTable<SparseHeightPageRuntime>>();
        let pages = pages.values.get_mut(&instance).unwrap(); pages.extension = Some(extension); pages.extension_revision = 1;
        let mut schedule = Schedule::default(); schedule.add_systems(sync_clipmap_terrain_grid);
        instances.push((instance, schedule));
    }
    // 测试只驱动固定两个来源；每份调度保持和正式实例驱动一样独立的change tick。
    app.add_systems(Update, move |world: &mut World| {
        for (instance, schedule) in &mut instances {
            world.resource_mut::<SceneRuntimeScope>().instance = Some(*instance); schedule.run(world);
        }
        *world.resource_mut::<SceneRuntimeScope>() = SceneRuntimeScope::default();
    });
    frames(&mut app, 60);
    let off = pixels(&mut app, &target);
    assert_eq!(gold_by_region(&off), [0; 3]);
    app.world_mut().resource_mut::<TerrainGridPreview>().visible = true;
    frames(&mut app, 90);
    let on = pixels(&mut app, &target);
    let counts = gold_by_region(&on);
    // 固定视角下，缺少坡度偏移的旧实现仅约2181像素，完整线框约3888；只断言非零会漏掉断线。
    assert!(counts.iter().all(|count| *count > 3000), "三个区域必须连续绘制三角线，不能只剩边框或深度闪烁的零星片段：{counts:?}");
    eprintln!("三角线GPU区域像素（普通/Clipmap1/Clipmap2）：{counts:?}");
    // 暖金线必须沿正式蓝色表面，而不是远离真实表面的零高度网格。
    for (pixel, rgba) in on.chunks_exact(4).enumerate() {
        if rgba[0] > 120 && rgba[1] > 80 && u16::from(rgba[0]) > u16::from(rgba[2]) * 3 / 2 && pixel % 768 >= 256 {
            let x = pixel % 768; let y = pixel / 768;
            assert!((y.saturating_sub(2)..=(y + 2).min(383)).any(|sy|
                (x.saturating_sub(2)..=(x + 2).min(767)).any(|sx| off[(sy * 768 + sx) * 4 + 2] > 40)),
                "Clipmap线条没有贴合真实GPU位移表面：{x},{y}");
        }
    }
    let output = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.local/validation");
    std::fs::create_dir_all(&output).unwrap();
    image::RgbaImage::from_raw(768, 384, on).unwrap().save(output.join("terrain-triangle-grid.png")).unwrap();
    app.world_mut().resource_mut::<TerrainGridPreview>().visible = false;
    frames(&mut app, 8);
    assert_eq!(gold_by_region(&pixels(&mut app, &target)), [0; 3]);
    assert_eq!(app.world_mut().query_filtered::<Entity, With<ClipmapTerrainGridOverlay>>().iter(app.world()).count(), 0);
}

#[test]
#[ignore = "真实GPU验证相机派生LOD与两个源外中心，不触碰用户场景或窗口"]
fn camera_lod_gpu_two_sources_remain_complete_and_change_triangle_density() {
    use crate::scenes::dynamic_scene::{sparse_lod::{ClipmapView, compile_view_plan}, clipmap::build_display_lod_mesh};
    use feathrise_scene_content::{SparseHeightSpec, SceneTerrainDisplayLod, compile_display_lod_plan};
    use feathrise_scene_core::residency::SceneSourceFrame;
    let mut app = gpu_app();
    let mut image = Image::new_target_texture(768, 576, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    // 固定俯视诊断相机使各档截图可逐像素比较；作者档位由同一个实际观察机位决定。
    app.world_mut().spawn((Camera3d::default(), RenderTarget::from(target.clone()),
        Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() },
        Projection::Orthographic(OrthographicProjection { scaling_mode: ScalingMode::Fixed { width: 320.0, height: 240.0 }, ..OrthographicProjection::default_3d() }),
        Transform::from_xyz(0.0, 500.0, -65.0).looking_at(Vec3::new(0.0, 0.0, -65.0), Vec3::NEG_Z), Msaa::Off));
    let spec = SparseHeightSpec::new([-48.0; 2], [48.0; 2], 0.25, -64.0, 128.0).unwrap();
    let authored = compile_display_lod_plan(&spec, &[
        SceneTerrainDisplayLod { outer_half_extent_meters: Some(16.0), spacing_multiplier: 1 },
        SceneTerrainDisplayLod { outer_half_extent_meters: Some(32.0), spacing_multiplier: 2 },
        SceneTerrainDisplayLod { outer_half_extent_meters: None, spacing_multiplier: 4 },
    ]).unwrap();
    let poses = [Transform::from_xyz(-80.0, 0.0, 0.0), Transform::from_xyz(80.0, 0.0, -130.0)];
    let mut sources = Vec::new();
    let mut instances = Vec::new();
    for pose in poses {
        let instance = app.world_mut().spawn_empty().id();
        initialize_instance_context(app.world_mut(), instance);
        let mut extension = ClipmapHeightExtension::new_flat(ClipmapBounds::new(Vec2::splat(-48.0), Vec2::splat(48.0)).unwrap(),
            UVec2::splat(384), app.world().resource::<ClipmapFlatHeightPage>()).unwrap();
        extension.set_source_frame(pose.to_matrix());
        let material = app.world_mut().resource_mut::<Assets<ClipmapHeightMaterial>>().add(ExtendedMaterial {
            base: StandardMaterial { base_color: Color::srgb(0.05, 0.18, 0.32), unlit: true, ..default() }, extension: extension.clone() });
        let entity = app.world_mut().spawn((SceneInstanceOwner(instance), TerrainGridTarget, DynamicSparseClipmapPrimitive,
            MeshMaterial3d(material), pose, NoFrustumCulling)).id();
        sources.push(entity);
        let mut pages = app.world_mut().resource_mut::<InstanceResourceTable<SparseHeightPageRuntime>>();
        let pages = pages.values.get_mut(&instance).unwrap(); pages.extension = Some(extension); pages.extension_revision = 1;
        let mut schedule = Schedule::default(); schedule.add_systems(sync_clipmap_terrain_grid);
        instances.push((instance, schedule));
    }
    app.add_systems(Update, move |world: &mut World| {
        for (instance, schedule) in &mut instances {
            world.resource_mut::<SceneRuntimeScope>().instance = Some(*instance); schedule.run(world);
        }
        *world.resource_mut::<SceneRuntimeScope>() = SceneRuntimeScope::default();
    });
    app.world_mut().resource_mut::<TerrainGridPreview>().visible = true;
    let mut phases = Vec::new();
    for (name, position) in [("near", Vec3::new(-80.0, 12.0, 25.0)), ("far", Vec3::new(-80.0, 350.0, 420.0)), ("return", Vec3::new(-80.0, 12.0, 25.0))] {
        let view = ClipmapView::new(&GlobalTransform::from(Transform::from_translation(position).looking_at(Vec3::new(30.0, 0.0, -100.0), Vec3::Y)),
            &Projection::Perspective(PerspectiveProjection { fov: 1.0, aspect_ratio: 1.0, ..default() }), Vec2::splat(1024.0)).unwrap();
        let mut stats = Vec::new();
        for (index, pose) in poses.iter().enumerate() {
            let (focus, key) = view.choice(&spec, SceneSourceFrame::from_transform(*pose), [0.0; 2], None, &authored).unwrap();
            let plan = compile_view_plan(&spec, &authored, key).unwrap();
            let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(build_display_lod_mesh(&plan, Vec2::splat(0.25)).unwrap());
            app.world_mut().entity_mut(sources[index]).insert((Mesh3d(mesh), *pose * Transform::from_xyz(focus.x, 0.0, focus.y)));
            stats.push((key.minimum_multiplier, plan.index_count / 3));
        }
        frames(&mut app, 60);
        let bytes = pixels(&mut app, &target);
        for (px, py) in [(0, 0), (383, 288), (767, 575)] {
            assert_eq!(&bytes[(py * 768 + px) * 4..][..3], &[0, 0, 0],
                "{name}白模不能把场景外覆盖环画到源边界之外");
        }
        // 每个96米方块内部固定取20×20点；任意一处黑像素都表示覆盖/接缝/材质失败。
        for pose in poses {
            for z in 0..20 { for x in 0..20 {
                let wx = pose.translation.x - 45.0 + x as f32 * 4.5;
                let wz = pose.translation.z - 45.0 + z as f32 * 4.5;
                let px = ((wx + 160.0) / 320.0 * 768.0) as usize;
                let py = ((wz + 185.0) / 240.0 * 576.0) as usize;
                let rgba = &bytes[(py * 768 + px) * 4..][..4];
                assert!(rgba[2] > 30 || rgba[0] > 120, "{name}网格内部出现空洞：{wx},{wz} {rgba:?}");
            }}
        }
        let output = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.local/validation");
        std::fs::create_dir_all(&output).unwrap();
        image::RgbaImage::from_raw(768, 576, bytes).unwrap().save(output.join(format!("camera-lod-{name}.png"))).unwrap();
        eprintln!("相机LOD {name}（倍率,三角数）：{stats:?}");
        phases.push(stats);
    }
    assert_eq!([phases[0][0].0, phases[0][1].0], [1, 4], "远来源只用已配置的末档");
    for index in 0..2 { assert_eq!(phases[1][index].0, 4, "拉远不能新增第4/5档或超过作者×4末档"); }
    assert!(phases[1][0].1 < phases[0][0].1, "近来源拉远后减少真实三角形");
    assert_eq!(phases[1][1], phases[0][1], "已经处于末档的远来源不能再无限粗化");
    assert_eq!(phases[0], phases[2], "靠近必须恢复精度");
}
