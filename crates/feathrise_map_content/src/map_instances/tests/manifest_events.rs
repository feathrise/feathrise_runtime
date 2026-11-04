//! 目录登记的清单句柄才是引用图来源；同MapId运行时投影不得反馈成新一轮整图重建。

use super::super::resources::{GraphIdentity, MapInstanceGraphWork};
use super::super::{
    ExpandedMapInstance, MapInstanceAuthorPreview, MapInstanceGraph, MapInstanceGraphPhase,
    MapInstanceRootRequest, MapInstanceSource,
};
use crate::{
    MAP_SCHEMA_VERSION, MapContentHandles, MapContentRegistry, MapId, MapManifestAsset,
    MapTerrainDescriptor,
};
use bevy::{
    asset::{AssetEvent, AssetId, AssetLoadFailedEvent},
    prelude::*,
};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

struct Fixture {
    app: App,
    map: MapId,
    registered: Handle<MapManifestAsset>,
}

impl Fixture {
    fn new() -> Self {
        let map = MapId::parse("MAP-manifest-events").unwrap();
        let mut app = App::new();
        // 沿正式MessageReader与CPU单飞任务驱动图；所有资产仅存在内存，完全不依赖用户文件。
        app.add_plugins(MinimalPlugins)
            .init_resource::<Assets<MapManifestAsset>>()
            .init_resource::<MapInstanceRootRequest>()
            .init_resource::<MapInstanceAuthorPreview>()
            .init_resource::<MapInstanceGraph>()
            .init_resource::<MapInstanceGraphWork>()
            .add_message::<AssetEvent<MapManifestAsset>>()
            .add_message::<AssetLoadFailedEvent<MapManifestAsset>>()
            .add_systems(Update, super::super::systems::drive_instance_graph);
        let registered = app
            .world_mut()
            .resource_mut::<Assets<MapManifestAsset>>()
            .add(MapManifestAsset {
                vegetation_paint_groups: Default::default(),
                schema_version: MAP_SCHEMA_VERSION,
                id: map.clone(),
                display_name: "权威空地图".into(),
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
            });
        app.insert_resource(MapContentRegistry {
            catalog: Handle::default(),
            revision: 1,
            maps: BTreeMap::from([(
                map.clone(),
                MapContentHandles {
                    manifest: registered.clone(),
                    portals: Handle::default(),
                    placements: Handle::default(),
                    characters_path: None,
                    surface: None,
                    vegetation: None,
                    water: None,
                    boundaries: None,
                },
            )]),
            last_error: None,
        });
        app.world_mut()
            .resource_mut::<MapInstanceRootRequest>()
            .map_id = Some(map.clone());
        Self {
            app,
            map,
            registered,
        }
    }

    fn manifest(&self) -> MapManifestAsset {
        self.app
            .world()
            .resource::<Assets<MapManifestAsset>>()
            .get(&self.registered)
            .unwrap()
            .clone()
    }

    fn settle(&mut self, phase: MapInstanceGraphPhase) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.app.update();
            let work = self.app.world().resource::<MapInstanceGraphWork>();
            if work.started.is_some()
                && work.started == work.desired
                && work.task.is_none()
                && work.releasing.is_none()
                && work.queued.is_empty()
                && work.updates.is_empty()
                && !work.scanning
                && !work.pruning
                && self.app.world().resource::<MapInstanceGraph>().phase == phase
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "引用图未完成有界后台发布：期望{phase:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn notify_without_rebuild(
        &mut self,
        stable: &StableGraph,
        event: AssetEvent<MapManifestAsset>,
        label: &str,
    ) {
        self.app.world_mut().write_message(event);
        // 每帧检查任务启动与源修订，不能仅在稍后图版本相同的偶然时间点证明没有重复重建。
        for _ in 0..8 {
            self.app.update();
            stable.assert_unchanged(&self.app, &self.map, label);
        }
    }
}

struct StableGraph {
    graph_revision: u64,
    structural_revision: u64,
    instances: Arc<[ExpandedMapInstance]>,
    source: Arc<MapInstanceSource>,
    source_revision: u64,
    work_revision: u64,
    published_sources_revision: u64,
    desired: Option<GraphIdentity>,
    started: Option<GraphIdentity>,
    handles: BTreeMap<AssetId<MapManifestAsset>, MapId>,
}

impl StableGraph {
    fn capture(fixture: &Fixture) -> Self {
        let graph = fixture.app.world().resource::<MapInstanceGraph>();
        let work = fixture.app.world().resource::<MapInstanceGraphWork>();
        assert_eq!(graph.phase, MapInstanceGraphPhase::Ready);
        assert!(work.task.is_none() && work.releasing.is_none());
        let source = work.sources[&fixture.map].clone();
        Self {
            graph_revision: graph.revision,
            structural_revision: graph.structural_revision,
            instances: graph.instances.clone(),
            source_revision: source.revision,
            source,
            work_revision: work.revision,
            published_sources_revision: work.published_sources_revision,
            desired: work.desired.clone(),
            started: work.started.clone(),
            handles: work.handles.clone(),
        }
    }

