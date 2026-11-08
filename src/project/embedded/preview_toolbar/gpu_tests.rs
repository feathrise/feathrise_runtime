//! 显式离屏GPU验收：真实预览HUD按钮、模型三角线框与Pointer拖拽；只使用本轮内存夹具。

use super::{components::*, resources::PreviewToolbarState, systems};
use crate::project::embedded::EmbeddedProjectState;
use avian3d::debug_render::PhysicsGizmos;
use bevy::{
    camera::RenderTarget,
    gizmos::config::GizmoConfigStore,
    picking::{backend::HitData, events::{Pointer, PointerDrag, PointerDragEnd, PointerDragStart}, pointer::{Location, PointerButton, PointerId}},
    prelude::*,
    render::{RenderApp, gpu_readback::{Readback, ReadbackComplete},
        render_resource::{CachedPipelineState, Extent3d, PipelineCache, TextureDimension, TextureFormat, TextureUsages, WgpuFeatures},
        settings::WgpuSettings},
};
use feathrise_editor_protocol::Mode;
use feathrise_scene::author_view::{GridView, PrepassLease, ShadingRenderPlugin, ShadingView};
use feathrise_scene_core::view::PrimarySceneView;
use std::{fs::OpenOptions, path::Path, time::{Duration, Instant}};

#[derive(Resource, Default)]
struct NextPress(Option<Entity>);

// 无OS窗口的夹具在原interact之前投递一帧Interaction，真实业务系统负责状态和显示租约。
fn inject_press(mut next: ResMut<NextPress>, mut buttons: Query<(Entity, &mut Interaction), With<ToolbarAction>>) {
    let pressed = next.0.take();
    for (entity, mut interaction) in &mut buttons {
        *interaction = if Some(entity) == pressed { Interaction::Pressed } else { Interaction::None };
    }
}

fn press(app: &mut App, action: ToolbarAction) {
    let world = app.world_mut();
    let entity = world.query::<(Entity, &ToolbarAction)>().iter(world)
        .find_map(|(entity, current)| (*current == action).then_some(entity)).unwrap();
    world.resource_mut::<NextPress>().0 = Some(entity);
    frame(app);
}

fn frame(app: &mut App) {
    app.update();
    let world = app.world_mut();
    assert_eq!(world.query::<&GridView>().iter(world).count(), 0, "每帧都不得安装无限参考网格");
    std::thread::sleep(Duration::from_millis(10));
}

fn ready(app: &mut App) {
    let started = Instant::now();
    let mut stable = 0;
    loop {
        frame(app);
        let cache = app.sub_app(RenderApp).world().resource::<PipelineCache>();
        let mut pending = cache.waiting_pipelines().next().is_some();
        for pipeline in cache.pipelines() {
            if let CachedPipelineState::Err(error) = &pipeline.state {
                assert!(matches!(error, bevy::shader::ShaderCacheError::ShaderNotLoaded(_)
                    | bevy::shader::ShaderCacheError::ShaderImportNotYetAvailable), "GPU管线失败：{error:?}");
                pending = true;
            }
        }
        stable = if !pending && cache.pipelines().next().is_some() { stable + 1 } else { 0 };
        if stable >= 4 { break; }
        assert!(started.elapsed() < Duration::from_secs(30), "预览HUD GPU管线等待超时");
    }
}

