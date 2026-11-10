//! 同场景网格共存时，空网格采用不能撤销另一网格已可见草窗的绘制配置。
use super::*;
use bevy::ecs::system::RunSystemOnce;
use feathrise_environment::vegetation::blade::{
    GpuBladeProfileBank, GpuBladeProfileBankId, GpuBladeProfiles,
};
use std::sync::Arc;

fn commit_empty(
    mut commands: Commands,
    mut state: ResMut<DynamicVegetationState>,
    mut status: ResMut<DynamicVegetationStatus>,
) {
    commit_staged_vegetation(&mut commands, &mut state, &mut status);
}

#[test]
fn profile_bank_scope_empty_second_mesh_keeps_first_grass_drawable() {
    let mut world = World::new();
    let first = world.spawn(Visibility::Inherited).id();
    let bank = GpuBladeProfileBank::new(GpuBladeProfiles::default(), None);
    let mut banks = GpuBladeProfileBanks::default();
    banks.replace(vec![bank.clone()]);
    world.insert_resource(banks);
    let id = GpuBladeProfileBankId::for_dynamic_index(0).unwrap();
    let window = world.spawn((ChildOf(first), id)).id();
    let second = world.spawn(Visibility::Hidden).id();
    world.insert_resource(DynamicVegetationState {
        candidate_root: Some(super::super::resources::DynamicVegetationRootBuild {
            root: second,
            blade_layer_count: 0,
            blade_window_count: 0,
        }),
        candidate_mesh_source: Some(Arc::new(super::super::resources::DynamicVegetationBuild {
            generation: 1,
            revision: 1,
            mesh_layers: vec![],
            volume_mesh_layers: Arc::from([]),
            blade_layers: vec![],
            ground: super::super::resources::DynamicVegetationGroundSource::Clipmap,
            world_min: [-1.; 2],
            world_max: [1.; 2],
        })),
        ..default()
    });
    world.init_resource::<DynamicVegetationStatus>();
    world.run_system_once(commit_empty).unwrap();
    assert!(world.get_entity(window).is_ok());
    assert_eq!(world.get::<GpuBladeProfileBankId>(window), Some(&id));
    assert_eq!(
        world
            .resource::<GpuBladeProfileBanks>()
            .banks()
            .get(id.index() as usize - 1),
        Some(&bank),
        "采用没有植被的第二网格不能清空第一网格仍在使用的GPU绘制目录"
    );
}

fn grass_pending(app: &mut App) -> super::super::resources::DynamicVegetationPending {
    use feathrise_scene_content::*;
    let manifest: SceneManifestAsset = serde_json::from_value(serde_json::json!({
        "schema_version":2,"id":"MAP-two-grass-meshes","display_name":"双网格草地回归",
        "runtime_scene":"data_driven","world_min":[0,0],"world_max":[10,10],"default_spawn":[5,0,5],
        "terrain":{"kind":"heightfield","glb_path":"scenes/test/terrain.glb","resolution":[2,2],"base_height":0,"height_scale":10}
    })).unwrap();
    let catalog: VegetationPresetCatalogAsset = serde_json::from_str(
        crate::test_fixtures::VEGETATION_CATALOG_JSON,
    )
    .unwrap();
    let presets = VegetationPresetRegistry {
        catalog: Handle::default(),
        revision: 1,
        resolved: ResolvedVegetationPresetCatalog::build(&catalog).unwrap(),
        last_error: None,
    };
    let mut layer = presets
        .resolve("hn01_ordinary_grass")
        .unwrap()
        .create_scene_layer("scenes/test");
    layer.resolution = [2; 2];
    let image = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::default());
    let snapshot = super::super::DynamicBladeDraftSnapshot {
        source_revision: 1,
        sources: [(layer.id.clone(), Arc::from([200; 4]))].into(),
        densities: [(layer.id.clone(), image.clone())].into(),
        nonzero_layers: [layer.id.clone()].into(),
        definition: SceneVegetationAsset {
            schema_version: feathrise_scene_content::SCENE_SCHEMA_VERSION,
            scene_id: manifest.id.clone(),
            layers: vec![layer],
            authoring: None,
        },
    };
    super::super::helpers::load_pending_assets(
        app.world().resource::<AssetServer>(),
        snapshot.definition.clone(),
        manifest,
        &presets,
        Some(image),
        Arc::from([0; 4]),
        1,
        1,
        1,
        Some(&snapshot),
    )
    .unwrap()
}

