//! 模型密集场景的离屏 GPU 对照。每段保留同一相机、材质、场景和有效绘制范围。
use super::{resources::MeshLodRuntime, tests, *};
use bevy::{
    camera::RenderTarget,
    render::{
        RenderApp, RenderPlugin,
        gpu_readback::{Readback, ReadbackComplete},
        render_resource::{TextureFormat, TextureUsages, WgpuFeatures},
        renderer::{RenderAdapterInfo, RenderDevice},
        settings::{RenderCreation, WgpuSettings},
    },
};
use feathrise_settings::RenderSettings;
use serde_json::{Value, json};
use std::{collections::HashSet, time::{Duration, Instant}};

const ROW: usize = 8;
const WIDTH: u32 = 1024;
const HEIGHT: u32 = 768;
const SAMPLES: usize = 60;

#[test]
#[ignore = "显式的真实 GPU 性能实验，无原生窗口，不访问作者工程"]
fn repeated_dense_placement_meshes_gpu_ab() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins
        .set(WindowPlugin { primary_window: None, exit_condition: bevy::window::ExitCondition::DontExit, ..default() })
        .set(RenderPlugin { render_creation: RenderCreation::Automatic(Box::new(WgpuSettings {
            features: WgpuFeatures::IMMEDIATES, ..default()
        })), synchronous_pipeline_compilation: true, ..default() })
        .disable::<bevy::winit::WinitPlugin>()
        .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>());
    app.init_resource::<RenderSettings>();
    install(&mut app);
    app.finish();
    app.cleanup();
    let adapter = app.sub_app(RenderApp).world().resource::<RenderAdapterInfo>();
    let adapter = json!({"name":adapter.name,"backend":format!("{:?}",adapter.backend)});

    let world = app.world_mut();
    let mut target = Image::new_target_texture(WIDTH, HEIGHT, TextureFormat::Rgba8UnormSrgb, None);
    target.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = world.resource_mut::<Assets<Image>>().add(target);
    world.spawn((Camera3d::default(), RenderTarget::from(target.clone()),
        Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() },
        Transform::from_xyz(0.0, 36.0, 0.0).looking_at(Vec3::ZERO, Vec3::NEG_Z), Msaa::Off));
    let mesh = Plane3d::default().mesh().size(2.0, 2.0).subdivisions(223).build();
    let vertex_count = mesh.count_vertices();
    let original = world.resource_mut::<Assets<Mesh>>().add(mesh
        .with_inserted_attribute(crate::ATTRIBUTE_AUTHORING_WIRE, vec![0.0; vertex_count]));
    let material = world.resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial {
        base_color: Color::srgb(0.23, 0.55, 0.85), unlit: true, ..default()
    });
    let mut visuals = Vec::with_capacity(ROW * ROW);
    for z in 0..ROW {
        for x in 0..ROW {
            let root = tests::owner_root(world, false);
            let coordinate = |index: usize| (index as f32 - (ROW - 1) as f32 * 0.5) * 2.45;
            world.entity_mut(root).insert((Transform::from_xyz(coordinate(x), 0.0, coordinate(z)), Visibility::Inherited));
            visuals.push(world.spawn((Mesh3d(original.clone()), MeshMaterial3d(material.clone()),
                Transform::IDENTITY, ChildOf(root))).id());
        }
    }
    world.resource_mut::<RenderSettings>().mesh_lod.collision_enabled = false;
    let off = compare(&mut app, &target, &visuals, false);
    let on = compare(&mut app, &target, &visuals, true);
    let off_again = compare(&mut app, &target, &visuals, false);
    assert_eq!(off["reduced_models"], 0);
    assert_eq!(off_again["submitted_triangles"], off["submitted_triangles"]);
    assert!(on["submitted_triangles"].as_u64() < off["submitted_triangles"].as_u64());
    assert_eq!(on["reduced_models"], visuals.len());
    assert_eq!(on["distinct_display_meshes"], 1);
    for phase in [&off, &on, &off_again] {
        let baseline = off["painted_pixels"].as_u64().unwrap();
        assert!(baseline > 50_000);
        assert!(baseline.abs_diff(phase["painted_pixels"].as_u64().unwrap()) < baseline / 100);
    }
    // 性能测量只输出数据，避免把机器负载抖动误当作 CI 几何回归。
    println!("模型LOD GPU帧时间 {}", json!({
        "scope":"offscreen shaded, CPU submission and GPU completion, no present pacing",
        "size":[WIDTH,HEIGHT], "models":visuals.len(), "adapter":adapter,
        "original":off, "lod":on, "original_repeat":off_again,
    }));
}

