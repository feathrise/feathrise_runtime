//! 验证真实引用协议、多级变换和失败整批拒绝，避免重复来源被误判为循环。

mod manifest_events; // 实驱动图发布，派生清单通知不能冒充目录权威来源。

use super::*;
use crate::{MapId, MapManifestAsset};
use bevy::prelude::{Rect, Vec2, Vec3};
use std::{collections::BTreeMap, sync::Arc};

fn id(value: &str) -> MapId { MapId::parse(value).unwrap() }
fn manifest_fixture() -> MapManifestAsset {
    serde_json::from_value(serde_json::json!({
        "schema_version": 1, "id": "MAP-fixture", "display_name": "测试地图",
        "runtime_scene": "DynamicMapScene", "world_min": [-32.0, -32.0],
        "world_max": [32.0, 32.0], "default_spawn": [0.0, 2.0, 0.0],
        "terrain": {"kind": "heightfield", "glb_path": "maps/MAP-fixture/terrain.glb",
            "resolution": [9, 9], "base_height": -80.0, "height_scale": 160.0},
        "asset_roots": []
    })).unwrap() // 子图协议测试不依赖某张实际作者地图。
}
fn instance(name: &str, source: &str, x: f32, yaw: f32) -> MapInstancePlacement {
    MapInstancePlacement {
        id: MapInstanceId::parse(name).unwrap(), source_map_id: id(source), label: name.to_owned(),
        transform: MapInstanceTransform { translation: [x, 0.0, 0.0], yaw_degrees: yaw,  ..Default::default() },
    }
}
fn source(name: &str, instances: Vec<MapInstancePlacement>) -> Arc<MapInstanceSource> {
    Arc::new(MapInstanceSource { terrains: None, map_id: id(name), revision: 0, world_bounds: Rect::from_corners(Vec2::splat(-2.0), Vec2::splat(2.0)), instances: instances.into() })
}
fn graph(sources: Vec<Arc<MapInstanceSource>>) -> BTreeMap<MapId, Arc<MapInstanceSource>> {
    sources.into_iter().map(|s| (s.map_id.clone(), s)).collect()
}

#[test]
fn repeated_sources_have_independent_addresses_and_shared_source_identity() {
    let sources = graph(vec![source("parent", vec![instance("village-a", "village", 10.0, 0.0), instance("village-b", "village", 30.0, 90.0)]), source("village", vec![])]);
    let expanded = expand_map_instances(&id("parent"), &sources).unwrap();
    assert_eq!(expanded.len(), 3);
    assert_eq!(expanded[1].source_map_id, expanded[2].source_map_id);
    assert_ne!(expanded[1].path, expanded[2].path);
    assert_eq!(expanded[1].transform.translation, [10.0, 0.0, 0.0]);
    assert_eq!(expanded[2].path.to_string(), "parent/village-b");
}

#[test]
fn nested_translation_rotates_in_parent_frame_and_inverse_is_exact() {
    let sources = graph(vec![source("parent", vec![instance("camp", "village", 10.0, 90.0)]), source("village", vec![instance("gate", "house", 3.0, 30.0)]), source("house", vec![])]);
    let expanded = expand_map_instances(&id("parent"), &sources).unwrap();
    assert!(Vec3::from_array(expanded[2].transform.translation).abs_diff_eq(Vec3::new(10.0, 0.0, -3.0), 0.00001));
    assert_eq!(expanded[2].transform.yaw_degrees, 120.0);
    let local = Vec3::new(1.0, 12.0, 2.0);
    let t = expanded[2].transform;
    assert!(t.to_local(t.to_world(local)).abs_diff_eq(local, 0.00001));
    assert!(expanded[2].path.is_descendant_of(&expanded[1].path));
}

#[test]
fn indirect_cycle_reports_complete_source_chain_and_publishes_nothing() {
    let sources = graph(vec![source("a", vec![instance("b-ref", "b", 0.0, 0.0)]), source("b", vec![instance("c-ref", "c", 0.0, 0.0)]), source("c", vec![instance("a-ref", "a", 0.0, 0.0)])]);
    assert!(expand_map_instances(&id("a"), &sources).unwrap_err().contains("a → b → c → a"));
}