    fn assert_unchanged(&self, app: &App, map: &MapId, label: &str) {
        let graph = app.world().resource::<MapInstanceGraph>();
        let work = app.world().resource::<MapInstanceGraphWork>();
        assert_eq!(graph.phase, MapInstanceGraphPhase::Ready, "{label}");
        assert_eq!(
            graph.revision, self.graph_revision,
            "{label}不应再次发布引用图"
        );
        assert_eq!(
            graph.structural_revision, self.structural_revision,
            "{label}不应重建完整实例批次"
        );
        assert!(
            Arc::ptr_eq(&graph.instances, &self.instances),
            "{label}必须保留原完整树"
        );
        assert_eq!(
            work.revision, self.work_revision,
            "{label}不能重新捕获目录来源"
        );
        assert_eq!(work.desired, self.desired, "{label}不能改变期望输入身份");
        assert_eq!(work.started, self.started, "{label}不能换代任务启动身份");
        assert_eq!(
            work.published_sources_revision, self.published_sources_revision,
            "{label}"
        );
        assert_eq!(
            work.handles, self.handles,
            "{label}不能改变权威句柄反向索引"
        );
        assert!(
            work.task.is_none() && work.releasing.is_none(),
            "{label}不应占用后台席位"
        );
        assert!(
            work.queued.is_empty() && work.updates.is_empty() && work.pending.is_empty(),
            "{label}不能登记待捕获来源"
        );
        assert!(!work.scanning && !work.pruning, "{label}不能开启目录重扫");
        assert_eq!(work.sources[map].revision, self.source_revision, "{label}");
        assert!(
            Arc::ptr_eq(&work.sources[map], &self.source),
            "{label}必须复用原来源快照"
        );
    }
}

#[test]
fn derived_manifest_asset_lifecycle_does_not_restart_the_published_graph() {
    let mut fixture = Fixture::new();
    fixture.settle(MapInstanceGraphPhase::Ready);
    let stable = StableGraph::capture(&fixture);
    let mut projection = fixture.manifest();
    projection.display_name = "运行实例私有投影".into();
    projection.world_min = [-2.; 2];
    projection.world_max = [2.; 2];
    let derived = fixture
        .app
        .world_mut()
        .resource_mut::<Assets<MapManifestAsset>>()
        .add(projection);
    let derived_id = derived.id();
    assert_ne!(derived_id, fixture.registered.id());
    fixture.notify_without_rebuild(&stable, AssetEvent::Added { id: derived_id }, "投影Added");
    fixture
        .app
        .world_mut()
        .resource_mut::<Assets<MapManifestAsset>>()
        .get_mut(&derived)
        .unwrap()
        .display_name = "已修改的运行投影".into();
    fixture.notify_without_rebuild(
        &stable,
        AssetEvent::Modified { id: derived_id },
        "投影Modified",
    );
    fixture.notify_without_rebuild(
        &stable,
        AssetEvent::LoadedWithDependencies { id: derived_id },
        "投影LoadedWithDependencies",
    );
    drop(derived); // Unused到达时资产仍可查询；不能借同MapId回退为目录权威源。
    fixture.notify_without_rebuild(&stable, AssetEvent::Unused { id: derived_id }, "投影Unused");
    assert!(
        fixture
            .app
            .world_mut()
            .resource_mut::<Assets<MapManifestAsset>>()
            .remove(derived_id)
            .is_some()
    );
    fixture.notify_without_rebuild(
        &stable,
        AssetEvent::Removed { id: derived_id },
        "投影Removed",
    );
}