fn frame(app: &mut App) -> f64 {
    let start = Instant::now();
    app.update();
    // 离屏没有窗口刷新率限制。只在这个测试等待GPU，避免把提交速度误解为渲染速度。
    app.world().resource::<RenderDevice>().wgpu_device().poll(wgpu::PollType::Wait {
        submission_index: None, timeout: Some(Duration::from_secs(10)),
    }).expect("模型LOD离屏帧的GPU提交未结束");
    start.elapsed().as_secs_f64() * 1000.0
}

fn compare(app: &mut App, target: &Handle<Image>, visuals: &[Entity], enabled: bool) -> Value {
    app.world_mut().resource_mut::<RenderSettings>().mesh_lod.display_enabled = enabled;
    let start = Instant::now();
    let mut settled = 0;
    while settled < 30 {
        frame(app);
        let state = app.world().resource::<MeshLodRuntime>();
        let idle = state.targets.len() == visuals.len()
            && (state.chunk_coarse.is_empty() != enabled)
            && state.capture.is_none() && state.task.is_none() && state.composition.is_none()
            && state.release.is_none() && state.retired.is_empty();
        settled = if idle { settled + 1 } else { 0 };
        assert!(start.elapsed() < Duration::from_secs(80), "模型LOD未在计时前收口");
    }
    let (send, receive) = std::sync::mpsc::channel();
    app.world_mut().spawn(Readback::texture(target.clone()))
        .observe(move |event: On<ReadbackComplete>, mut commands: Commands| {
            let _ = send.send(event.data.clone());
            commands.entity(event.entity).try_despawn();
        });
    let pixels = loop {
        frame(app);
        if let Ok(bytes) = receive.try_recv() { break bytes; }
        assert!(start.elapsed() < Duration::from_secs(90), "模型GPU回读无结果");
    };
    let painted_pixels = pixels.chunks_exact(4).filter(|rgba| rgba[2] > 90).count();
    for _ in 0..8 { frame(app); } // 等回读任务退场，保持计时不含取证开销。
    let mut times: Vec<_> = (0..SAMPLES).map(|_| frame(app)).collect();
    times.sort_by(f64::total_cmp);
    let median = (times[SAMPLES / 2 - 1] + times[SAMPLES / 2]) * 0.5;
    let world = app.world();
    let meshes = world.resource::<Assets<Mesh>>();
    let mut distinct = HashSet::new();
    let submitted: usize = visuals.iter().map(|&entity| {
        assert!(world.get::<ViewVisibility>(entity).unwrap().get());
        let handle = &world.get::<Mesh3d>(entity).unwrap().0;
        distinct.insert(handle.id());
        meshes.get(handle).unwrap().indices().unwrap().len() / 3
    }).sum();
    let state = world.resource::<MeshLodRuntime>();
    json!({"samples":SAMPLES,"median_ms":median,"p95_ms":times[SAMPLES * 95 / 100 - 1],
        "submitted_triangles":submitted,"reduced_models":state.chunk_coarse.len(),
        "distinct_display_meshes":distinct.len(),"cache_bytes":state.cache_bytes,"painted_pixels":painted_pixels})
}