#[test]
fn missing_source_and_duplicate_instance_id_are_errors() {
    let sources = graph(vec![source("a", vec![instance("missing", "b", 0.0, 0.0)])]);
    assert!(expand_map_instances(&id("a"), &sources).unwrap_err().contains("a/missing"));
    let repeated = vec![instance("same", "b", 0.0, 0.0), instance("same", "c", 0.0, 0.0)];
    assert!(validate_map_instances(&id("a"), &repeated).unwrap_err().contains("重复"));
}

#[test]
fn invalid_transform_and_direct_self_reference_never_enter_runtime() {
    let mut child = instance("child", "b", 0.0, 0.0);
    child.transform.translation[2] = f32::NAN;
    assert!(validate_map_instances(&id("a"), &[child]).is_err());
    assert!(validate_map_instances(&id("a"), &[instance("self", "a", 0.0, 0.0)]).is_err());
    assert!(MapInstanceId::parse("../../terrain").is_err());
}

#[test]
fn old_manifest_omits_empty_references_and_new_references_round_trip() {
    let mut manifest = manifest_fixture();
    assert!(manifest.sub_maps.is_empty());
    assert!(serde_json::to_value(&manifest).unwrap().get("sub_maps").is_none());
    manifest.sub_maps = Arc::from([instance("child-1", "MAP-child", 2.0, -45.0)]);
    let saved = serde_json::to_value(&manifest).unwrap();
    let decoded: MapManifestAsset = serde_json::from_value(saved).unwrap();
    assert_eq!(decoded.sub_maps, manifest.sub_maps);
}

#[test]
fn depth_budget_is_a_failure_instead_of_a_truncated_tree() {
    let mut sources = BTreeMap::new();
    for n in 0..=MAX_MAP_INSTANCE_DEPTH + 1 {
        let name = format!("map-{n}");
        let children = if n <= MAX_MAP_INSTANCE_DEPTH { vec![instance("next", &format!("map-{}", n + 1), 0.0, 0.0)] } else { vec![] };
        sources.insert(id(&name), source(&name, children));
    }
    assert!(expand_map_instances(&id("map-0"), &sources).unwrap_err().contains("32层"));
}

#[test]
fn rotated_bounds_include_all_corners_but_query_uses_local_frame() {
    let sources = graph(vec![source("parent", vec![instance("rotated", "child", 10.0, 45.0)]), source("child", vec![])]);
    let expanded = expand_map_instances(&id("parent"), &sources).unwrap();
    let child = &expanded[1];
    assert!(child.world_bounds.width() > child.local_bounds.width());
    assert!(child.world_bounds.min.x < 8.0);
    assert!(child.world_bounds.max.x > 12.0);
}

