//! 真实场景目录、GLB 实例和 Avian 查询共同验证子场景的隐藏候选、同帧交接与退役。

use super::*;
use crate::{GameScenePlugin, SceneApplicationHooks, SceneApplicationStartupStatus, ScenePlacementRuntimePhase, ScenePlacementRuntimeStatus};
use avian3d::prelude::*;
use bevy::{
    asset::io::{AssetSourceBuilder, AssetSourceId, memory::{Dir, MemoryAssetReader}},
    ecs::system::RunSystemOnce,
    time::TimeUpdateStrategy,
    window::ExitCondition,
};
use feathrise_scene_content::{SceneCatalogAsset, SceneCatalogEntry, SceneInstanceId, SceneInstancePlacement, ScenePlacementAssetTarget, ScenePlacementHeightMode, ScenePlacementRecord, ScenePlacementsAsset, ScenePortalsAsset, SCENE_CATALOG_ASSET_PATH};
use serde_json::json;
use std::{collections::BTreeSet, path::Path, time::{Duration, Instant}};

const ROOT_A: &str = "SCENE-collider-root-a";
const ROOT_B: &str = "SCENE-collider-root-b";
const SOURCE_A: &str = "SCENE-collider-source-a";
const SOURCE_B: &str = "SCENE-collider-source-b";
const COMBINED: &str = "fixture/combined.glb";
const GROUPED: &str = "fixture/grouped.glb";
const X_POINTS: [f32; 4] = [0., -4., 4., 7.];

#[derive(Resource, Default)]
struct HoldCandidate(bool);

fn id(value: &str) -> SceneId { SceneId::parse(value).unwrap() }

fn reference(source: &str) -> SceneInstancePlacement {
    SceneInstancePlacement { id: SceneInstanceId::parse("first-floor").unwrap(), source_scene_id: id(source),
        label: "碰撞夹具".into(), transform: Default::default() }
}

fn write_json(files: &Dir, path: &str, value: &impl serde::Serialize) {
    files.insert_asset(Path::new(path), serde_json::to_vec(value).unwrap());
}

fn write_models(files: &Dir, second_x: f32) {
    fn glb(xs: &[f32], independent: bool) -> Vec<u8> {
        let vertices: [f32; 12] = [-1., 0., -1., -1., 0., 1., 1., 0., -1., 1., 0., 1.];
        let mut bin: Vec<_> = vertices.into_iter().flat_map(f32::to_le_bytes).collect();
        // 提供法线以保留索引。Bevy 会拆分缺失法线的顶点来生成平直法线。
        bin.extend([[0_f32, 1., 0.]; 4].into_iter().flatten().flat_map(f32::to_le_bytes));
        bin.extend([0_u16, 1, 2, 1, 3, 2].into_iter().flat_map(u16::to_le_bytes));
        let mut nodes = Vec::new();
        for (index, x) in xs.iter().enumerate() {
            let mut node = json!({"name":format!("平面-{index}"),"mesh":0,"translation":[x,0,0]});
            if independent {
                // 同策略且不相交的网格允许合并。两份实际策略保证走独立形状分组。
                let lod = if index == 0 { json!({"mode":"disabled"}) } else {
                    json!({"mode":"override","settings":{"safe_near_meters":32.,"max_error_meters":0.02}})
                };
                node["extras"] = json!({"feathrise_mesh_properties":{"collision":true,"collision_lod":lod}});
            }
            nodes.push(node);
        }
        // 空节点让退役树必须跨帧推进，碰撞准备仍只处理上面两个真实网格。
        if independent { nodes.extend((0..160).map(|i| json!({"name":format!("中性节点-{i}")}))); }
        let roots: Vec<_> = (0..nodes.len()).collect();
        let mut json = serde_json::to_vec(&json!({
            "asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":roots}],"nodes":nodes,
            "meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1},"indices":2}]}],
            "buffers":[{"byteLength":bin.len()}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":48,"target":34962},
                           {"buffer":0,"byteOffset":48,"byteLength":48,"target":34962},
                           {"buffer":0,"byteOffset":96,"byteLength":12,"target":34963}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[-1,0,-1],"max":[1,0,1]},
                         {"bufferView":1,"componentType":5126,"count":4,"type":"VEC3"},
                         {"bufferView":2,"componentType":5123,"count":6,"type":"SCALAR"}]
        })).unwrap();
        json.resize(json.len().next_multiple_of(4), b' ');
        let mut bytes = b"glTF".to_vec();
        for word in [2, (28 + json.len() + bin.len()) as u32, json.len() as u32, 0x4e4f_534a] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend(json);
        bytes.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"BIN\0");
        bytes.extend(bin);
        bytes
    }
    files.insert_asset(Path::new(COMBINED), glb(&[0.], false));
    files.insert_asset(Path::new(GROUPED), glb(&[-4., second_x], true));
}

