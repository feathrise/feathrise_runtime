//! 无地形R8静水使用正式水插件做像素回归，比较完整几何与生产上传分块的颜色和覆盖。

use super::{gpu_app_with, gpu_frames};
use bevy::camera::{RenderTarget, ScalingMode};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
use bevy::render::render_resource::{TextureFormat, TextureUsages};
use feathrise_map_content::{MapId, MapWaterLayer};
use feathrise_scene_core::residency::SceneResidencyRuntimeSet;

use crate::shared::large_map::{
    BuiltWaterSurface, DynamicWaterMaterial, LargeMapWaterPlugin, PaintedWaterMeshData,
    PaintedWaterPurpose, PaintedWaterSurfaceSpawn, WaterRenderChunk, WaterSurfaceDescriptor,
    chunk_water_surfaces, spawn_staged_water_surface, stage_water_surface_assets,
    stage_water_surface_chunk,
};

const IMAGE_SIDE: u32 = 256; // RGBA行宽为1024字节，满足GPU回读对齐且无需裁掉填充字节。

fn render_target(app: &mut App) -> Handle<Image> {
    let world = app.world_mut();
    let cameras: Vec<_> = world.query_filtered::<Entity, With<Camera3d>>().iter(world).collect();
    for entity in cameras { world.despawn(entity); }
    let mut image = Image::new_target_texture(IMAGE_SIDE, IMAGE_SIDE, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = world.resource_mut::<Assets<Image>>().add(image);
    world.spawn((
        Camera3d::default(),
        RenderTarget::from(target.clone()),
        Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() },
        Projection::Orthographic(OrthographicProjection {
            near: 0.1,
            far: 64.,
            scaling_mode: ScalingMode::Fixed { width: 32., height: 32. },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0., 24., 0.).looking_at(Vec3::new(0., 3.5, 0.), Vec3::NEG_Z),
        Tonemapping::None,
        Msaa::Off,
    )); // 同一固定俯视投影依次切换两份几何，非零水位也验证子块继承根变换。
    target
}

fn circular_surface() -> BuiltWaterSurface {
    let mut mask = vec![0; (IMAGE_SIDE * IMAGE_SIDE) as usize];
    for z in 0..IMAGE_SIDE {
        for x in 0..IMAGE_SIDE {
            let distance = Vec2::new(x as f32 + 0.5 - 128., z as f32 + 0.5 - 128.).length();
            mask[(z * IMAGE_SIDE + x) as usize] = (((90. - distance) / 14.).clamp(0., 1.) * 255.).round() as u8;
        }
    }
    let mesh = PaintedWaterMeshData::from_independent_mask(
        &mask, [IMAGE_SIDE; 2], [-16.; 2], [16.; 2], 3.5,
    ).unwrap(); // 直接使用独立水入口，不创建地形、R16或假的深度承托面。
    assert!(mesh.triangle_count() > 16_384, "回归必须真正越过单上传块容量");
    let layer: MapWaterLayer = serde_json::from_value(serde_json::json!({
        "id":"circular-independent", "label":"圆形独立水GPU回归",
        "mask_source_path":"self-contained.r8", "mask_texture_path":"self-contained.ktx2",
        "resolution":[IMAGE_SIDE,IMAGE_SIDE], "water_level":3.5,
        "base_color_srgba":[0.12,0.42,0.56,0.72]
    })).unwrap();
    let mut descriptor = WaterSurfaceDescriptor::from_layer(&layer);
    descriptor.interaction_enabled = false;
    BuiltWaterSurface { descriptor, mesh }
}

fn spawn_surface(app: &mut App, input: Vec<BuiltWaterSurface>, chunked: bool) -> PaintedWaterSurfaceSpawn {
    app.world_mut().resource_scope(|world, mut meshes: Mut<Assets<Mesh>>| {
        world.resource_scope(|world, mut materials: Mut<Assets<DynamicWaterMaterial>>| {
            let mut queue = CommandQueue::default();
            let mut commands = Commands::new(&mut queue, world);
            let mut staged = Vec::new();
            for surface in input {
                if chunked {
                    stage_water_surface_chunk(&mut commands, &mut meshes, &mut materials, &mut staged, surface, 0.);
                } else {
                    staged.push(stage_water_surface_assets(&mut meshes, &mut materials, surface.descriptor, surface.mesh, 0.));
                }
            }
            assert_eq!(staged.len(), 1, "同一水体的全部块必须由唯一根发布");
            let spawn = spawn_staged_water_surface(
                &mut commands, MapId::parse("MAP-independent-water-gpu").unwrap(),
                PaintedWaterPurpose::EditorPreview, staged.pop().unwrap(),
            );
            drop(commands);
            queue.apply(world);
            // 沿用正式DynamicWater材质和波形，只关闭受光以单独量测顶点alpha、透明混合与覆盖。
            materials.get_mut(&spawn.material).unwrap().base.unlit = true;
            spawn
        })
    })
}

fn pixels(app: &mut App, target: &Handle<Image>) -> Vec<u8> {
    let (send, receive) = std::sync::mpsc::channel();
    app.world_mut().spawn(Readback::texture(target.clone())).observe(
        move |event: On<ReadbackComplete>, mut commands: Commands| {
            let _ = send.send(event.data.clone());
            commands.entity(event.entity).try_despawn();
        },
    );
    let start = std::time::Instant::now();
    loop {
        app.update();
        if let Ok(bytes) = receive.try_recv() {
            assert_eq!(bytes.len(), (IMAGE_SIDE * IMAGE_SIDE * 4) as usize);
            return bytes;
        }
        assert!(start.elapsed().as_secs() < 15, "独立静水GPU图像回读超时");
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}

fn coverage(bytes: &[u8]) -> usize {
    bytes.chunks_exact(4).filter(|p| p[1] > 8 && p[2] > 12).count()
}

fn save_pixels(name: &str, bytes: &[u8]) {
    let output = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.local/validation");
    std::fs::create_dir_all(&output).unwrap();
    image::RgbaImage::from_raw(IMAGE_SIDE, IMAGE_SIDE, bytes.to_vec()).unwrap()
        .save(output.join(name)).unwrap();
}

#[test]
#[ignore = "需要真实GPU；无窗口对比无地形R8静水的整网格与正式分块颜色覆盖，并输出PNG"]
fn independent_painted_water_gpu_chunks_match_whole_mesh_pixels() {
    let mut app = gpu_app_with(|app| {
        // GltfPlugin只登记GLTF资产；完整水插件还消费WorldAsset生命周期消息，必须由正式宿主插件提供。
        app.add_plugins((bevy::world_serialization::WorldSerializationPlugin, LargeMapWaterPlugin));
        // 此夹具保留完整正式水插件及RenderApp，但不驱动玩家物理、水花和整网格作者任务。
        // 几何波仍由正式Shader执行；冻结时间保证先后两张图处于同一波形相位。
        app.configure_sets(Startup, SceneResidencyRuntimeSet.run_if(|| false))
            .configure_sets(Update, SceneResidencyRuntimeSet.run_if(|| false))
            .configure_sets(PostUpdate, SceneResidencyRuntimeSet.run_if(|| false))
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::ZERO));
    });
    let target = render_target(&mut app);
    let source = circular_surface();
    let triangles = source.mesh.triangle_count();
    let pieces = chunk_water_surfaces(vec![source.clone()]).unwrap();
    let piece_count = pieces.len();
    assert!(piece_count > 1);
    assert_eq!(pieces.iter().map(|piece| piece.mesh.triangle_count()).sum::<usize>(), triangles);
    let whole = spawn_surface(&mut app, vec![source], false);
    let chunked = spawn_surface(&mut app, pieces, true);
    assert_eq!(chunked.additional_entities.len() + 1, piece_count);
    for &child in &chunked.additional_entities {
        assert!(app.world().get::<WaterRenderChunk>(child).is_some());
        assert_eq!(app.world().get::<MeshMaterial3d<DynamicWaterMaterial>>(child).unwrap().0, chunked.material);
    }
    app.world_mut().entity_mut(chunked.entity).insert(Visibility::Hidden);
    gpu_frames(&mut app, 90);
    let whole_pixels = pixels(&mut app, &target);
    save_pixels("independent-water-whole.png", &whole_pixels);
    assert!(coverage(&whole_pixels) > 20_000, "整网格必须绘制实际圆形水域，不能黑图对黑图通过");

    app.world_mut().entity_mut(whole.entity).insert(Visibility::Hidden);
    app.world_mut().entity_mut(chunked.entity).insert(Visibility::Inherited);
    gpu_frames(&mut app, 60);
    let chunk_pixels = pixels(&mut app, &target);
    save_pixels("independent-water-chunked.png", &chunk_pixels);
    let mut max_delta = 0;
    let mut differing_pixels = 0;
    for (whole, chunk) in whole_pixels.chunks_exact(4).zip(chunk_pixels.chunks_exact(4)) {
        let delta = whole.iter().zip(chunk).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        max_delta = max_delta.max(delta);
        if delta > 2 { differing_pixels += 1; }
    }
    assert_eq!(differing_pixels, 0,
        "上传分块不能引入色阶硬边或缺面：块数={piece_count}，三角数={triangles}，差异像素={differing_pixels}，最大通道差={max_delta}");

    // 负对照主动只保留首块，证明同一投影和像素断言能够发现漏掉WaterRenderChunk的覆盖缺损。
    for &child in &chunked.additional_entities {
        app.world_mut().entity_mut(child).insert(Visibility::Hidden);
    }
    gpu_frames(&mut app, 8);
    let first_chunk_pixels = pixels(&mut app, &target);
    save_pixels("independent-water-first-chunk-only.png", &first_chunk_pixels);
    assert!(coverage(&first_chunk_pixels) + 5_000 < coverage(&whole_pixels),
        "首块负对照必须显著失去圆形覆盖，回归不能因错误相机或旧帧而失去辨识能力");
    eprintln!("独立静水GPU：三角数={triangles}，上传块={piece_count}，完整/分块/首块覆盖={}/{}/{}，最大通道差={max_delta}",
        coverage(&whole_pixels), coverage(&chunk_pixels), coverage(&first_chunk_pixels));
}