#[test]
fn background_graph_waits_for_declared_sources_and_preserves_the_last_complete_tree() {
    use bevy::{asset::{AssetEvent, AssetLoadFailedEvent}, prelude::*};
    use crate::{MapContentHandles, MapContentRegistry};
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<Assets<MapManifestAsset>>()
        .init_resource::<MapInstanceRootRequest>()
        .init_resource::<MapInstanceAuthorPreview>()
        .init_resource::<MapInstanceGraph>()
        .init_resource::<super::resources::MapInstanceGraphWork>()
        .add_message::<AssetEvent<MapManifestAsset>>()
        .add_message::<AssetLoadFailedEvent<MapManifestAsset>>()
        .add_systems(Update, super::systems::drive_instance_graph);
    let mut parent = manifest_fixture();
    parent.id = id("MAP-parent");
    parent.runtime_scene = "DynamicMapScene".into();
    parent.sub_maps = Arc::from([instance("village", "MAP-village", 10.0, 90.0)]);
    let mut village = parent.clone(); village.id = id("MAP-village"); village.sub_maps = Arc::from([]);
    let parent_handle = app.world_mut().resource_mut::<Assets<MapManifestAsset>>().add(parent);
    let child_handle = app.world_mut().resource_mut::<Assets<MapManifestAsset>>().reserve_handle();
    let handles = |manifest| MapContentHandles { manifest, portals: default(), placements: default(),
        surface: None, vegetation: None, water: None, boundaries: None, characters_path: None };
    app.insert_resource(MapContentRegistry { catalog: default(), revision: 1, last_error: None,
        maps: BTreeMap::from([(id("MAP-parent"), handles(parent_handle)), (id("MAP-village"), handles(child_handle.clone()))]) });
    app.world_mut().resource_mut::<MapInstanceRootRequest>().map_id = Some(id("MAP-parent"));
    let wait = |app: &mut App, phase| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.update();
            let work = app.world().resource::<super::resources::MapInstanceGraphWork>();
            if work.started.is_some() && work.task.is_none() && app.world().resource::<MapInstanceGraph>().phase == phase { break; }
            assert!(std::time::Instant::now() < deadline, "有界后台图任务未在测试期限内完成");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    };
    wait(&mut app, MapInstanceGraphPhase::Waiting);
    assert!(app.world().resource::<MapInstanceGraph>().instances.is_empty()); // 冷清单不是缺失来源或半份成功。
    app.world_mut().resource_mut::<Assets<MapManifestAsset>>().insert(child_handle.id(), village.clone()).unwrap();
    app.world_mut().write_message(AssetEvent::Added { id: child_handle.id() });
    wait(&mut app, MapInstanceGraphPhase::Ready);
    let complete = app.world().resource::<MapInstanceGraph>().instances.clone();
    assert_eq!(complete.len(), 2);
    app.world_mut().resource_mut::<Assets<MapManifestAsset>>().remove(child_handle.id());
    app.world_mut().write_message(AssetEvent::Removed { id: child_handle.id() });
    wait(&mut app, MapInstanceGraphPhase::Waiting);
    assert!(Arc::ptr_eq(&complete, &app.world().resource::<MapInstanceGraph>().instances));
    app.world_mut().resource_mut::<Assets<MapManifestAsset>>().insert(child_handle.id(), village).unwrap();
    app.world_mut().write_message(AssetEvent::Added { id: child_handle.id() });
    wait(&mut app, MapInstanceGraphPhase::Ready);
    assert_eq!(app.world().resource::<MapInstanceGraph>().instances[1].path.to_string(), "MAP-parent/village");
}

