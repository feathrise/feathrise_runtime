//! 真实动态地图监听器的清单身份与通知消费回归；内存资产覆盖同图投影和独立实例域。

use super::{
    DynamicMapRuntimePhase, DynamicMapRuntimeState, DynamicMapRuntimeStatus,
    observe_dynamic_map_assets,
};
use bevy::{
    asset::{AssetId, AssetLoadError, AssetLoadFailedEvent},
    prelude::*,
};
use feathrise_map_content::{
    ActiveDynamicMap, MAP_SCHEMA_VERSION, MapContentHandles, MapContentRegistry, MapId,
    MapManifestAsset, MapTerrainDescriptor,
};
use feathrise_scene_core::residency::{
    InstanceRes, InstanceResourceTable, MapRuntimeScope, ResidentAssetEvent,
    install_instance_asset_journal,
};
use std::{collections::BTreeMap, sync::Arc};

const INITIAL_ENVIRONMENT_REVISION: u64 = 11;

struct Fixture {
    app: App,
    map: MapId,
    registered: Handle<MapManifestAsset>,
    projection: Handle<MapManifestAsset>,
    other: Handle<MapManifestAsset>,
}

fn manifest(map: MapId, label: &str) -> MapManifestAsset {
    MapManifestAsset {
        vegetation_paint_groups: Default::default(),
        schema_version: MAP_SCHEMA_VERSION,
        id: map,
        display_name: label.into(),
        notes: String::new(),
        runtime_scene: "DynamicMapScene".into(),
        world_min: [-8.; 2],
        world_max: [8.; 2],
        default_spawn: [0.; 3],
        environment: Default::default(),
        terrain: MapTerrainDescriptor::empty(),
        terrains: Some(Arc::from([])),
        sub_maps: Default::default(),
        puddles: None,
        asset_roots: Vec::new(),
    }
}

fn handles(manifest: Handle<MapManifestAsset>) -> MapContentHandles {
    MapContentHandles {
        manifest,
        portals: Handle::default(),
        placements: Handle::default(),
        characters_path: None,
        surface: None,
        vegetation: None,
        water: None,
        boundaries: None,
    }
}

fn failure(id: AssetId<MapManifestAsset>, map: &MapId) -> AssetLoadFailedEvent<MapManifestAsset> {
    AssetLoadFailedEvent {
        id,
        path: format!("maps/{map}/map.map_manifest.json").into(),
        error: AssetLoadError::AssetMetaReadError,
    }
}

impl Fixture {
    fn new() -> Self {
        let map = MapId::parse("MAP-observer-current").unwrap();
        let other_map = MapId::parse("MAP-observer-other").unwrap();
        let mut app = App::new();
        // 不安装资产IO或窗口插件，只把正式消息通道送入生产系统；测试不会接触用户地图文件。
        app.init_resource::<Assets<MapManifestAsset>>()
            .add_message::<AssetEvent<MapManifestAsset>>()
            .add_message::<ResidentAssetEvent<MapManifestAsset>>()
            .add_message::<AssetLoadFailedEvent<MapManifestAsset>>()
            .add_message::<AssetLoadFailedEvent<WorldAsset>>()
            .add_systems(Update, observe_dynamic_map_assets);
        let (registered, projection, other) = {
            let mut assets = app.world_mut().resource_mut::<Assets<MapManifestAsset>>();
            (
                assets.add(manifest(map.clone(), "正式清单")),
                assets.add(manifest(map.clone(), "同图私有投影")),
                assets.add(manifest(other_map.clone(), "其他地图")),
            )
        };
        app.insert_resource(ActiveDynamicMap {
            map_id: Some(map.clone()),
        })
        .insert_resource(MapContentRegistry {
            catalog: Handle::default(),
            revision: 3,
            maps: BTreeMap::from([
                (map.clone(), handles(registered.clone())),
                (other_map, handles(other.clone())),
            ]),
            last_error: None,
        })
        .insert_resource(DynamicMapRuntimeState {
            dirty: false,
            catalog_revision: 3,
            environment_revision: INITIAL_ENVIRONMENT_REVISION,
            ..default()
        })
        .insert_resource(DynamicMapRuntimeStatus {
            phase: DynamicMapRuntimePhase::Ready,
            map_id: Some(map.clone()),
            detail: "原完整地图".into(),
            ..default()
        });
        Self {
            app,
            map,
            registered,
            projection,
            other,
        }
    }

    fn assert_root_stable(&self) {
        let state = self.app.world().resource::<DynamicMapRuntimeState>();
        assert!(!state.dirty, "无关清单不能重启当前地形准备");
        assert_eq!(state.environment_revision, INITIAL_ENVIRONMENT_REVISION);
        let status = self.app.world().resource::<DynamicMapRuntimeStatus>();
        assert_eq!(status.phase, DynamicMapRuntimePhase::Ready);
        assert_eq!(status.detail, "原完整地图");
    }
}