// 等待和PNG压缩只发生在显式测试驱动，生产帧系统不安装回读或遍历资产的逻辑。
fn capture(app: &mut App, target: &Handle<Image>, path: &Path) -> Vec<u8> {
    ready(app);
    let size = app.world().resource::<Assets<Image>>().get(target).unwrap().size();
    let (sender, receiver) = std::sync::mpsc::channel();
    app.world_mut().spawn(Readback::texture(target.clone()))
        .observe(move |event: On<ReadbackComplete>, mut commands: Commands| {
            let _ = sender.send(event.data.clone());
            commands.entity(event.entity).try_despawn();
        });
    let started = Instant::now();
    let bytes = loop {
        frame(app);
        if let Ok(bytes) = receiver.try_recv() { break bytes; }
        assert!(started.elapsed() < Duration::from_secs(15), "预览HUD GPU回读超时");
    };
    let stride = size.x as usize * 4;
    let padded = stride.div_ceil(256) * 256;
    assert!(bytes.len() >= padded * size.y as usize);
    let pixels = bytes.chunks_exact(padded).take(size.y as usize)
        .flat_map(|row| row[..stride].iter().copied()).collect::<Vec<_>>();
    let image = Image::new(Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
        TextureDimension::D2, pixels.clone(), TextureFormat::Rgba8UnormSrgb, default());
    let mut file = OpenOptions::new().write(true).create_new(true).open(path).unwrap();
    image.try_into_dynamic().unwrap().write_to(&mut file,
        bevy::image::ImageFormat::Png.as_image_crate_format().unwrap()).unwrap();
    pixels
}

fn rect(app: &App, entity: Entity) -> Rect {
    Rect::from_center_size(app.world().get::<UiGlobalTransform>(entity).unwrap().translation,
        app.world().get::<ComputedNode>(entity).unwrap().size())
}

fn scene_changes(before: &[u8], after: &[u8]) -> usize {
    // 排除顶部HUD；按钮底色变化不能冒充真实3D模型线框。
    before.chunks_exact(4).zip(after.chunks_exact(4)).enumerate()
        .filter(|(index, (before, after))| index / 800 >= 80 && before[..3] != after[..3]).count()
}

