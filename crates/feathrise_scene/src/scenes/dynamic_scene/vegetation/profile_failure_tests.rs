//! 必需Mesh预设失败沿原植被就绪门上报；空层、无关请求和旧目录错误不吞掉可恢复的pending。

use super::*;
use bevy::{asset::{AssetPlugin, RenderAssetUsages}, render::render_resource::{Extent3d, TextureDimension, TextureFormat}};
use feathrise_environment::{mesh_wind::{TreeWindMaterial, TreeWindNoiseTexture}, vegetation::VegetationMeshBillboardMaterial};
use feathrise_scene_content::*;
use bevy::ecs::system::RunSystemOnce;
use std::{sync::Arc, time::{Duration, Instant}};
use crate::scenes::dynamic_scene::mesh_profiles::{self, DynamicMeshProfileStatus, DynamicMeshProfilePhase};

fn fixture() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<Image>().init_asset::<Mesh>().init_asset::<StandardMaterial>()
        .init_asset::<TreeWindMaterial>().init_asset::<VegetationMeshBillboardMaterial>()
        .init_resource::<DynamicMeshProfileRequests>().init_resource::<DynamicMeshProfileState>()
        .init_resource::<DynamicMeshProfileStatus>().init_resource::<DynamicMeshProfileCache>()
        .init_resource::<VegetationMeshPrototypeRegistry>()
        .init_resource::<DynamicVegetationState>().init_resource::<DynamicVegetationStatus>()
        .insert_resource(TreeWindNoiseTexture(Handle::default()));
    let catalog: VegetationPresetCatalogAsset = serde_json::from_str(
        crate::test_fixtures::VEGETATION_CATALOG_JSON).unwrap();
    app.insert_resource(VegetationPresetRegistry { catalog: Handle::default(), revision: 1,
        resolved: ResolvedVegetationPresetCatalog::build(&catalog).unwrap(), last_error: None });
    app.finish();
    app.cleanup();
    app
}

fn set_pending(app: &mut App, density: u8, revision: u32) {
    let manifest: SceneManifestAsset = serde_json::from_value(serde_json::json!({
        "schema_version":2,"id":"MAP-profile-failure","display_name":"原型失败传播",
        "runtime_scene":"data_driven","world_min":[0,0],"world_max":[64,64],"default_spawn":[32,0,32],
        "terrain":{"kind":"heightfield","glb_path":"scenes/test/terrain.glb","resolution":[2,2],"base_height":0,"height_scale":16}
    })).unwrap();
    let mut layer = app.world().resource::<VegetationPresetRegistry>()
        .resolve("hn01_tree").unwrap().create_scene_layer("scenes/test");
    layer.resolution = [2; 2];
    let image = Image::new(Extent3d { width: 2, height: 2, depth_or_array_layers: 1 },
        TextureDimension::D2, vec![density; 4], TextureFormat::R8Unorm, RenderAssetUsages::all());
    let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    let snapshot = super::super::DynamicBladeDraftSnapshot {
        source_revision: revision as u64,
        sources: [(layer.id.clone(), Arc::from([density; 4]))].into(),
        densities: [(layer.id.clone(), handle.clone())].into(),
        nonzero_layers: if density == 0 { Default::default() } else { [layer.id.clone()].into() },
        definition: SceneVegetationAsset { schema_version: feathrise_scene_content::SCENE_SCHEMA_VERSION, scene_id: manifest.id.clone(),
            layers: vec![layer], authoring: None },
    };
    let pending = super::super::helpers::load_pending_assets(app.world().resource::<AssetServer>(),
        snapshot.definition.clone(), manifest, app.world().resource::<VegetationPresetRegistry>(),
        Some(handle), Arc::from([0; 4]), 1, 1, revision, Some(&snapshot)).unwrap();
    let mut state = app.world_mut().resource_mut::<DynamicVegetationState>();
    state.generation = 1;
    state.revision = revision;
    state.terrain_generation = Some(1);
    state.pending = Some(pending);
    state.attempted_revision = None;
}

fn drive(app: &mut App) {
    app.update();
    app.world_mut().run_system_once(mesh_profiles::drive_dynamic_mesh_profiles).unwrap();
}

#[test]
fn mesh_profile_failure_required_layer_reports_failure_without_discarding_pending_and_empty_layer_can_continue() {
    let mut app = fixture();
    set_pending(&mut app, 255, 1);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        drive(&mut app);
        app.world_mut().run_system_once(start_dynamic_vegetation_build).unwrap();
        if app.world().resource::<DynamicVegetationStatus>().phase == DynamicVegetationPhase::Failed { break; }
        assert!(Instant::now() < deadline, "真实Mesh资产失败须经植被状态传播，不能永久显示等待");
        std::thread::sleep(Duration::from_millis(1));
    }
    let state = app.world().resource::<DynamicVegetationState>();
    assert!(state.pending.is_some());
    assert!(state.attempted_revision.is_none());
    assert!(state.task.is_none());
    assert!(app.world().resource::<DynamicVegetationStatus>().detail.contains("正式Mesh原型加载失败"));
    set_pending(&mut app, 0, 2);
    app.world_mut().run_system_once(start_dynamic_vegetation_build).unwrap();
    assert_eq!(app.world().resource::<DynamicVegetationStatus>().phase, DynamicVegetationPhase::Building,
        "同一预设的空输出不需要原型，旧错误不能阻止新空修订冻结");
    assert!(app.world().resource::<DynamicVegetationState>().pending.is_some());
}

#[test]
fn mesh_profile_failure_unrelated_failed_request_cannot_starve_new_required_profile_or_borrow_old_catalog_error() {
    let mut app = fixture();
    app.world_mut().resource_mut::<DynamicMeshProfileRequests>().request("hn01_shrub").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.world().resource::<DynamicMeshProfileStatus>().phase != DynamicMeshProfilePhase::Failed {
        drive(&mut app);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    set_pending(&mut app, 255, 1);
    app.world_mut().run_system_once(start_dynamic_vegetation_build).unwrap();
    assert_eq!(app.world().resource::<DynamicVegetationStatus>().phase, DynamicVegetationPhase::Waiting);
    loop {
        drive(&mut app);
        app.world_mut().run_system_once(start_dynamic_vegetation_build).unwrap();
        if app.world().resource::<DynamicMeshProfileCache>().loading_profile() == Some("hn01_tree") { break; }
        assert!(Instant::now() < deadline, "排序在前的无关灌木失败不得阻止必需树木取得自己的加载槽");
    }
    assert_eq!(app.world().resource::<DynamicVegetationStatus>().phase, DynamicVegetationPhase::Waiting);
    app.world_mut().resource_mut::<VegetationPresetRegistry>().revision = 2;
    drive(&mut app);
    app.world_mut().run_system_once(start_dynamic_vegetation_build).unwrap();
    assert_eq!(app.world().resource::<DynamicVegetationStatus>().phase, DynamicVegetationPhase::Waiting,
        "目录换代不能借用旧目录相同ID的失败回执");
    assert!(app.world().resource::<DynamicVegetationState>().pending.is_some());
}