#[test]
fn unrelated_manifest_lifecycle_and_failures_leave_the_current_map_untouched() {
    let mut fixture = Fixture::new();
    assert_ne!(fixture.registered.id(), fixture.projection.id());
    for id in [fixture.projection.id(), fixture.other.id()] {
        for event in [
            AssetEvent::Added { id },
            AssetEvent::Modified { id },
            AssetEvent::Removed { id },
            AssetEvent::Unused { id },
            AssetEvent::LoadedWithDependencies { id },
        ] {
            fixture.app.world_mut().write_message(event);
        }
        // 故意共用当前地图路径：失败只能依据AssetId归属，不能凭文件名误认同图投影。
        fixture
            .app
            .world_mut()
            .write_message(failure(id, &fixture.map));
    }
    for _ in 0..4 {
        fixture.app.update();
        fixture.assert_root_stable();
    }
}

#[test]
fn mixed_original_and_resident_notifications_are_consumed_in_one_update() {
    let mut fixture = Fixture::new();
    let current = fixture.registered.id();
    let unrelated = fixture.projection.id();
    for event in [
        AssetEvent::Added { id: unrelated },
        AssetEvent::Modified { id: current },
        AssetEvent::Removed { id: unrelated },
        AssetEvent::LoadedWithDependencies { id: current },
    ] {
        fixture.app.world_mut().write_message(event);
    }
    for event in [
        AssetEvent::Modified { id: unrelated },
        AssetEvent::Modified { id: current },
        AssetEvent::Added { id: current },
    ] {
        fixture
            .app
            .world_mut()
            .write_message(ResidentAssetEvent(event));
    }
    fixture.app.update();
    let state = fixture.app.world().resource::<DynamicMapRuntimeState>();
    assert!(state.dirty);
    assert_eq!(state.environment_revision, INITIAL_ENVIRONMENT_REVISION + 1);
    fixture
        .app
        .world_mut()
        .resource_mut::<DynamicMapRuntimeState>()
        .dirty = false;
    // 模拟地形系统处理完成；同一轮剩余消息不能在后续帧再次制造环境修订。
    for _ in 0..4 {
        fixture.app.update();
        let state = fixture.app.world().resource::<DynamicMapRuntimeState>();
        assert!(!state.dirty, "已消费的混合通知不能连续重刷");
        assert_eq!(state.environment_revision, INITIAL_ENVIRONMENT_REVISION + 1);
    }
}

#[derive(Resource, Default)]
struct RuntimeManifest(Option<Handle<MapManifestAsset>>);

fn capture_runtime_manifest(
    active: InstanceRes<ActiveDynamicMap>,
    catalog: crate::maps::dynamic_map::sources::MapRuntimeCatalog,
    mut probe: ResMut<RuntimeManifest>,
) {
    probe.0 = active
        .map_id
        .as_ref()
        .and_then(|map| catalog.handles(map))
        .map(|handles| handles.manifest.clone());
}

#[test]
fn the_parent_container_accepts_its_actual_runtime_catalog_projection() {
    let mut fixture = Fixture::new();
    fixture
        .app
        .init_resource::<feathrise_map_content::MapInstanceAuthorPreview>()
        .init_resource::<RuntimeManifest>()
        .add_systems(Update, capture_runtime_manifest);
    crate::maps::dynamic_map::sources::install(&mut fixture.app);
    fixture.app.update(); // 正式PreUpdate构造容器投影，不能把全局目录原句柄冒充实际运行源。
    fixture.assert_root_stable();
    let projected = fixture
        .app
        .world()
        .resource::<RuntimeManifest>()
        .0
        .clone()
        .unwrap();
    assert_ne!(projected.id(), fixture.registered.id());
    fixture
        .app
        .world_mut()
        .write_message(AssetEvent::Modified { id: projected.id() });
    fixture.app.update();
    let state = fixture.app.world().resource::<DynamicMapRuntimeState>();
    assert!(state.dirty, "容器实际投影的Modified仍须使当前环境换代");
    assert_eq!(state.environment_revision, INITIAL_ENVIRONMENT_REVISION + 1);
    fixture
        .app
        .world_mut()
        .resource_mut::<DynamicMapRuntimeState>()
        .dirty = false;
    fixture.app.update();
    let state = fixture.app.world().resource::<DynamicMapRuntimeState>();
    assert!(!state.dirty);
    assert_eq!(state.environment_revision, INITIAL_ENVIRONMENT_REVISION + 1);
}