#[test]
fn continuous_pose_preview_publishes_complete_frames_but_rejects_changed_source_identity() {
    use bevy::{asset::{AssetEvent, AssetLoadFailedEvent}, prelude::*, tasks::AsyncComputeTaskPool};
    use super::resources::{GraphCompletion, GraphIdentity, MapInstanceGraphWork};
    use crate::{MapContentRegistry, MapContentHandles};
    let mut app = App::new();
    app.add_plugins(MinimalPlugins).init_resource::<Assets<MapManifestAsset>>()
        .init_resource::<MapInstanceRootRequest>().init_resource::<MapInstanceAuthorPreview>()
        .init_resource::<MapInstanceGraph>().init_resource::<MapInstanceGraphWork>()
        .add_message::<AssetEvent<MapManifestAsset>>().add_message::<AssetLoadFailedEvent<MapManifestAsset>>()
        .add_systems(Update, super::systems::drive_instance_graph);
    let initial = instance("child", "MAP-child", 0.0, 0.0);
    let sources = graph(vec![source("MAP-parent", vec![initial.clone()]), source("MAP-child", vec![])]);
    app.insert_resource(MapContentRegistry { catalog: default(), revision: 1, last_error: None,
        maps: BTreeMap::from([(id("MAP-parent"), MapContentHandles { manifest: default(), portals: default(),
            placements: default(), surface: None, vegetation: None, water: None, boundaries: None, characters_path: None })]) });
    app.world_mut().resource_mut::<MapInstanceRootRequest>().map_id = Some(id("MAP-parent"));
    app.world_mut().insert_resource(MapInstanceAuthorPreview { map_id: Some(id("MAP-parent")), revision: 2,
        instances: Arc::from([instance("child", "MAP-child", 10.0, 0.0)]), terrains: None });
    {
        let mut work = app.world_mut().resource_mut::<MapInstanceGraphWork>();
        work.catalog_revision = Some(1); work.revision = 5;
        let frozen = sources.clone();
        let result: Arc<[ExpandedMapInstance]> = expand_map_instances(&id("MAP-parent"), &frozen).unwrap().into();
        work.task = Some(AsyncComputeTaskPool::get().spawn(async move { GraphCompletion {
            identity: GraphIdentity { root: Some(id("MAP-parent")), revision: 5, author_revision: 1 },
            sources: frozen, result: Ok(result), pending: default(), waiting: false, root_references: Arc::from([initial]) } }));
    }
    let await_task = |app: &mut App| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.world().resource::<MapInstanceGraphWork>().task.is_none() {
            assert!(std::time::Instant::now() < deadline);
            app.update(); // 完整旧树的后台释放仍占单飞席位，先非阻塞等到下一姿态任务启动。
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        while app.world().resource::<MapInstanceGraphWork>().task.as_ref().is_some_and(|task| !task.is_finished()) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    };
    await_task(&mut app);
    app.update();
    assert_eq!(app.world().resource::<MapInstanceGraph>().instances.len(), 2);
    assert_eq!(app.world().resource::<MapInstanceGraph>().instances[1].transform.translation[0], 0.0);
    assert_eq!(app.world().resource::<MapInstanceGraph>().phase, MapInstanceGraphPhase::Ready);
    let structural_revision = app.world().resource::<MapInstanceGraph>().structural_revision;
    // 模拟每次后台完成前鼠标都前进一帧，完整结果仍持续发布，结构身份不漂移。
    for step in 3..7 {
        let previous_revision = app.world().resource::<MapInstanceGraph>().revision;
        let expected_pose = app.world().resource::<MapInstanceAuthorPreview>().instances[0].transform;
        await_task(&mut app);
        let mut preview = app.world_mut().resource_mut::<MapInstanceAuthorPreview>();
        preview.revision = step; preview.instances = Arc::from([instance("child", "MAP-child", step as f32 * 10.0, 0.0)]);
        app.update();
        let complete = app.world().resource::<MapInstanceGraph>();
        assert!(complete.revision > previous_revision);
        assert_eq!(complete.instances[1].transform, expected_pose);
        assert_eq!(complete.structural_revision, structural_revision);
        assert_eq!(complete.phase, MapInstanceGraphPhase::Ready);
    }
    await_task(&mut app);
    let complete = app.world().resource::<MapInstanceGraph>().instances.clone();
    let mut preview = app.world_mut().resource_mut::<MapInstanceAuthorPreview>();
    preview.revision += 1; preview.instances = Arc::from([instance("child", "MAP-another-source", 0.0, 0.0)]);
    app.update();
    assert!(Arc::ptr_eq(&complete, &app.world().resource::<MapInstanceGraph>().instances));
}

#[test]
fn instance_xyz_protocol_defaults_omit_zero_and_reject_nonfinite_angles() {
    let legacy = serde_json::json!({"translation": [3.0, -4.0, 5.0], "yaw_degrees": 73.0});
    let pose: MapInstanceTransform = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!((pose.pitch_degrees, pose.roll_degrees), (0., 0.));
    assert_eq!(serde_json::to_value(pose).unwrap(), legacy);
    let tilted = MapInstanceTransform { pitch_degrees: -28., roll_degrees: 19., ..pose };
    assert_eq!(serde_json::from_slice::<MapInstanceTransform>(&serde_json::to_vec(&tilted).unwrap()).unwrap(), tilted);
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for axis in 0..3 {
            let mut child = instance("child", "source", 0., 0.);
            match axis { 0 => child.transform.pitch_degrees = invalid,
                1 => child.transform.yaw_degrees = invalid, _ => child.transform.roll_degrees = invalid }
            assert!(validate_map_instances(&id("parent"), &[child]).is_err());
        }
    }
}

