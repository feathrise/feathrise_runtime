//! 真Window目标与正式画质插件重复切四球；每档等待真实GPU像素，不用静态RTT代替窗口MSAA竞争。
use super::*;
use bevy::{
    core_pipeline::prepass::{DepthPrepass, NormalPrepass},
    prelude::*,
    render::{
        settings::WgpuSettings,
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    winit::WinitPlugin,
};
use feathrise_settings::{
    AntiAliasingMode, BloomQuality, GraphicsRuntimeLeaseRegistry, GraphicsRuntimeLeaseSystems,
    GraphicsSettingsPlugin, RenderSettings,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

// 首次与再次取得预通道都覆盖；None是F12归还真实工程相机，不仅是切回渲染球。
const MODES: [Option<u8>; 11] = [
    Some(3),
    Some(0),
    Some(1),
    Some(2),
    Some(3),
    None,
    Some(0),
    Some(2),
    Some(1),
    Some(3),
    None,
];

#[derive(Default)]
struct Pixels {
    accepted: bool,
    red: usize,
    black: usize,
    gray: usize,
    neutral_background: usize,
    top_edges: usize,
    diagnostic: serde_json::Value,
}

fn neutral_background([r, g, b, _]: [u8; 4]) -> bool {
    let minimum = r.min(g).min(b);
    let maximum = r.max(g).max(b);
    minimum >= 30 && maximum <= 120 && maximum - minimum <= 12
}

#[derive(Resource)]
struct Proof {
    camera: Entity,
    builtin: bool,
    atmosphere: bool,
    step: usize,
    frames: usize,
    started: Instant,
    since: Instant,
    waiting: bool,
    reported_failure: bool,
    result: Arc<Mutex<Option<Result<Pixels, String>>>>,
    output: PathBuf,
    completed: Arc<Mutex<usize>>,
}

fn requested(mode: Option<u8>, builtin: bool) -> Option<ShadingView> {
    mode.map(|mode| ShadingView {
        options: Vec4::new(f32::from(mode), 1., 0., 1.),
        wire: Vec4::new(if builtin { 1. } else { 0. }, 1., 0., 0.),
    })
}

fn declare(world: &mut World) {
    let proof = world.resource::<Proof>();
    if let Some(mode) = MODES.get(proof.step) {
        apply_shading(world, proof.camera, requested(*mode, proof.builtin));
    }
}

fn drive(world: &mut World) {
    world.resource_scope(|world, mut proof: Mut<Proof>| {
        assert!(proof.started.elapsed().as_secs() < 180, "四球窗口GPU回归总超时");
        assert!(proof.since.elapsed().as_secs() < 35, "四球窗口GPU档位{}未产生真实像素", proof.step);
        let Some(&mode) = MODES.get(proof.step) else { return; };
        let needs_prepass = mode.is_some_and(|mode| mode <= 1);
        let expected_msaa = if needs_prepass { Msaa::Off } else { Msaa::Sample4 };
        assert_eq!(world.get::<Msaa>(proof.camera), Some(&expected_msaa), "正式窗口画质不得覆盖四球租约");
        assert_eq!(world.get::<DepthPrepass>(proof.camera).is_some(), needs_prepass);
        assert_eq!(world.get::<NormalPrepass>(proof.camera).is_some(), needs_prepass);
        assert_eq!(world.get::<PrepassLease>(proof.camera).is_some(), needs_prepass);
        assert_eq!(world.resource::<GraphicsRuntimeLeaseRegistry>().active_lease_count(), usize::from(needs_prepass));
        let result = proof.result.try_lock().ok().and_then(|mut result| result.take());
        if let Some(result) = result {
            let pixels = result.expect("后台四球截图分析与保存失败");
            proof.waiting = false;
            proof.frames = 0;
            if pixels.accepted {
                eprintln!("four_spheres_window step={} mode={mode:?} msaa={expected_msaa:?} red={} black={} gray={} background={} top_edges={}",
                    proof.step, pixels.red, pixels.black, pixels.gray, pixels.neutral_background, pixels.top_edges);
                proof.step += 1;
                proof.since = Instant::now();
                proof.reported_failure = false;
                *proof.completed.lock().unwrap() = proof.step;
                if proof.step == MODES.len() {
                    assert!(world.get::<ShadingView>(proof.camera).is_none());
                    world.write_message(AppExit::Success);
                }
                return;
            }
            if !proof.reported_failure {
                eprintln!("four_spheres_window_rejected step={} mode={mode:?} red={} black={} top_edges={} evidence={}",
                    proof.step,pixels.red,pixels.black,pixels.top_edges,pixels.diagnostic);
                proof.reported_failure = true;
            }
        }
        if proof.waiting { return; }
        proof.frames += 1;
        if proof.frames < 24 { return; }
        proof.waiting = true;
        let mailbox = proof.result.clone();
        let path = proof.output.join(format!("{:02}-{}.png", proof.step, mode.map_or("play".into(), |mode|mode.to_string())));
        let atmosphere = proof.atmosphere;
        let top_edges = if atmosphere && mode == Some(0) {
            projected_top_edges(world, proof.camera)
        } else { Vec::new() };
        let surface_point = atmosphere.then(|| world.get::<Camera>(proof.camera).unwrap()
            .world_to_viewport(world.get::<GlobalTransform>(proof.camera).unwrap(),Vec3::new(9.13,2.,-2.17)).unwrap());
        world.spawn(Screenshot::primary_window()).observe(move |shot: On<ScreenshotCaptured>| {
            let image = shot.image.clone();
            let mailbox = mailbox.clone();
            let path = path.clone();
            let top_edges = top_edges.clone();
            // 单份截图在途；全像素扫描和PNG压缩离开帧关键路径。
            std::thread::Builder::new().name("author-view-gpu-proof".into()).spawn(move || {
                let result = (|| {
                    let image = image.try_into_dynamic().map_err(|error|error.to_string())?.to_rgba8();
                    let mut pixels = Pixels::default();
                    for pixel in image.pixels() {
                        let [r, g, b, _] = pixel.0;
                        pixels.red += usize::from(r > 100 && r > g.saturating_mul(2) && r > b.saturating_mul(2));
                        pixels.black += usize::from(r <= 28 && g <= 28 && b <= 28);
                        pixels.gray += usize::from(r >= 120 && r.abs_diff(g) < 12 && r.abs_diff(b) < 12);
                        pixels.neutral_background += usize::from(neutral_background(pixel.0));
                    }
                    let total = image.width() as usize * image.height() as usize;
                    // 实际场景的网格也含深色像素；只数黑色会把“只有网格、没有方块”误判通过。
                    // 顶面四条真实边必须各自覆盖至少5个投影采样，不能靠整图hash或背景颜色过关。
                    let mut edges = Vec::new();
                    for points in &top_edges {
                        let mut samples = Vec::new();
                        let mut matched = 0;
                        for point in points {
                            let [x,y] = [point.x.round() as i32,point.y.round() as i32];
                            let inside = x >= 0 && y >= 0 && x < image.width() as i32 && y < image.height() as i32;
                            let center = inside.then(||image.get_pixel(x as u32,y as u32).0);
                            let mut nearest = [255_u8;4];
                            for dy in -2..=2 { for dx in -2..=2 {
                                let [sx,sy] = [x+dx,y+dy];
                                if sx >= 0 && sy >= 0 && sx < image.width() as i32 && sy < image.height() as i32 {
                                    let pixel = image.get_pixel(sx as u32,sy as u32).0;
                                    if pixel[..3].iter().max() < nearest[..3].iter().max() { nearest = pixel; }
                                }
                            }}
                            let hit = inside && nearest[..3].iter().all(|channel| *channel <= 28);
                            matched += usize::from(hit);
                            samples.push(serde_json::json!({"position":point.to_array(),"inside":inside,"rgba":center,
                                "nearest_rgba":nearest,"matched":hit}));
                        }
                        pixels.top_edges += usize::from(matched >= 5);
                        edges.push(serde_json::json!({"samples":samples,"matched":matched}));
                    }
                    pixels.diagnostic = serde_json::json!({"width":image.width(),"height":image.height(),"edges":edges,
                        "matched_edges":pixels.top_edges,"red":pixels.red,"black":pixels.black});
                    let surface_gray = surface_point.is_some_and(|point| {
                        let [r,g,b,_] = image.get_pixel(point.x.round() as u32,point.y.round() as u32).0;
                        pixels.diagnostic["surface"] = serde_json::json!({"position":point.to_array(),"rgb":[r,g,b]});
                        r >= 120 && r.abs_diff(g) < 12 && r.abs_diff(b) < 12
                    });
                    pixels.accepted = match mode {
                        // 原网格的红X轴必须继续存在；逐边门只检查真实方块，不把全图零红色作为前提。
                        Some(0) if atmosphere => pixels.top_edges == 4,
                        Some(1) if atmosphere => surface_gray,
                        // 原shader画黑线/中性灰背景；黑清屏或只有灰清屏都不能冒充线框。
                        Some(0) => pixels.red == 0 && pixels.black > 10 && pixels.black < total / 4
                            && pixels.neutral_background > total / 2 && neutral_background(image.get_pixel(0, 0).0),
                        Some(1) => pixels.red < 100 && pixels.gray > 200,
                        _ => pixels.red > 1000,
                    };
                    if pixels.accepted || atmosphere { image.save(&path).map_err(|error|error.to_string())?; }
                    if atmosphere {
                        std::fs::write(path.with_extension("json"),serde_json::to_vec_pretty(&pixels.diagnostic).unwrap())
                            .map_err(|error|error.to_string())?;
                    }
                    Ok(pixels)
                })();
                *mailbox.lock().unwrap() = Some(result);
            }).expect("创建有界截图分析线程");
        });
    });
}

#[test]
#[ignore = "真实Window+GPU；单项运行，FEATHRISE_AUTHOR_VIEW_GPU_DIR可指定11张连续切换截图目录"]
fn window_msaa_four_spheres_switch_repeatedly_and_restore_game_camera() {
    run_window_proof(false, false);
}

#[test]
#[ignore = "真实Window+GPU；正式新增方块的UV1真实面边+穿透线框，不能用普通Cuboid代替"]
fn window_msaa_formal_builtin_four_spheres_preserve_real_face_edges() {
    run_window_proof(true, false);
}

#[test]
#[ignore = "真实Window+GPU；复现正式游戏大气、近俯视透视相机与网格，逐边核对方块线框"]
fn window_msaa_formal_builtin_with_atmosphere_preserves_projected_edges() {
    run_window_proof(true, true);
}

fn output_directory(executable: &std::path::Path, explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = explicit { return Ok(path); }
    // 从本次测试 EXE 找实际 Cargo 缓存；单包归档没有 crates/父目录，不能按源码层级猜工作区。
    let target = executable.ancestors().skip(1).take(6)
        .find(|path| path.join(".rustc_info.json").is_file())
        .ok_or("无法定位本次测试的已有 target；请显式设置 FEATHRISE_AUTHOR_VIEW_GPU_DIR")?;
    Ok(target.join("validation/author-view-window-msaa"))
}

#[test]
fn standalone_sdk_output_stays_owned_without_an_editor_checkout() {
    let root = std::env::temp_dir().join(format!("feathrise-sdk-output-{}", bevy::asset::uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let sdk = root.join("sdk-only");
    let target = sdk.join("target");
    std::fs::create_dir_all(target.join("debug/deps")).unwrap();
    std::fs::write(target.join(".rustc_info.json"), "{}").unwrap();
    let executable = target.join("debug/deps/scene-tests.exe");
    let output = output_directory(&executable, None).unwrap();
    std::fs::create_dir_all(&output).unwrap();
    assert!(output.canonicalize().unwrap().starts_with(target.canonicalize().unwrap()));
    assert!(!root.join("feathrise").exists());
    let explicit = root.join("selected-evidence");
    // 安装包或单包源码位于其他目录时，只有实际测试缓存影响默认输出；显式取证目录仍优先。
    let outside_cache = root.join("unpacked-crate/tests.exe");
    assert!(output_directory(&outside_cache, None).is_err());
    assert_eq!(output_directory(&outside_cache, Some(explicit.clone())).unwrap(), explicit);
    // 此处只清理由上面 create_dir 独占创建的临时夹具，生产 target 不参与清理。
    assert!(root.starts_with(std::env::temp_dir()));
    std::fs::remove_dir_all(root).unwrap();
}

fn run_window_proof(builtin: bool, atmosphere: bool) {
    let output = output_directory(&std::env::current_exe().unwrap(),
        std::env::var_os("FEATHRISE_AUTHOR_VIEW_GPU_DIR").map(PathBuf::from)).unwrap();
    let output = if atmosphere { output.join("formal-atmosphere") }
        else if builtin { output.join("formal-builtin") } else { output };
    std::fs::create_dir_all(&output).unwrap();
    let mut gpu = WgpuSettings::default();
    let required = bevy::render::render_resource::WgpuFeatures::POLYGON_MODE_LINE
        | bevy::render::render_resource::WgpuFeatures::IMMEDIATES;
    gpu.features |= required;
    if let Some(disabled) = gpu.disabled_features.as_mut() {
        disabled.remove(required);
    }
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WinitPlugin {
                run_on_any_thread: true,
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "四球窗口MSAA动态切换验收".into(),
                    // 使用live9视口宽高比；固定DPI使投影点与GPU回读像素同域，不能把裁出窗口当成缺线。
                    resolution: if atmosphere {
                        bevy::window::WindowResolution::new(748, 576).with_scale_factor_override(1.)
                    } else { (384, 384).into() },
                    focused: false,
                    ..default()
                }),
                ..default()
            })
            .set(bevy::render::RenderPlugin {
                render_creation: gpu.into(),
                ..default()
            }),
    )
    .add_plugins((GraphicsSettingsPlugin, ShadingRenderPlugin, GridRenderPlugin))
    .insert_resource(bevy::render::error_handler::RenderErrorHandler(
        |error, _, _| panic!("四球窗口GPU验证失败：{error:?}"),
    ))
    .add_systems(
        PostUpdate,
        declare.before(GraphicsRuntimeLeaseSystems::RestoreReleased),
    )
    .add_systems(Last, drive);
    {
        let mut settings = app.world_mut().resource_mut::<RenderSettings>();
        settings.graphics.anti_aliasing = AntiAliasingMode::Msaa4;
        settings.graphics.bloom_quality = BloomQuality::Off;
    }
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Msaa::Sample4,
            Camera {
                clear_color: bevy::camera::ClearColorConfig::Custom(Color::srgb(0.04, 0.05, 0.06)),
                ..default()
            },
            if atmosphere {
                // 正式取消数值拖动后的物体中心是x=9；围绕该中心构图，避免相机与左侧面x=8共面，
                // 原无限网格半覆盖恰好叠在同一整条投影边上，令底层黑线变成约RGB40的合成灰线。
                Transform::from_xyz(9., 8.883598, -0.6402626).looking_at(Vec3::new(9.,1.,-2.),Vec3::Y)
            } else { Transform::from_xyz(4., 3., 6.).looking_at(Vec3::ZERO, Vec3::Y) },
        ))
        .id();
    assert!(matches!(
        app.world().get::<bevy::camera::RenderTarget>(camera),
        Some(bevy::camera::RenderTarget::Window(_))
    ));
    if builtin {
        let root = spawn_formal_builtin(app.world_mut());
        if atmosphere {
            app.world_mut().entity_mut(root).insert(Transform::from_xyz(9.,1.,-2.));
            install_atmosphere_scene(app.world_mut(), camera);
        }
    } else {
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::new(2., 2., 2.));
    let material = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: Color::srgb(0.8, 0.1, 0.05),
            unlit: true,
            ..default()
        });
    app.world_mut()
        .spawn((Mesh3d(mesh), MeshMaterial3d(material)));
    }
    let completed = Arc::new(Mutex::new(0));
    app.insert_resource(Proof {
        camera,
        builtin,
        atmosphere,
        step: 0,
        frames: 0,
        started: Instant::now(),
        since: Instant::now(),
        waiting: false,
        reported_failure: false,
        result: Arc::default(),
        output,
        completed: completed.clone(),
    });
    assert_eq!(app.run(), AppExit::Success, "GPU错误或提前关闭必须失败");
    assert_eq!(
        *completed.lock().unwrap(),
        MODES.len(),
        "每档须取得真实截图，最后恢复玩法Sample4"
    );
}