fn seed_assets() -> Dir {
    use feathrise_scene_content as content;

    let files = Dir::default();
    // 同公共示例的空配置，只启动场景所需的目录；没有任何私有工程素材。
    for (path, schema_version) in [
        (content::BUILDING_PRESET_CATALOG_ASSET_PATH, content::BUILDING_PRESET_SCHEMA_VERSION),
        (content::COURTYARD_PRESET_CATALOG_ASSET_PATH, content::COURTYARD_PRESET_SCHEMA_VERSION),
        (content::VEGETATION_PRESET_CATALOG_ASSET_PATH, content::VEGETATION_PRESET_SCHEMA_VERSION),
        (content::WATER_PRESET_CATALOG_ASSET_PATH, content::WATER_PRESET_SCHEMA_VERSION),
    ] {
        write_json(&files, path, &json!({"schema_version":schema_version,"presets":[]}));
    }
    write_json(&files, content::WORLD_ENVIRONMENT_ASSET_PATH, &content::WorldEnvironmentAsset::default());
    write_json(&files, content::SURFACE_PRESET_CATALOG_ASSET_PATH, &json!({
        "schema_version":content::SURFACE_PRESET_SCHEMA_VERSION,"texture_resolution":[1,1],
        "base_color_array_path":"textures/surface/base.ktx2","normal_array_path":"textures/surface/normal.ktx2",
        "arm_height_array_path":"textures/surface/arm_height.ktx2","presets":[]
    }));
    write_models(&files, 4.);
    let mut catalog = SceneCatalogAsset::default();
    for (scene, child) in [(ROOT_A, Some(SOURCE_A)), (ROOT_B, Some(SOURCE_A)), (SOURCE_A, None), (SOURCE_B, None)] {
        let entry = SceneCatalogEntry { id: id(scene), display_name: scene.into(), folder: format!("scenes/{scene}"),
            published: true, placements_glb: true, surface_path: None, vegetation_path: None, water_path: None,
            boundary_path: None, characters_path: None };
        let manifest = SceneManifestAsset {
            vegetation_paint_groups: Default::default(), schema_version: SCENE_SCHEMA_VERSION,
            id: id(scene), display_name: scene.into(), notes: String::new(), runtime_scene: "data_driven".into(),
            world_min: [-32.; 2], world_max: [32.; 2], default_spawn: [0., 4., 0.], environment: Default::default(),
            terrain: SceneTerrainDescriptor::empty(), terrains: Some(Arc::from([])),
            subscenes: child.map(|child| Arc::from([reference(child)])).unwrap_or_default(), puddles: None, asset_roots: Vec::new(),
        };
        write_json(&files, &entry.manifest_path(), &manifest);
        write_json(&files, &entry.portals_path(), &ScenePortalsAsset { scene_id: id(scene), ..default() });
        let placements = if child.is_none() {
            [COMBINED, GROUPED].into_iter().map(|asset_path| ScenePlacementRecord {
                id: asset_path.rsplit('/').next().unwrap().trim_end_matches(".glb").into(),
                asset_path: asset_path.into(), asset_target: ScenePlacementAssetTarget::Scene { collision: true },
                translation: [0.; 3], yaw_degrees: 0., scale: [1.; 3], height_mode: ScenePlacementHeightMode::FixedHeight,
                terrain_offset: 0., building_preset_id: None, courtyard: None, terrain_patch: None,
                terrain_binding: None, mesh_lod: None, editor: Default::default(), water_surfaces: Vec::new(),
            }).collect()
        } else { Vec::new() };
        let path = entry.placements_path();
        let placements = ScenePlacementsAsset { scene_id: id(scene), schema_version: SCENE_SCHEMA_VERSION, placements };
        let bytes = feathrise_scene_content::write_scene_placements_glb(&placements, &path, None).unwrap();
        files.insert_asset(Path::new(&path), bytes);
        catalog.scenes.push(entry);
    }
    write_json(&files, SCENE_CATALOG_ASSET_PATH, &catalog);
    files
}

