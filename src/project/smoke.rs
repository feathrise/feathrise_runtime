//! 显式场景运行验收：中性视图、生产几何就绪门、源文件指纹与真实GPU截图；不替代玩家或战斗验收。

#[path = "smoke_author_tools.rs"]
mod author_tools;

use crate::project::{ProjectContext, context, startup::SavedGameStartup};
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
    tasks::{IoTaskPool, Task, futures::check_ready},
};
use feathrise_scene::{
    DynamicMapRuntimePhase, DynamicMapRuntimeStatus, DynamicVegetationPhase,
    DynamicVegetationStatus, DynamicWaterPhase, DynamicWaterStatus, MapPlacementRuntimePhase,
    MapPlacementRuntimeStatus,
};
use feathrise_scene_core::state::{ScenePhase, LoadingProgress, SceneStartupGate};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Resource)]
struct GameSmoke {
    directory: PathBuf,
    started: Instant,
    budget: Duration,
    stable: u8,
    capture_requested: bool,
    snapshot: Option<serde_json::Value>,
    source: serde_json::Value,
    writer: Option<Task<Result<(), String>>>,
    completed: bool,
    saved_play: bool,
    return_requested: Option<Instant>,
}

pub(super) fn install(
    app: &mut App,
    project: &ProjectContext,
    saved_play: bool,
) -> Result<(), String> {
    let Some(directory) = std::env::var_os("NEW_EDITOR_GAME_SMOKE_DIR").map(PathBuf::from) else {
        return Ok(());
    };
    if !directory.is_absolute() {
        return Err("NEW_EDITOR_GAME_SMOKE_DIR必须是绝对隔离输出目录".into());
    }
    fs::create_dir_all(&directory).map_err(|error| format!("运行验收输出目录不可用：{error}"))?;
    let directory = directory
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if project
        .asset_roots
        .iter()
        .any(|root| directory.starts_with(root))
        || directory == project.root
    {
        return Err("运行验收输出不得覆盖工程根或作者资产目录".into());
    }
    let startup = app.world().resource::<SavedGameStartup>();
    let source = source_evidence(project, &startup.entry)?;
    if author_tools::enabled() {
        author_tools::install(app)?;
    }
    // 有限验收不依赖窗口是否恰好获得前台；普通游戏仍使用自己的失焦更新策略。
    app.insert_resource(bevy::winit::WinitSettings::continuous());
    let budget = std::env::var("NEW_EDITOR_GAME_SMOKE_SECONDS")
        .ok()
        .map(|value| value.parse::<u64>().map_err(|_| "运行验收时限必须是秒数"))
        .transpose()?
        .unwrap_or(180)
        .clamp(5, 180);
    app.insert_resource(GameSmoke {
        directory,
        started: Instant::now(),
        budget: Duration::from_secs(budget),
        stable: 0,
        capture_requested: false,
        snapshot: None,
        source,
        writer: None,
        completed: false,
        saved_play,
        return_requested: None,
    })
    .add_systems(Last, advance);
    Ok(())
}

fn ready(world: &mut World) -> bool {
    let view_ready = world.query_filtered::<(&Camera, &Projection, &GlobalTransform, &bevy::camera::RenderTarget),
        (With<Camera3d>, With<feathrise_scene_core::view::PrimarySceneView>)>()
        .single(world).is_ok_and(|(camera, projection, transform, target)| {
            camera.is_active && transform.translation().is_finite()
                && matches!(target, bevy::camera::RenderTarget::Window(_))
                && match projection {
                    Projection::Perspective(value) => value.fov.is_finite() && value.fov > 0. && value.near > 0. && value.far > value.near,
                    Projection::Orthographic(value) => value.scale.is_finite() && value.scale > 0. && value.far > value.near,
                    Projection::Custom(_) => false,
                }
        });
    let geometry_ready = world.query_filtered::<Entity, With<Mesh3d>>().iter(world).next().is_some();
    let startup = world.resource::<SavedGameStartup>();
    let ready = world.get_resource::<State<ScenePhase>>().is_some_and(|state| *state.get() == ScenePhase::Active)
        && view_ready && geometry_ready
        // 加载门完成后可被移除；仍须真实中性窗口视图、可视网格及正式地图各域Ready。
        && world.get_resource::<SceneStartupGate>().is_none_or(|gate| gate.target == ScenePhase::Active && gate.ready && gate.collision_ready && gate.scene_assets_ready && gate.application_ready && gate.application_error.is_none())
        && !world.contains_resource::<LoadingProgress>()
        && world.get_resource::<DynamicMapRuntimeStatus>().is_some_and(|state| state.map_id.as_ref() == Some(&startup.map_id) && state.phase == DynamicMapRuntimePhase::Ready)
        && world.get_resource::<MapPlacementRuntimeStatus>().is_some_and(|state| state.map_id.as_ref() == Some(&startup.map_id) && state.phase == MapPlacementRuntimePhase::Ready)
        && world.get_resource::<DynamicVegetationStatus>().is_some_and(|state| state.map_id.as_ref() == Some(&startup.map_id) && state.phase == DynamicVegetationPhase::Ready)
        && world.get_resource::<DynamicWaterStatus>().is_none_or(|state| state.phase == DynamicWaterPhase::Disabled || (state.map_id.as_ref() == Some(&startup.map_id) && state.phase == DynamicWaterPhase::Ready && state.applied_generation == state.desired_generation));
    ready && (!author_tools::enabled() || author_tools::render_ready(world))
}

