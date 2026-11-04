//! 已登记空植被的生产采用回归；无地形、清空旧根与切图迟到结果均沿正式系统验证。

use super::*;
use bevy::{asset::AssetPlugin, ecs::system::RunSystemOnce};
use feathrise_map_content::*;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

struct Fixture {
    app: App,
    map: MapId,
    definition: Handle<MapVegetationAsset>,
}

impl Fixture {
    fn new() -> Self {
        let map = MapId::parse("MAP-empty-vegetation-definition").unwrap();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Image>()
            .init_asset::<Mesh>()
            .init_asset::<MapManifestAsset>()
            .init_asset::<MapVegetationAsset>()
            .init_resource::<DynamicMeshProfileRequests>()
            .init_resource::<DynamicMeshProfileState>()
            .init_resource::<DynamicMeshProfileCache>()
            .init_resource::<VegetationMeshPrototypeRegistry>()
            .init_resource::<GpuBladeProfileBanks>()
            .init_resource::<DynamicTerrainHeightCache>()
            .init_resource::<super::super::DynamicBladeDraft>()
            .init_resource::<DynamicVegetationStatus>()
            .init_resource::<MapInstanceAuthorPreview>();
        let manifest: MapManifestAsset = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "id": map, "display_name": "空植被容器",
            "runtime_scene": "data_driven", "world_min": [-0.5, -0.5],
            "world_max": [0.5, 0.5], "default_spawn": [0, 2, 0], "terrains": []
        }))
        .unwrap();
        let manifest = app
            .world_mut()
            .resource_mut::<Assets<MapManifestAsset>>()
            .add(manifest);
        // 新建地图会保留空作者配置和登记路径，不能用“完全未声明植被”替代此场景。
        let definition: MapVegetationAsset = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "map_id": map, "layers": [],
            "authoring": {"groups": [], "layers": []}
        }))
        .unwrap();
        let definition = app
            .world_mut()
            .resource_mut::<Assets<MapVegetationAsset>>()
            .add(definition);
        let catalog: VegetationPresetCatalogAsset =
            serde_json::from_str(crate::test_fixtures::VEGETATION_CATALOG_JSON).unwrap();
        app.insert_resource(VegetationPresetRegistry {
            catalog: Handle::default(),
            revision: 1,
            resolved: ResolvedVegetationPresetCatalog::build(&catalog).unwrap(),
            last_error: None,
        })
        .insert_resource(ActiveDynamicMap {
            map_id: Some(map.clone()),
        })
        .insert_resource(MapContentRegistry {
            catalog: Handle::default(),
            revision: 1,
            maps: BTreeMap::from([(
                map.clone(),
                MapContentHandles {
                    manifest,
                    vegetation: Some(definition.clone()),
                    portals: Handle::default(),
                    placements: Handle::default(),
                    characters_path: None,
                    surface: None,
                    water: None,
                    boundaries: None,
                },
            )]),
            last_error: None,
        })
        .insert_resource(DynamicVegetationState {
            map_id: Some(map.clone()),
            generation: 5,
            revision: 7,
            dirty: true,
            ..default()
        });
        app.finish();
        app.cleanup();
        Self {
            app,
            map,
            definition,
        }
    }

    fn prepare(&mut self) {
        self.app
            .world_mut()
            .run_system_once(super::super::loading::prepare_dynamic_vegetation_definition)
            .unwrap();
    }

    fn start(&mut self) {
        self.app
            .world_mut()
            .run_system_once(start_dynamic_vegetation_build)
            .unwrap();
    }

    fn finish_task(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self
            .app
            .world()
            .resource::<DynamicVegetationState>()
            .task
            .is_some()
        {
            self.app
                .world_mut()
                .run_system_once(install_dynamic_vegetation)
                .unwrap();
            assert!(Instant::now() < deadline, "真实零层后台任务应有限完成");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[test]
fn registered_empty_container_vegetation_adopts_without_a_height_source() {
    let mut fixture = Fixture::new();
    fixture.prepare();
    {
        let state = fixture.app.world().resource::<DynamicVegetationState>();
        assert!(
            state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.layers.is_empty())
        );
        assert!(state.terrain_generation.is_none());
        assert_eq!(
            fixture
                .app
                .world()
                .resource::<DynamicVegetationStatus>()
                .phase,
            DynamicVegetationPhase::Waiting,
            "准备阶段不能冒充正式采用"
        );
    }
    fixture.start();
    assert!(
        fixture
            .app
            .world()
            .resource::<DynamicVegetationState>()
            .task
            .is_some()
    );
    fixture.finish_task();
    let state = fixture.app.world().resource::<DynamicVegetationState>();
    assert_eq!((state.generation, state.revision), (5, 7));
    assert!(
        state
            .mesh_source
            .as_ref()
            .is_some_and(|source| source.mesh_layers.is_empty())
    );
    assert_eq!(
        fixture
            .app
            .world()
            .get::<Visibility>(state.active_root.unwrap()),
        Some(&Visibility::Inherited)
    );
    let status = fixture.app.world().resource::<DynamicVegetationStatus>();
    assert_eq!(status.map_id, Some(fixture.map));
    assert_eq!(status.phase, DynamicVegetationPhase::Ready);
    assert_eq!(
        (
            status.layer_count,
            status.blade_window_count,
            status.mesh_instance_count
        ),
        (0, 0, 0)
    );
}

#[test]
fn adopting_zero_layers_retires_the_old_visible_tree_without_changing_generation() {
    let mut fixture = Fixture::new();
    let old_root = fixture
        .app
        .world_mut()
        .spawn((Visibility::Inherited, Transform::IDENTITY))
        .id();
    let old_child = fixture
        .app
        .world_mut()
        .spawn((ChildOf(old_root), Name::new("旧植被实例")))
        .id();
    fixture
        .app
        .world_mut()
        .resource_mut::<DynamicVegetationState>()
        .active_root = Some(old_root);
    fixture.prepare();
    assert!(
        fixture.app.world().get_entity(old_child).is_ok(),
        "零层完成前仍保留旧完整根"
    );
    fixture.start();
    fixture.finish_task();
    let state = fixture.app.world().resource::<DynamicVegetationState>();
    assert_eq!((state.generation, state.revision), (5, 7));
    assert_ne!(state.active_root, Some(old_root));
    assert!(fixture.app.world().get_entity(old_root).is_err());
    assert!(fixture.app.world().get_entity(old_child).is_err());
}

#[test]
fn a_nonempty_definition_cannot_bypass_the_missing_height_source() {
    let mut fixture = Fixture::new();
    let mut layer = fixture
        .app
        .world()
        .resource::<VegetationPresetRegistry>()
        .resolve("hn01_ordinary_grass")
        .unwrap()
        .create_map_layer("maps/empty-test");
    layer.resolution = [2; 2];
    fixture
        .app
        .world_mut()
        .resource_mut::<Assets<MapVegetationAsset>>()
        .get_mut(&fixture.definition)
        .unwrap()
        .layers
        .push(layer);
    fixture.prepare();
    fixture.start();
    let state = fixture.app.world().resource::<DynamicVegetationState>();
    assert!(state.pending.is_none() && state.task.is_none() && state.active_root.is_none());
    let status = fixture.app.world().resource::<DynamicVegetationStatus>();
    assert_eq!(status.phase, DynamicVegetationPhase::Failed);
    assert!(
        status.detail.contains("地形分辨率无效"),
        "非空层不能借用空内容特例伪造高度就绪"
    );
}

#[test]
fn an_old_empty_result_cannot_mark_the_next_map_ready() {
    let mut fixture = Fixture::new();
    fixture.prepare();
    fixture.start();
    fixture
        .app
        .world_mut()
        .run_system_once(clear_dynamic_vegetation)
        .unwrap();
    let next_map = MapId::parse("MAP-next-empty-vegetation").unwrap();
    fixture
        .app
        .world_mut()
        .resource_mut::<ActiveDynamicMap>()
        .select(next_map.clone());
    {
        let mut state = fixture
            .app
            .world_mut()
            .resource_mut::<DynamicVegetationState>();
        assert_eq!(state.generation, 6);
        state.map_id = Some(next_map.clone());
        state.revision = 1;
        state.dirty = true;
    }
    fixture
        .app
        .world_mut()
        .resource_mut::<DynamicVegetationStatus>()
        .map_id = Some(next_map.clone());
    fixture.finish_task();
    let state = fixture.app.world().resource::<DynamicVegetationState>();
    assert!(state.active_root.is_none() && state.mesh_source.is_none());
    assert_eq!(state.map_id, Some(next_map.clone()));
    let status = fixture.app.world().resource::<DynamicVegetationStatus>();
    assert_eq!(status.map_id, Some(next_map));
    assert_eq!(status.phase, DynamicVegetationPhase::Waiting);
}

#[test]
fn the_formal_container_projection_keeps_empty_vegetation_ready() {
    let mut fixture = Fixture::new();
    crate::maps::dynamic_map::sources::install(&mut fixture.app);
    fixture.app.update(); // 正式PreUpdate投影剥离容器的兼容地形植被；保留真实分域合同。
    fixture.prepare();
    let state = fixture.app.world().resource::<DynamicVegetationState>();
    assert!(state.definition_handle.is_none() && state.pending.is_none() && state.task.is_none());
    let status = fixture.app.world().resource::<DynamicVegetationStatus>();
    assert_eq!(status.map_id, Some(fixture.map));
    assert_eq!(status.phase, DynamicVegetationPhase::Ready);
}