fn spawn_formal_builtin(world: &mut World) -> Entity {
    use bevy::ecs::system::RunSystemOnce;
    use feathrise_map_content::{MapId, MapPlacementAssetTarget, MapPlacementRecord};
    let target = MapPlacementAssetTarget::Cuboid {
        size_mm: [2000; 3], color_srgb: [204, 25, 13], roughness_milli: 700, collision: false,
    };
    let record: MapPlacementRecord = serde_json::from_value(serde_json::json!({
        "id":"author-view-gpu-builtin", "asset_path":"", "asset_target":target,
        "translation":[0,0,0], "yaw_degrees":0, "scale":[1,1,1]
    })).unwrap();
    let root = world.spawn((
        crate::MapPlacementInstance::new(&MapId::parse("MAP-author-view-gpu").unwrap(), &record),
        Transform::IDENTITY, Visibility::Inherited,
    )).id();
    world.run_system_once(move |mut commands: Commands| {
        assert!(crate::spawn_builtin_placement_visual(&mut commands, root, target));
    }).unwrap();
    // 沿正式作者目标生成Mesh与StandardMaterial，必须实际带真实面边编码才覆盖本次缺失路径。
    let visual = world.query_filtered::<&Mesh3d, With<crate::MapPlacementPrimitiveVisual>>()
        .single(world).unwrap();
    let mesh = world.resource::<Assets<Mesh>>().get(&visual.0).unwrap();
    assert!(mesh.attribute(crate::ATTRIBUTE_AUTHORING_WIRE).is_some());
    assert!(mesh.attribute(Mesh::ATTRIBUTE_UV_1).is_some());
    world.spawn((DirectionalLight { illuminance: 20_000., ..default() },
        Transform::from_xyz(4., 7., 5.).looking_at(Vec3::ZERO, Vec3::Y)));
    root
}