/// 显式验收才在完整就绪帧采一份有界状态；普通玩法帧不存在World全量证据收集。
fn snapshot(world: &mut World, passed: bool, reason: &str) -> serde_json::Value {
    let startup = world.resource::<SavedGameStartup>();
    let map = startup.map_id.0.clone();
    let name = startup.name.clone();
    let runtime = world.get_resource::<DynamicMapRuntimeStatus>().map(|state| serde_json::json!({
        "map":state.map_id.as_ref().map(|id|&id.0), "phase":format!("{:?}", state.phase), "revision":state.revision, "detail":state.detail,
    }));
    let placements = world.get_resource::<MapPlacementRuntimeStatus>().map(|state| serde_json::json!({
        "map":state.map_id.as_ref().map(|id|&id.0), "phase":format!("{:?}", state.phase), "revision":state.revision,
        "placement_count":state.placement_count, "detail":state.detail,
    }));
    let vegetation = world.get_resource::<DynamicVegetationStatus>().map(|state| serde_json::json!({
        "map":state.map_id.as_ref().map(|id|&id.0), "phase":format!("{:?}", state.phase), "revision":state.revision,
        "layer_count":state.layer_count, "blade_layers":state.blade_layer_count, "blade_windows":state.blade_window_count,
        "mesh_instances":state.mesh_instance_count, "mesh_batches":state.mesh_batch_count, "detail":state.detail,
    }));
    let water = world.get_resource::<DynamicWaterStatus>().map(|state| serde_json::json!({
        "map":state.map_id.as_ref().map(|id|&id.0), "phase":format!("{:?}", state.phase), "revision":state.revision,
        "layers":state.layer_count, "triangles":state.triangle_count, "desired_generation":state.desired_generation,
        "applied_generation":state.applied_generation, "detail":state.detail,
    }));
    let gate = world.get_resource::<SceneStartupGate>().map(|gate| serde_json::json!({
        "ready":gate.ready,"collision":gate.collision_ready,"scene_assets":gate.scene_assets_ready,
        "application":gate.application_ready,"application_error":gate.application_error,
        "sub_maps":gate.sub_maps_ready,"sub_maps_error":gate.sub_maps_error,
    }));
    let loading = world.get_resource::<LoadingProgress>().map(|state| serde_json::json!({"stage":format!("{:?}",state.stage),"label":state.label,"completed":state.completed,"total":state.total}));
    // 记录真实主视图的投影和姿态，不再用玩家种子或角色实例冒充场景已呈现。
    let views = world.query_filtered::<(&Camera, &Projection, &GlobalTransform, &bevy::camera::RenderTarget),
        (With<Camera3d>, With<feathrise_scene_core::view::PrimarySceneView>)>()
        .iter(world).take(8).map(|(camera, projection, transform, target)| {
            let projection = match projection {
                Projection::Perspective(value) => serde_json::json!({"kind":"perspective","fov":value.fov,"near":value.near,"far":value.far}),
                Projection::Orthographic(value) => serde_json::json!({"kind":"orthographic","scale":value.scale,"near":value.near,"far":value.far}),
                Projection::Custom(_) => serde_json::json!({"kind":"custom"}),
            };
            serde_json::json!({
            "active":camera.is_active,"position":transform.translation().to_array(),
            "projection":projection,"target":format!("{target:?}"),
            "window_target":matches!(target, bevy::camera::RenderTarget::Window(_)),
        })}).collect::<Vec<_>>();
    let mesh_entities = world
        .query_filtered::<Entity, With<Mesh3d>>()
        .iter(world)
        .count();
    // 自有小地图验收同时核对真实正式实例；仅源文件指纹相同不能证明新对象已进入游戏World。
    let instances = world
        .query::<(&feathrise_scene::MapPlacementInstance, &GlobalTransform)>()
        .iter(world)
        .filter(|(instance, _)| instance.map_id().as_str() == map && instance.is_active())
        .take(128)
        .map(|(instance, transform)| {
            serde_json::json!({
                "id":instance.id(),"position":transform.translation().to_array(),
                "asset_target":instance.asset_target(),
            })
        })
        .collect::<Vec<_>>();
    let present = wgpu_hal::vulkan::present_telemetry();
    // 只读生产启动已加载的目录，不为验收额外安装作者插件或主动加载另一份预设。
    let preset_catalogs = preset_catalog_evidence(world);
    let (terrain_objects, terrain_evidence_error) = match terrain_evidence(world) {
        Ok(objects) => (objects, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    let author_tools = author_tools::enabled().then(|| author_tools::snapshot(world));
    let passed = passed
        && terrain_evidence_error.is_none()
        && author_tools.as_ref().is_none_or(Result::is_ok);
    serde_json::json!({
        "schema":2,"entry":"production-SceneRuntimePlugin","passed":passed,"reason":reason,
        "map":map,"name":name,"source":world.resource::<GameSmoke>().source,
        "runtime":runtime,"placements":placements,"vegetation":vegetation,"water":water,"gate":gate,"loading":loading,
        "gate_lifecycle":"生产加载完成可移除SceneStartupGate与LoadingProgress；无门时仍验唯一中性视图、真实网格和地图各域Ready",
        "primary_scene_view_count":views.len(),"primary_scene_views":views,"mesh_entities":mesh_entities,"placement_instances":instances,
        "excluded_scope":"player movement, character initialization, inventory, equipment, skills and combat",
        "preset_catalogs":preset_catalogs,
        "terrain_objects":terrain_objects,
        "terrain_evidence_error":terrain_evidence_error,
        "author_tools":author_tools,
        "elapsed_seconds":world.resource::<GameSmoke>().started.elapsed().as_secs_f64(),
        "present":{"ready":present.ready,"accepted_count":present.successful_present_count,"epoch":present.presentation_epoch},
        "screenshot":"game.png","world":"separate-game-process",
        "return_input":if world.resource::<GameSmoke>().saved_play { "official KeyboardInput F12 through exit_saved_play" } else { "finite standalone fixture AppExit" },
    })
}

/// 容器地图的根植被状态不代表独立地形；使用原活动路径查询，不能借候选或同名兄弟的计数。
fn terrain_evidence(world: &mut World) -> Result<Vec<serde_json::Value>, String> {
    let map = world.resource::<SavedGameStartup>().map_id.clone();
    let targets = world
        .query::<(
            Entity,
            &feathrise_scene::SubMapInstance,
            &feathrise_scene::TerrainObjectInstance,
        )>()
        .iter(world)
        .filter(|(_, instance, object)| {
            instance.source_map_id == map
                && instance.path
                    == feathrise_map_content::MapInstancePath::root(map.clone())
                        .terrain(object.definition.id.clone())
        })
        .take(64)
        .map(|(entity, _, object)| (entity, object.definition.clone()))
        .collect::<Vec<_>>();
    let mut access = bevy::ecs::system::SystemState::<
        feathrise_scene::TerrainObjectResource<DynamicVegetationStatus>,
    >::new(world);
    let vegetation = access.get(world).map_err(|error| error.to_string())?;
    Ok(targets
        .into_iter()
        .filter_map(|(entity, object)| {
            let (active, status) = vegetation.for_saved_object(&map, &object.id)?;
            if active != entity { return None; }
            Some(serde_json::json!({
                "terrain_id":object.id.as_str(),"source_map_id":map.as_str(),
                "active_instance":format!("{active:?}"),
                "terrain_glb_path":object.terrain.glb_path,"surface_path":object.surface_path,
                "vegetation_path":object.vegetation_path,"water_path":object.water_path,
                "vegetation":{"map":status.map_id.as_ref().map(|id|id.as_str()),
                    "phase":format!("{:?}",status.phase),"layers":status.layer_count,
                    "blade_layers":status.blade_layer_count,"blade_windows":status.blade_window_count,
                    "mesh_instances":status.mesh_instance_count,"mesh_batches":status.mesh_batch_count,
                    "mesh_render_nodes":status.mesh_render_node_count,"detail":status.detail},
            }))
        })
        .collect())
}

fn preset_catalog_evidence(world: &World) -> serde_json::Value {
    serde_json::json!({
        "surface":world.get_resource::<feathrise_map_content::SurfacePresetRegistry>().map(|registry| serde_json::json!({
            "revision":registry.revision,"error":registry.last_error,
            "first":registry.presets().first().map(|preset| {
                let descriptor_sha256: String = Sha256::digest(serde_json::to_vec(preset).expect("地表描述符必须能序列化"))
                    .iter().map(|byte| format!("{byte:02x}")).collect();
                serde_json::json!({"stable_id":preset.id,"name":preset.label,"descriptor_sha256":descriptor_sha256})
            }),
        })), // 只观察生产启动已采用的普通地表；不为验收加载另一份目录。
    })
}

fn advance(world: &mut World) {
    if world.resource::<GameSmoke>().completed {
        // 试玩回程必须由原F12处理器完成，不能由截图夹具直接退出掩盖返回链路失效。
        if world
            .resource::<GameSmoke>()
            .return_requested
            .is_some_and(|started| started.elapsed() > Duration::from_secs(10))
        {
            world.resource_mut::<GameSmoke>().return_requested = None;
            eprintln!("完整运行验收未通过：原F12返回入口未结束运行");
            world.write_message(AppExit::error());
        }
        return;
    }
    let completion = {
        let mut smoke = world.resource_mut::<GameSmoke>();
        smoke.writer.as_mut().and_then(check_ready)
    };
    if let Some(result) = completion {
        world.resource_mut::<GameSmoke>().completed = true;
        match result {
            Ok(()) => {
                if world.resource::<GameSmoke>().saved_play {
                    let window = world
                        .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
                        .single(world);
                    if let Ok(window) = window {
                        world.write_message(bevy::input::keyboard::KeyboardInput {
                            key_code: KeyCode::F12,
                            logical_key: bevy::input::keyboard::Key::F12,
                            state: bevy::input::ButtonState::Pressed,
                            text: None,
                            repeat: false,
                            window,
                        });
                        world.resource_mut::<GameSmoke>().return_requested = Some(Instant::now());
                    } else {
                        eprintln!("完整运行验收未通过：F12返回缺少主窗口");
                        world.write_message(AppExit::error());
                    }
                } else {
                    world.write_message(AppExit::Success);
                }
            }
            Err(error) => {
                eprintln!("完整运行验收未通过：{error}");
                world.write_message(AppExit::error());
            }
        }
        return;
    }
    if world.resource::<GameSmoke>().writer.is_some() {
        return;
    }
    let failed = world
        .get_resource::<LoadingProgress>()
        .is_some_and(|progress| progress.stage == feathrise_scene_core::state::LoadingStage::Failed);
    if failed
        || world.resource::<GameSmoke>().started.elapsed() > world.resource::<GameSmoke>().budget
    {
        let reason = if failed {
            "原生产加载门已报告失败"
        } else {
            "SceneRuntimePlugin未在验收时限内完成场景/关键内容/截图验收"
        };
        let report = snapshot(world, false, reason);
        let directory = world.resource::<GameSmoke>().directory.clone();
        // 失败报告同样后台写，返回Err确保调用方不会把进程正常收口冒称验收通过。
        world.resource_mut::<GameSmoke>().writer = Some(IoTaskPool::get().spawn(async move {
            write_json(&directory.join("game.json"), &report)?;
            Err("完整运行验收超时，详见game.json".into())
        }));
        return;
    }
    if world.resource::<GameSmoke>().capture_requested {
        return;
    }
    if !ready(world) {
        world.resource_mut::<GameSmoke>().stable = 0;
        return;
    }
    let stable = {
        let mut smoke = world.resource_mut::<GameSmoke>();
        smoke.stable = smoke.stable.saturating_add(1);
        smoke.stable
    };
    if stable < 40 {
        return;
    } // 真实启动门后的稳定渲染帧，避免首个已就绪tick还未提交GPU画面。
    let report = snapshot(
        world,
        true,
        "中性主视图与地图各域已就绪，等待真实GPU截图写入",
    );
    {
        let mut smoke = world.resource_mut::<GameSmoke>();
        smoke.capture_requested = true;
        smoke.snapshot = Some(report);
    }
    world.spawn(Screenshot::primary_window()).observe(captured);
}

fn captured(
    event: On<ScreenshotCaptured>,
    mut smoke: ResMut<GameSmoke>,
    project: Res<ProjectContext>,
) {
    if smoke.writer.is_some() || smoke.completed {
        return;
    }
    let Some(mut report) = smoke.snapshot.take() else {
        return;
    };
    let image = event.image.clone(); // 仅显式验收一帧快照；编码/文件写入移入唯一IO任务。
    let directory = smoke.directory.clone();
    let project = project.clone();
    smoke.writer = Some(IoTaskPool::get().spawn(async move {
        let width = image.width();
        let height = image.height();
        let pixels = image.data.as_ref().ok_or("GPU截图没有CPU回读像素")?;
        report["capture"] = serde_json::json!({"width":width,"height":height,"bytes":pixels.len(),
            "sha256":Sha256::digest(pixels).iter().map(|byte|format!("{byte:02x}")).collect::<String>(),"source":"Bevy ScreenshotCaptured"});
        let dynamic = image
            .try_into_dynamic()
            .map_err(|error| format!("GPU截图转换失败：{error}"))?;
        dynamic
            .to_rgb8()
            .save(directory.join("game.png"))
            .map_err(|error| format!("GPU截图保存失败：{error}"))?;
        // 地形私有文件的完整指纹只在显式验收后台采集，不把文件读取放进游戏帧。
        append_terrain_sources(&project, &mut report)?;
        report["reason"] = "SceneRuntimePlugin已就绪，真实GPU截图与报告均已落盘".into();
        write_json(&directory.join("game.json"), &report)
    }));
}

fn append_terrain_sources(
    project: &ProjectContext,
    report: &mut serde_json::Value,
) -> Result<(), String> {
    let mut paths = std::collections::BTreeSet::new();
    for object in report["terrain_objects"].as_array().into_iter().flatten() {
        for field in [
            "terrain_glb_path",
            "surface_path",
            "vegetation_path",
            "water_path",
        ] {
            if let Some(path) = object[field].as_str() {
                paths.insert(path.to_owned());
            }
        }
    }
    if author_tools::enabled() {
        paths.extend(
            [
                "scene/probe.glb",
                "scene/albedo.png",
                "scene/animation.glb",
            ]
            .into_iter()
            .map(str::to_owned),
        );
    }
    let files = report["source"]["files"]
        .as_array_mut()
        .ok_or("缺少运行文件来源")?;
    for path in paths {
        if files.iter().any(|file| file["path"] == path) {
            continue;
        }
        if files.len() >= 64 {
            return Err("有限地图验收的文件指纹超出64项".into());
        }
        let file = context::checked_existing(project.asset_root(), Path::new(&path), false)?;
        let (size, hash) = hash_file(&file)?;
        files.push(serde_json::json!({"path":path,"bytes":size,"sha256":hash}));
    }
    Ok(())
}

fn write_json(path: &Path, report: &serde_json::Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(report).map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| format!("运行报告写入失败：{error}"))
}

fn source_evidence(
    project: &ProjectContext,
    row: &feathrise_map_content::MapCatalogEntry,
) -> Result<serde_json::Value, String> {
    let mut paths = vec![
        row.manifest_path(),
        row.placements_path(),
        row.portals_path(),
    ];
    paths.extend(
        [
            row.surface_path.as_ref(),
            row.vegetation_path.as_ref(),
            row.water_path.as_ref(),
            row.boundary_path.as_ref(),
            row.characters_path.as_ref(),
        ]
        .into_iter()
        .flatten()
        .cloned(),
    );
    let mut files = Vec::new();
    for path in paths {
        let file = context::checked_existing(project.asset_root(), Path::new(&path), false)?;
        let (size, hash) = hash_file(&file)?;
        files.push(serde_json::json!({"path":path,"bytes":size,"sha256":hash}));
    }
    let catalog = context::checked_existing(
        project.asset_root(),
        Path::new(feathrise_map_content::MAP_CATALOG_ASSET_PATH),
        false,
    )?;
    let (_, catalog_sha256) = hash_file(&catalog)?;
    Ok(
        serde_json::json!({"project_id":project.id,"project_root":project.root,"asset_roots":project.asset_roots,
        "catalog_sha256":catalog_sha256,"files":files}),
    )
}

fn hash_file(path: &Path) -> Result<(u64, String), String> {
    let mut reader = File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut size = 0_u64;
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        size += count as u64;
        digest.update(&buffer[..count]);
    }
    Ok((
        size,
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    ))
}