#[test]
fn registered_manifest_modified_removed_and_added_still_update_and_wait_atomically() {
    let mut fixture = Fixture::new();
    fixture.settle(MapInstanceGraphPhase::Ready);
    let initial = StableGraph::capture(&fixture);
    fixture
        .app
        .world_mut()
        .resource_mut::<Assets<MapManifestAsset>>()
        .get_mut(&fixture.registered)
        .unwrap()
        .world_max[0] = 16.;
    fixture.app.world_mut().write_message(AssetEvent::Modified {
        id: fixture.registered.id(),
    });
    fixture.settle(MapInstanceGraphPhase::Ready);
    let changed = StableGraph::capture(&fixture);
    assert!(changed.graph_revision > initial.graph_revision);
    assert!(changed.structural_revision > initial.structural_revision);
    assert!(changed.source_revision > initial.source_revision);
    assert_eq!(changed.source.world_bounds.max.x, 16.);
    assert_eq!(changed.instances[0].local_bounds.max.x, 16.);

    let removed = fixture
        .app
        .world_mut()
        .resource_mut::<Assets<MapManifestAsset>>()
        .remove(fixture.registered.id())
        .unwrap();
    fixture.app.world_mut().write_message(AssetEvent::Removed {
        id: fixture.registered.id(),
    });
    fixture.settle(MapInstanceGraphPhase::Waiting);
    let graph = fixture.app.world().resource::<MapInstanceGraph>();
    assert_eq!(graph.revision, changed.graph_revision);
    assert_eq!(graph.structural_revision, changed.structural_revision);
    assert!(
        Arc::ptr_eq(&graph.instances, &changed.instances),
        "正式来源暂时缺席仍保留上一完整树"
    );
    let work = fixture.app.world().resource::<MapInstanceGraphWork>();
    assert!(work.pending.contains(&fixture.map));
    assert!(!work.sources.contains_key(&fixture.map));
    fixture
        .app
        .world_mut()
        .resource_mut::<Assets<MapManifestAsset>>()
        .insert(fixture.registered.id(), removed)
        .unwrap();
    fixture.app.world_mut().write_message(AssetEvent::Added {
        id: fixture.registered.id(),
    });
    fixture.settle(MapInstanceGraphPhase::Ready);
    let restored = StableGraph::capture(&fixture);
    assert!(restored.graph_revision > changed.graph_revision);
    assert_eq!(restored.instances[0].local_bounds.max.x, 16.);
}

#[test]
fn replaced_registered_handle_cannot_be_invalidated_by_its_stale_reverse_index_events() {
    let mut fixture = Fixture::new();
    fixture.settle(MapInstanceGraphPhase::Ready);
    let old = fixture.registered.clone();
    let old_id = old.id();
    let mut replacement = fixture.manifest();
    replacement.world_max[0] = 24.;
    let registered = fixture
        .app
        .world_mut()
        .resource_mut::<Assets<MapManifestAsset>>()
        .add(replacement);
    {
        let mut registry = fixture.app.world_mut().resource_mut::<MapContentRegistry>();
        registry.maps.get_mut(&fixture.map).unwrap().manifest = registered.clone();
        registry.revision += 1;
    }
    fixture.registered = registered;
    fixture.settle(MapInstanceGraphPhase::Ready);
    let stable = StableGraph::capture(&fixture);
    assert_eq!(stable.source.world_bounds.max.x, 24.);
    assert_eq!(stable.handles.get(&old_id), Some(&fixture.map)); // 真实旧反向索引仍在，必须复核当前登记句柄。
    assert_eq!(
        stable.handles.get(&fixture.registered.id()),
        Some(&fixture.map)
    );
    fixture
        .app
        .world_mut()
        .resource_mut::<Assets<MapManifestAsset>>()
        .get_mut(&old)
        .unwrap()
        .world_max[0] = 4.;
    fixture.notify_without_rebuild(
        &stable,
        AssetEvent::Modified { id: old_id },
        "旧登记句柄Modified",
    );
    drop(old);
    fixture.notify_without_rebuild(
        &stable,
        AssetEvent::Unused { id: old_id },
        "旧登记句柄Unused",
    );
    fixture
        .app
        .world_mut()
        .resource_mut::<Assets<MapManifestAsset>>()
        .remove(old_id)
        .unwrap();
    fixture.notify_without_rebuild(
        &stable,
        AssetEvent::Removed { id: old_id },
        "旧登记句柄Removed",
    );
}

#[test]
fn first_registered_added_publishes_before_the_reverse_handle_index_exists() {
    let mut fixture = Fixture::new();
    {
        let mut work = fixture
            .app
            .world_mut()
            .resource_mut::<MapInstanceGraphWork>();
        // 独立验证Added回退查询：不能由目录扫描顺便捕获来源而掩盖首次通知丢失。
        work.catalog_revision = Some(1);
        assert!(work.handles.is_empty() && work.sources.is_empty());
        assert!(!work.scanning && !work.pruning && work.task.is_none());
    }
    fixture.app.world_mut().write_message(AssetEvent::Added {
        id: fixture.registered.id(),
    });
    fixture.settle(MapInstanceGraphPhase::Ready);
    let graph = fixture.app.world().resource::<MapInstanceGraph>();
    assert_eq!(graph.root_map_id.as_ref(), Some(&fixture.map));
    assert_eq!(graph.instances.len(), 1);
    assert_eq!(graph.instances[0].source_map_id, fixture.map);
    let work = fixture.app.world().resource::<MapInstanceGraphWork>();
    assert_eq!(
        work.handles.get(&fixture.registered.id()),
        Some(&fixture.map)
    );
    assert!(work.sources.contains_key(&fixture.map));
    assert_eq!(work.started, work.desired);
}