#[test]
fn instance_xyz_composition_keeps_legacy_yaw_sum_and_multiplies_tilted_frames() {
    let child = MapInstanceTransform { translation: [3., 8., -4.], yaw_degrees: 27.,
        pitch_degrees: -22., roll_degrees: 31. };
    for parent in [
        MapInstanceTransform { translation: [10., -3., 7.], yaw_degrees: 73., ..Default::default() },
        MapInstanceTransform { translation: [10., -3., 7.], yaw_degrees: 73., pitch_degrees: 26., roll_degrees: -17. },
    ] {
        let result = parent.compose(&child);
        result.validate().unwrap();
        if parent.pitch_degrees == 0. {
            assert_eq!(result.yaw_degrees, 100., "纯父Yaw链保留原精确角度相加");
            assert_eq!(result.pitch_degrees, child.pitch_degrees);
            assert_eq!(result.roll_degrees, child.roll_degrees);
        }
        // 姿态必须等价于先子后父的完整矩阵，而非欧拉角逐轴相加。
        for point in [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(2., -3., 5.)] {
            let expected = parent.to_world(child.to_world(point));
            assert!(result.to_world(point).abs_diff_eq(expected, 0.0001));
            assert!(result.to_local(expected).abs_diff_eq(point, 0.0001));
        }
    }
}

#[test]
fn nested_xyz_instances_and_scaled_terrain_keep_world_pose_after_manifest_reopen() {
    let mut parent = manifest_fixture(); parent.id = id("parent");
    let mut village = manifest_fixture(); village.id = id("village");
    let mut house = manifest_fixture(); house.id = id("house");
    let outer = MapInstanceTransform { translation: [10., 7., -12.], yaw_degrees: 37.,
        pitch_degrees: -28., roll_degrees: 19. };
    let inner = MapInstanceTransform { translation: [-3., 2., 5.], yaw_degrees: -41.,
        pitch_degrees: 16., roll_degrees: -33. };
    let terrain_pose = crate::TerrainObjectTransform { translation: [4., 6., -2.], yaw_degrees: 21.,
        pitch_degrees: 32., roll_degrees: -13., scale: [2., 3., 0.5] };
    let mut outer_ref = instance("village-ref", "village", 0., 0.); outer_ref.transform = outer;
    let mut inner_ref = instance("house-ref", "house", 0., 0.); inner_ref.transform = inner;
    parent.sub_maps = Arc::from([outer_ref]);
    village.sub_maps = Arc::from([inner_ref]);
    let object: crate::MapTerrainObject = serde_json::from_value(serde_json::json!({
        "id": "terrain-tilted", "display_name": "倾斜网格", "enabled": true,
        "transform": terrain_pose, "world_min": [-8.0, -8.0], "world_max": [8.0, 8.0],
        "terrain": house.terrain,
    })).unwrap();
    house.terrain = crate::MapTerrainDescriptor::empty();
    house.terrains = Some(Arc::from([object.clone()]));
    // 真正经过清单序列化重开，避免展开测试只保留内存中新加的角度。
    let sources = graph(vec![parent, village, house].into_iter().map(|map| {
        let reopened: MapManifestAsset = serde_json::from_slice(&serde_json::to_vec(&map).unwrap()).unwrap();
        Arc::new(MapInstanceSource { map_id: reopened.id.clone(), revision: 1,
            world_bounds: reopened.world_bounds().unwrap(), instances: reopened.sub_maps,
            terrains: reopened.terrains })
    }).collect());
    let nodes = expand_map_instances(&id("parent"), &sources).unwrap();
    let terrain = nodes.iter().find(|node| node.terrain.is_some()).unwrap();
    assert_eq!(terrain.path.to_string(), "parent/village-ref/house-ref/terrain:terrain-tilted");
    assert_eq!(terrain.terrain.as_ref().unwrap().transform, terrain_pose);
    assert_eq!(terrain.local_transform.pitch_degrees, terrain_pose.pitch_degrees);
    assert_eq!(terrain.local_transform.roll_degrees, terrain_pose.roll_degrees);
    let expected = outer.as_transform().to_matrix() * inner.as_transform().to_matrix()
        * terrain_pose.as_transform().to_matrix();
    for point in [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(2., -3., 5.)] {
        let world = terrain.world_pose().transform_point(point);
        assert!(world.abs_diff_eq(expected.transform_point3(point), 0.0002));
        assert!(terrain.world_pose().to_matrix().inverse().transform_point3(world).abs_diff_eq(point, 0.0002));
    }
}