fn install_atmosphere_scene(world: &mut World, camera: Entity) {
    use bevy::light::{Atmosphere, atmosphere::ScatteringMedium};
    // 与正式动态天空相同的原生Earth大气，不用Skybox替代其opaque/transparent之间的合成。
    let medium = world.resource_mut::<Assets<ScatteringMedium>>().add(ScatteringMedium::earth(256,256));
    world.spawn(Atmosphere::earth(medium));
    world.entity_mut(camera).insert((bevy::pbr::AtmosphereSettings::default(), GridView {
        origin_stride: Vec4::new(0.,0.,0.,1.), plane: Vec4::new(0.,1.,0.,1.015),
        axis_u: Vec4::X, axis_v: Vec4::Z,
    }));
}

fn projected_top_edges(world: &World, camera: Entity) -> Vec<Vec<Vec2>> {
    let camera_view = world.get::<Camera>(camera).unwrap();
    let transform = world.get::<GlobalTransform>(camera).unwrap();
    let corners = [Vec3::new(8.,2.,-3.),Vec3::new(10.,2.,-3.),
        Vec3::new(10.,2.,-1.),Vec3::new(8.,2.,-1.)];
    (0..4).map(|edge| (1..=7).map(|sample| {
        camera_view.world_to_viewport(transform,
            corners[edge].lerp(corners[(edge+1)%4], sample as f32/8.)).unwrap()
    }).collect()).collect()
}