fn install(
    app: &mut App,
    mut pending: super::super::resources::DynamicVegetationPending,
) -> Entity {
    let old;
    {
        let mut state = app.world_mut().resource_mut::<DynamicVegetationState>();
        old = state.active_root;
        state.scene_id = Some(pending.scene_id.clone());
        state.generation = 1;
        state.revision += 1;
        pending.revision = state.revision;
        let build = super::super::resources::DynamicVegetationBuild {
            generation: 1,
            revision: pending.revision,
            mesh_layers: vec![],
            volume_mesh_layers: Arc::from([]),
            blade_layers: vec![],
            ground: super::super::resources::DynamicVegetationGroundSource::Clipmap,
            world_min: [0.; 2],
            world_max: [10.; 2],
        };
        state.task = Some(AsyncComputeTaskPool::get().spawn(async move { Ok(build) }));
        state.pending = Some(pending);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        app.world_mut()
            .run_system_once(install_dynamic_vegetation)
            .unwrap();
        let root = app.world().resource::<DynamicVegetationState>().active_root;
        if root.is_some() && root != old {
            return root.unwrap();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "真实后台结果必须有限完成并采用"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn profile_bank_scope_real_windows_survive_sibling_rebuild_clear_and_gameplay() {
    use bevy::asset::AssetPlugin;
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<Image>()
        .init_asset::<Mesh>()
        .init_resource::<GpuBladeProfileBanks>()
        .init_resource::<DynamicTerrainHeightCache>()
        .init_resource::<DynamicVegetationState>()
        .init_resource::<DynamicVegetationStatus>();
    let first_pending = grass_pending(&mut app);
    let expected = first_pending.blade_profile_banks[0].clone();
    let first_root = install(&mut app, first_pending.clone());
    let first_state = app
        .world_mut()
        .remove_resource::<DynamicVegetationState>()
        .unwrap();
    app.init_resource::<DynamicVegetationState>();
    let mut query = app
        .world_mut()
        .query::<(Entity, &ChildOf, &GpuBladeProfileBankId)>();
    let first_windows: Vec<_> = query
        .iter(app.world())
        .filter(|(_, parent, _)| parent.parent() == first_root)
        .map(|(entity, _, id)| (entity, *id))
        .collect();
    assert_eq!(
        first_windows.len(),
        first_pending.layers[0].blade_meshes.as_ref().unwrap().len()
    );
    let mut second_pending = first_pending;
    let mut appearance = feathrise_environment::vegetation::blade::GpuBladeProfile::DEFAULT;
    appearance.root_color = Vec4::ONE;
    second_pending.blade_profile_banks[0].profiles = GpuBladeProfiles::new([appearance; 4]);
    let second_expected = second_pending.blade_profile_banks[0].clone();
    let second_root = install(&mut app, second_pending.clone());
    assert_ne!(first_root, second_root);
    let second_windows: Vec<_> = query
        .iter(app.world())
        .filter(|(_, parent, _)| parent.parent() == second_root)
        .map(|(_, _, id)| *id)
        .collect();
    assert_eq!(second_windows.len(), first_windows.len());
    for id in second_windows {
        assert!(!first_windows.iter().any(|(_, first)| *first == id));
        assert_eq!(
            app.world().resource::<GpuBladeProfileBanks>().banks()[id.index() as usize - 1],
            second_expected
        );
    }
    for (window, id) in &first_windows {
        assert_eq!(app.world().get::<GpuBladeProfileBankId>(*window), Some(id));
        assert_eq!(
            app.world().resource::<GpuBladeProfileBanks>().banks()[id.index() as usize - 1],
            expected
        );
    }
    second_pending.layers.clear();
    second_pending.blade_profile_banks.clear();
    install(&mut app, second_pending); // 第二网格清空/末作者层移除采用后，第一网格仍可绘制。
    app.world_mut()
        .run_system_once(clear_dynamic_vegetation)
        .unwrap(); // 玩法清理当前域同样不能清空兄弟目录。
    for (window, id) in &first_windows {
        assert!(app.world().get_entity(*window).is_ok());
        assert_eq!(
            app.world().resource::<GpuBladeProfileBanks>().banks()[id.index() as usize - 1],
            expected
        );
    }
    app.insert_resource(first_state);
    app.world_mut()
        .run_system_once(clear_dynamic_vegetation)
        .unwrap();
    assert!(
        app.world().resource::<GpuBladeProfileBanks>().is_empty(),
        "最后植被根离场归还全部槽位"
    );
}