#[test]
fn an_instance_uses_its_registered_projection_and_drains_the_asset_journal() {
    let mut fixture = Fixture::new();
    let child_map = MapId::parse("MAP-observer-other").unwrap();
    let child = fixture.app.world_mut().spawn_empty().id();
    let projection = fixture
        .app
        .world_mut()
        .resource_mut::<Assets<MapManifestAsset>>()
        .add(manifest(child_map.clone(), "子实例合法投影"));
    // 父图选择与子实例选择不同，且子实例登记合法投影，证明监听器同时遵循两个域参数。
    fixture
        .app
        .insert_resource(MapRuntimeScope {
            instance: Some(child),
            ..default()
        })
        .insert_resource(InstanceResourceTable::<ActiveDynamicMap> {
            values: BTreeMap::from([(
                child,
                ActiveDynamicMap {
                    map_id: Some(child_map.clone()),
                },
            )]),
        })
        .insert_resource(InstanceResourceTable::<MapContentRegistry> {
            values: BTreeMap::from([(
                child,
                MapContentRegistry {
                    catalog: Handle::default(),
                    revision: 5,
                    maps: BTreeMap::from([(child_map.clone(), handles(projection.clone()))]),
                    last_error: None,
                },
            )]),
        })
        .insert_resource(InstanceResourceTable::<DynamicMapRuntimeState> {
            values: BTreeMap::from([(
                child,
                DynamicMapRuntimeState {
                    dirty: false,
                    catalog_revision: 5,
                    environment_revision: 23,
                    ..default()
                },
            )]),
        })
        .insert_resource(InstanceResourceTable::<DynamicMapRuntimeStatus> {
            values: BTreeMap::from([(
                child,
                DynamicMapRuntimeStatus {
                    phase: DynamicMapRuntimePhase::Ready,
                    map_id: Some(child_map.clone()),
                    ..default()
                },
            )]),
        });
    install_instance_asset_journal::<MapManifestAsset>(&mut fixture.app);
    for id in [fixture.registered.id(), fixture.other.id()] {
        fixture
            .app
            .world_mut()
            .write_message(AssetEvent::Modified { id });
        fixture
            .app
            .world_mut()
            .write_message(failure(id, &child_map));
    }
    fixture.app.update();
    let states = fixture
        .app
        .world()
        .resource::<InstanceResourceTable<DynamicMapRuntimeState>>();
    assert!(!states.values[&child].dirty);
    assert_eq!(states.values[&child].environment_revision, 23);
    assert_eq!(
        fixture
            .app
            .world()
            .resource::<InstanceResourceTable<DynamicMapRuntimeStatus>>()
            .values[&child]
            .phase,
        DynamicMapRuntimePhase::Ready
    );
    fixture.assert_root_stable();

    let id = projection.id();
    fixture
        .app
        .world_mut()
        .write_message(AssetEvent::Modified { id });
    fixture
        .app
        .world_mut()
        .write_message(ResidentAssetEvent(AssetEvent::Modified { id }));
    fixture.app.update();
    let states = fixture
        .app
        .world()
        .resource::<InstanceResourceTable<DynamicMapRuntimeState>>();
    assert!(
        states.values[&child].dirty,
        "域登记的合法投影Modified必须生效"
    );
    assert_eq!(states.values[&child].environment_revision, 24);
    fixture
        .app
        .world_mut()
        .resource_mut::<InstanceResourceTable<DynamicMapRuntimeState>>()
        .values
        .get_mut(&child)
        .unwrap()
        .dirty = false;
    for _ in 0..4 {
        fixture.app.update();
        let states = fixture
            .app
            .world()
            .resource::<InstanceResourceTable<DynamicMapRuntimeState>>();
        assert!(
            !states.values[&child].dirty,
            "轮转日志必须完整消费，不能遗留投影通知"
        );
        assert_eq!(states.values[&child].environment_revision, 24);
        fixture.assert_root_stable();
    }
}

#[test]
fn current_manifest_failure_still_reports_a_loading_failure() {
    let mut fixture = Fixture::new();
    fixture
        .app
        .world_mut()
        .write_message(failure(fixture.registered.id(), &fixture.map));
    fixture.app.update();
    let status = fixture.app.world().resource::<DynamicMapRuntimeStatus>();
    assert_eq!(status.phase, DynamicMapRuntimePhase::Failed);
    assert!(status.detail.contains("动态地图清单加载失败"));
    assert!(status.detail.contains(&fixture.map.to_string()));
}

#[test]
fn catalog_revision_changes_still_invalidate_the_environment() {
    let mut fixture = Fixture::new();
    fixture
        .app
        .world_mut()
        .resource_mut::<MapContentRegistry>()
        .revision += 1;
    fixture.app.update();
    let state = fixture.app.world().resource::<DynamicMapRuntimeState>();
    assert!(state.dirty);
    assert_eq!(state.environment_revision, INITIAL_ENVIRONMENT_REVISION + 1);
}