#[test]
#[ignore = "显式真实GPU离屏验收；必须指定独占FEATHRISE_PREVIEW_HUD_GPU_REPORT绝对目录"]
fn production_preview_hud_gpu_renders_triangles_restores_scene_and_drags() {
    let output = std::path::PathBuf::from(std::env::var_os("FEATHRISE_PREVIEW_HUD_GPU_REPORT")
        .expect("必须指定本轮独占FEATHRISE_PREVIEW_HUD_GPU_REPORT绝对目录"));
    assert!(output.is_absolute());
    std::fs::create_dir_all(&output).unwrap();
    let _owner = OpenOptions::new().write(true).create_new(true).open(output.join("owner"))
        .expect("本轮输出目录必须独占，禁止复用旧证据目录");
    // 夹具资产根归本轮独占输出；字体直接消费运行装配的公开字节，不读取其他包的源码目录。
    let assets = output.join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    let mut gpu = WgpuSettings::default();
    let required = WgpuFeatures::POLYGON_MODE_LINE | WgpuFeatures::IMMEDIATES;
    gpu.features |= required;
    if let Some(disabled) = gpu.disabled_features.as_mut() { disabled.remove(required); }
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.build()
        .set(WindowPlugin { primary_window: None, exit_condition: bevy::window::ExitCondition::DontExit, ..default() })
        .set(AssetPlugin { file_path: assets.to_string_lossy().into_owned(),
            watch_for_changes_override: Some(false), ..default() })
        .set(bevy::render::RenderPlugin { render_creation: gpu.into(), ..default() })
        .disable::<bevy::winit::WinitPlugin>()
        .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>()
        .disable::<bevy::audio::AudioPlugin>()
        .disable::<bevy::log::LogPlugin>())
        .add_plugins(ShadingRenderPlugin)
        .init_gizmo_group::<PhysicsGizmos>()
        .insert_resource(EmbeddedProjectState { mode: Mode::Editing, world_id: 91, revision: 7, transitioning: false })
        .init_resource::<NextPress>()
        .insert_resource(bevy::render::error_handler::RenderErrorHandler(
            |error, _, _| panic!("预览HUD GPU错误：{error:?}")))
        .add_systems(Update, inject_press.before(systems::interact));
    super::install(&mut app);
    app.world_mut().resource_mut::<GizmoConfigStore>().config_mut::<PhysicsGizmos>().0.enabled = false;
    let started = Instant::now();
    while app.plugins_state() != bevy::app::PluginsState::Ready {
        bevy::tasks::tick_global_task_pools_on_main_thread();
        assert!(started.elapsed() < Duration::from_secs(30), "GPU设备初始化超时");
        std::thread::sleep(Duration::from_millis(10));
    }
    app.finish(); app.cleanup();
    let font = app.world_mut().resource_mut::<Assets<Font>>().add(Font::from_bytes(
        crate::app::default_font::DEFAULT_FONT_BYTES.to_vec()));
    // Startup创建原UI之后，仅给夹具文字指定SDK正式中文字体。
    app.update();
    for mut text in app.world_mut().query::<&mut TextFont>().iter_mut(app.world_mut()) { text.font = font.clone().into(); }
    let mut image = Image::new_target_texture(800, 600, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    let camera = app.world_mut().spawn((PrimarySceneView, Camera3d::default(), Camera {
        clear_color: ClearColorConfig::Custom(Color::srgb(0.04, 0.05, 0.06)), ..default()
    }, RenderTarget::from(target.clone()), bevy::camera::Hdr, Msaa::Sample4,
        Transform::from_xyz(5., 4., 7.).looking_at(Vec3::new(0., 0.8, 0.), Vec3::Y))).id();
    let vertices = std::array::from_fn(|index| std::array::from_fn(|axis| if index & (1 << axis) == 0 { -2000 } else { 2000 }));
    let cube = feathrise_scene::editable_cuboid_mesh(vertices);
    assert!(cube.attribute(feathrise_scene::ATTRIBUTE_AUTHORING_WIRE).is_some());
    for (mesh, color, translation) in [(cube, Color::srgb(0.85, 0.18, 0.08), Vec3::Y),
        (Plane3d::default().mesh().size(8., 8.).build(), Color::srgb(0.20, 0.30, 0.18), Vec3::ZERO)] {
        let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
        let material = app.world_mut().resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial { base_color: color, unlit: true, ..default() });
        app.world_mut().spawn((Mesh3d(mesh), MeshMaterial3d(material), Transform::from_translation(translation)));
    }
    systems::mode_changed(app.world_mut(), Mode::Playing);
    app.world_mut().resource_mut::<EmbeddedProjectState>().mode = Mode::Playing;
    let normal = capture(&mut app, &target, &output.join("01-normal.png"));
    let world = app.world_mut();
    let root = world.query_filtered::<Entity, With<ToolbarRoot>>().single(world).unwrap();
    let handle = world.query_filtered::<Entity, With<ToolbarDragHandle>>().single(world).unwrap();
    let original = rect(&app, root);
    assert!(original.width() > 180. && original.width() < 600. && original.height() <= 40., "HUD不能占满顶部整行：{original:?}");
    assert!((original.center().x - 400.).abs() < 1. && (original.min.y - 12.).abs() < 1.,
        "HUD默认应在游戏视口顶部居中：{original:?}");
    assert_eq!(app.world().get::<UiTargetCamera>(root).unwrap().0, camera);
    assert_eq!(app.world_mut().query::<&ToolbarAction>().iter(app.world()).count(), 3);
    assert_eq!(app.world_mut().query::<&bevy::text::TextLayoutInfo>().iter(app.world())
        .filter(|layout| !layout.glyphs.is_empty()).count(), 3, "三个中文标签都必须完成实际字形布局");
    let viewport = app.world().get::<Camera>(camera).unwrap().physical_viewport_rect();
    assert_eq!(viewport, Some(URect::from_corners(UVec2::ZERO, UVec2::new(800, 600))));
    assert!(normal.chunks_exact(4).filter(|p| p[0] > 100 && p[0] > p[1].saturating_mul(2)).count() > 1000, "必须实际绘出红色立方体");
    press(&mut app, ToolbarAction::Grid);
    assert!(app.world().resource::<PreviewToolbarState>().grid);
    assert!(app.world().get::<ShadingView>(camera) == Some(&ShadingView::mesh_triangles()));
    assert!(app.world().get::<PrepassLease>(camera).is_some());
    let wire = capture(&mut app, &target, &output.join("02-model-triangles.png"));
    let changed = scene_changes(&normal, &wire);
    assert!(changed > 1000, "实际3D像素必须改变，不能只改变按钮或组件");
    // 顶面0→2对角线属于三角拓扑，作者四边形线框原本会隐藏它；逐点核对真实GPU黑线。
    let camera_view = app.world().get::<Camera>(camera).unwrap();
    let transform = app.world().get::<GlobalTransform>(camera).unwrap();
    let diagonal_hits = (1..8).filter(|sample| {
        let p = camera_view.world_to_viewport(transform, Vec3::new(-1., 2., -1.).lerp(Vec3::new(1., 2., 1.), *sample as f32 / 8.)).unwrap();
        (-2..=2).any(|dy| (-2..=2).any(|dx| {
            let index = (((p.y.round() as i32 + dy) * 800 + p.x.round() as i32 + dx) * 4) as usize;
            wire[index..index + 3].iter().all(|channel| *channel < 40)
        }))
    }).count();
    assert!(diagonal_hits >= 5, "必须显示真实模型顶面的三角对角线：{diagonal_hits}/7");
    press(&mut app, ToolbarAction::Collision);
    assert!(app.world().resource::<GizmoConfigStore>().config::<PhysicsGizmos>().0.enabled);
    press(&mut app, ToolbarAction::Collision);
    assert!(!app.world().resource::<GizmoConfigStore>().config::<PhysicsGizmos>().0.enabled);
    press(&mut app, ToolbarAction::Grid);
    assert!(app.world().get::<ShadingView>(camera).is_none() && app.world().get::<PrepassLease>(camera).is_none());
    assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Sample4));
    let restored = capture(&mut app, &target, &output.join("03-restored.png"));
    assert_eq!(scene_changes(&normal, &restored), 0, "关闭模型网格必须恢复同一3D画面");
    let location = Location { target: RenderTarget::from(target.clone()).normalize(None).unwrap(), position: rect(&app, handle).center() };
    let distance = Vec2::new(420., 480.);
    app.world_mut().trigger(PointerDragStart {
        entity: handle,
        pointer: Pointer::new(PointerId::Mouse, location.clone()),
        button: PointerButton::Primary, hit: HitData::new(camera, 0., None, None),
    });
    app.world_mut().trigger(PointerDrag {
        entity: handle,
        pointer: Pointer::new(PointerId::Mouse, location.clone()),
        button: PointerButton::Primary, distance, delta: distance,
    });
    app.world_mut().trigger(PointerDragEnd {
        entity: handle,
        pointer: Pointer::new(PointerId::Mouse, location),
        button: PointerButton::Primary, distance,
    });
    let dragged = capture(&mut app, &target, &output.join("04-dragged.png"));
    let moved = rect(&app, root);
    assert!(moved.min.distance(original.min) > 300. && moved.max.x <= 800. && moved.max.y <= 600.);
    assert_eq!(moved.size(), original.size());
    assert_eq!(app.world().get::<Camera>(camera).unwrap().physical_viewport_rect(), viewport);
    assert!(restored != dragged, "真实PNG必须体现HUD拖动");
    assert_eq!(app.world_mut().query::<&GridView>().iter(app.world()).count(), 0, "全流程不得安装无限参考网格");
    let mut report = OpenOptions::new().write(true).create_new(true).open(output.join("report.json")).unwrap();
    serde_json::to_writer_pretty(&mut report, &serde_json::json!({"passed":true,"source":"production-toolbar-and-ReadbackComplete",
        "world_id":91,"revision":7,"scene_changed_pixels":changed,"triangle_diagonal_samples":diagonal_hits,
        "hud_before":[original.min.to_array(),original.max.to_array()],"hud_after":[moved.min.to_array(),moved.max.to_array()],
        "viewport_unchanged":true,"grid_view_count":0,"os_input_verified":false})).unwrap();
    println!("FEATHRISE_PREVIEW_HUD_GPU_REPORT {}", output.display());
}