fn app(files: &Dir) -> App {
    let mut app = App::new();
    let files = files.clone();
    let step = Duration::from_secs_f64(1. / 60.);
    // 正式装配需要为非 AssetServer 读取提供路径。此夹具没有文件系统资产。
    let asset_root = std::env::temp_dir().join(format!("feathrise-collision-fixture-{}", std::process::id()));
    assert!(!asset_root.exists());
    app.register_asset_source(AssetSourceId::Default, AssetSourceBuilder::new(move ||
        Box::new(MemoryAssetReader { root: files.clone() })))
        .add_plugins(DefaultPlugins
            .set(WindowPlugin { primary_window: None, exit_condition: ExitCondition::DontExit, ..default() })
            .disable::<bevy::winit::WinitPlugin>()
            .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>())
        .init_state::<ScenePhase>()
        .add_plugins((PhysicsPlugins::default(), feathrise_settings::GraphicsSettingsPlugin,
            feathrise_billboard_text::BillboardTextPlugin, feathrise_portal::PortalPlugin,
            feathrise_atmosphere::WorldTimePlugin, feathrise_atmosphere::WeatherPlugin,
            feathrise_grounding::GroundingPlugin, GameScenePlugin))
        .insert_resource(feathrise_scene_content::RuntimeAssetRoot::new(asset_root).unwrap())
        .insert_resource(TimeUpdateStrategy::ManualDuration(step))
        .insert_resource(Time::<Fixed>::from_duration(step))
        .insert_resource(Gravity(Vec3::ZERO))
        .init_resource::<HoldCandidate>()
        .insert_resource(SceneApplicationHooks {
            startup: |_, _| SceneApplicationStartupStatus { completed: 0, total: 0, error: None },
            configure_instance: |_| {},
            instance_ready: |world, instance| {
                // 仅给正式完整门增加测试阻断，其余六域仍必须由生产系统证明完成。
                let batch = world.get::<SubsceneInstance>(instance).unwrap().batch;
                Ok(!world.resource::<HoldCandidate>().0 || world.get::<SubsceneActive>(batch).is_some())
            },
            publish_instance: |_, _, _| {}, retire_instance: |_, _, _| None,
            reset_instances: |_, _| {}, expect_instances: |_, _| {},
        });
    app.finish();
    app.cleanup();
    app
}

fn wait(app: &mut App, mut done: impl FnMut(&mut App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        app.update();
        if done(app) { return; }
        let status = app.world().resource::<SubsceneRuntimeStatus>();
        assert!(Instant::now() < deadline, "未见正式批次推进：{:?} {:?}", status.loading_detail, status.last_error);
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn active(app: &App) -> Option<Entity> {
    app.world().resource::<SubsceneRuntime>().active.as_ref().map(|batch| batch.root)
}

fn prepared_candidate(app: &App) -> Option<Entity> {
    let world = app.world();
    let candidate = world.resource::<SubsceneRuntime>().candidate.as_ref()?;
    let context = candidate.contexts.first()?.entity;
    let status = world.resource::<InstanceResourceTable<ScenePlacementRuntimeStatus>>().values.get(&context)?;
    (status.phase == ScenePlacementRuntimePhase::Ready && status.placement_count == 2).then_some(candidate.root)
}

fn raw(app: &mut App, probes: [Entity; 4]) -> [BTreeSet<Entity>; 4] {
    app.world_mut().run_system_once(move |queries: SpatialQuery| X_POINTS.map(|x| {
        let rays: BTreeSet<_> = queries.ray_hits(Vec3::new(x + 0.6, 4., 0.), Dir3::NEG_Y, 8., 16, true,
            &SpatialQueryFilter::default()).into_iter().map(|hit| hit.entity).collect();
        let volume = Collider::cuboid(1.5, 1., 1.5).aabb(Vec3::new(x, 0., 0.), Quat::IDENTITY, 0.);
        let boxes: BTreeSet<_> = queries.aabb_intersections_with_aabb(volume).into_iter().filter(|entity| !probes.contains(entity)).collect();
        assert_eq!(rays, boxes, "射线及无过滤宽相查询应看到同一批碰撞，X={x}");
        rays
    })).unwrap()
}

fn constrained(app: &App, probes: [Entity; 4]) -> BTreeSet<Entity> {
    let graph = app.world().resource::<ContactGraph>();
    probes.into_iter().flat_map(|probe| graph.contact_pairs_with(probe).filter_map(move |pair| {
        (pair.is_touching() && pair.generates_constraints()).then_some(if pair.collider1 == probe { pair.collider2 } else { pair.collider1 })
    })).collect()
}

#[test]
#[ignore = "正式场景插件需要离屏 GPU，夹具只使用独占内存资产"]
fn real_subscene_collision_batch_exchanges_without_raw_query_leaks() {
    let files = seed_assets();
    let mut app = app(&files);
    wait(&mut app, |app| app.world().resource::<SceneContentRegistry>().handles(&id(ROOT_A)).is_some());
    app.world_mut().resource_mut::<ActiveDynamicScene>().select(id(ROOT_A));
    app.world_mut().resource_mut::<NextState<ScenePhase>>().set(ScenePhase::Active);
    let probes = X_POINTS.map(|x| app.world_mut().spawn((RigidBody::Dynamic, Collider::sphere(0.22),
        LockedAxes::TRANSLATION_LOCKED, SleepingDisabled, Transform::from_xyz(x, 0.14, 0.))).id());
    wait(&mut app, |app| active(app).is_some());
    let old_batch = active(&app).unwrap();
    let old = raw(&mut app, probes);
    assert_eq!(old.each_ref().map(BTreeSet::len), [1, 1, 1, 0]);
    let old_shapes: BTreeSet<_> = old.iter().flatten().copied().collect();
    assert_eq!(old_shapes.len(), 3, "单体加独立分组各有正式形状");
    wait(&mut app, |app| constrained(app, probes) == old_shapes);

    app.world_mut().resource_mut::<HoldCandidate>().0 = true;
    let handle = app.world().resource::<SceneContentRegistry>().handles(&id(ROOT_A)).unwrap().manifest.clone();
    app.world_mut().resource_mut::<Assets<SceneManifestAsset>>().get_mut(&handle).unwrap().subscenes = Arc::from([reference(SOURCE_B)]);
    wait(&mut app, |app| {
        assert_eq!(raw(app, probes), old, "重叠位置候选不可从原始查询透出");
        assert_eq!(constrained(app, probes), old_shapes);
        prepared_candidate(app).is_some()
    });
    let new_batch = prepared_candidate(&app).unwrap();
    assert_ne!(new_batch, old_batch);
    app.world_mut().resource_mut::<HoldCandidate>().0 = false;
    wait(&mut app, |app| {
        if active(app) == Some(new_batch) { return true; }
        assert_eq!(raw(app, probes), old);
        false
    });
    let adopted = raw(&mut app, probes);
    assert_eq!(adopted.each_ref().map(BTreeSet::len), [1, 1, 1, 0]);
    let adopted_shapes: BTreeSet<_> = adopted.iter().flatten().copied().collect();
    assert!(old_shapes.is_disjoint(&adopted_shapes));
    assert!(constrained(&app, probes).is_subset(&adopted_shapes));
    assert!(app.world().get_entity(old_batch).is_ok(), "物理旧形状同帧撤销，旧树仍由退役游标回收");
    wait(&mut app, |app| {
        assert_eq!(raw(app, probes), adopted);
        constrained(app, probes) == adopted_shapes && app.world().get_entity(old_batch).is_err()
    });

    // 同一引用来源的 GLB 热更换掉第二块板，等待时保持旧的完整碰撞，采用时不可出现两套支撑。
    app.world_mut().resource_mut::<HoldCandidate>().0 = true;
    write_models(&files, 7.);
    app.world().resource::<AssetServer>().reload(GROUPED);
    wait(&mut app, |app| {
        assert_eq!(raw(app, probes), adopted, "源资产换代不得提前替换活动碰撞");
        assert_eq!(constrained(app, probes), adopted_shapes);
        prepared_candidate(app).is_some()
    });
    let reloaded_batch = prepared_candidate(&app).unwrap();
    app.world_mut().resource_mut::<HoldCandidate>().0 = false;
    wait(&mut app, |app| {
        if active(app) == Some(reloaded_batch) { return true; }
        assert_eq!(raw(app, probes), adopted);
        false
    });
    let reloaded = raw(&mut app, probes);
    assert_eq!(reloaded.each_ref().map(BTreeSet::len), [1, 1, 0, 1], "热更新须整体交接");
    let reloaded_shapes: BTreeSet<_> = reloaded.iter().flatten().copied().collect();
    assert!(reloaded_shapes.is_disjoint(&adopted_shapes));
    assert!(constrained(&app, probes).is_subset(&reloaded_shapes));
    wait(&mut app, |app| {
        assert_eq!(raw(app, probes), reloaded);
        constrained(app, probes) == reloaded_shapes && app.world().get_entity(new_batch).is_err()
    });

    app.world_mut().resource_mut::<HoldCandidate>().0 = true;
    app.world_mut().resource_mut::<ActiveDynamicScene>().select(id(ROOT_B));
    wait(&mut app, |app| {
        if app.world().resource::<SubsceneRuntime>().root_scene_id.as_ref() != Some(&id(ROOT_B)) { return false; }
        assert!(raw(app, probes).iter().all(BTreeSet::is_empty), "换父图即离场，隐藏的下一图不得参加查询");
        assert!(constrained(app, probes).is_empty());
        prepared_candidate(app).is_some()
    });
    app.world_mut().resource_mut::<HoldCandidate>().0 = false;
    wait(&mut app, |app| active(app).is_some());
    let switched = raw(&mut app, probes);
    assert_eq!(switched.each_ref().map(BTreeSet::len), [1, 1, 0, 1]);
    let switched_shapes: BTreeSet<_> = switched.iter().flatten().copied().collect();
    assert!(switched_shapes.is_disjoint(&reloaded_shapes));
    wait(&mut app, |app| constrained(app, probes) == switched_shapes);
}
