//! 真实动态场景监听器的清单身份与通知消费回归；内存资产覆盖同图投影和独立实例域。

use super::{
    DynamicSceneRuntimePhase, DynamicSceneRuntimeState, DynamicSceneRuntimeStatus,
    observe_dynamic_scene_assets,
};
use bevy::{
    asset::{AssetId, AssetLoadError, AssetLoadFailedEvent},
    prelude::*,
};
use feathrise_scene_content::{
    ActiveDynamicScene, SCENE_SCHEMA_VERSION, SceneContentHandles, SceneContentRegistry, SceneId,
    SceneManifestAsset, SceneTerrainDescriptor,
};
use feathrise_scene_core::residency::{
    InstanceRes, InstanceResourceTable, SceneRuntimeScope, ResidentAssetEvent,
    install_instance_asset_journal,
};
use std::{collections::BTreeMap, sync::Arc};

const INITIAL_ENVIRONMENT_REVISION: u64 = 11;

struct Fixture {
    app: App,
    map: SceneId,
    registered: Handle<SceneManifestAsset>,
    projection: Handle<SceneManifestAsset>,
    other: Handle<SceneManifestAsset>,
}

fn manifest(map: SceneId, label: &str) -> SceneManifestAsset {
    SceneManifestAsset {
        vegetation_paint_groups: Default::default(),
        schema_version: SCENE_SCHEMA_VERSION,
        id: map,
        display_name: label.into(),
        notes: String::new(),
        runtime_scene: "data_driven".into(),
        world_min: [-8.; 2],
        world_max: [8.; 2],
        default_spawn: [0.; 3],
        environment: Default::default(),
        terrain: SceneTerrainDescriptor::empty(),
        terrains: Some(Arc::from([])),
        subscenes: Default::default(),
        puddles: None,
        asset_roots: Vec::new(),
    }
}

fn handles(manifest: Handle<SceneManifestAsset>) -> SceneContentHandles {
    SceneContentHandles {
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

fn failure(id: AssetId<SceneManifestAsset>, map: &SceneId) -> AssetLoadFailedEvent<SceneManifestAsset> {
    AssetLoadFailedEvent {
        id,
        path: format!("scenes/{map}/scene.scene_manifest.json").into(),
        error: AssetLoadError::AssetMetaReadError,
    }
}

impl Fixture {
    fn new() -> Self {
        let map = SceneId::parse("MAP-observer-current").unwrap();
        let other_scene = SceneId::parse("MAP-observer-other").unwrap();
        let mut app = App::new();
        // 不安装资产IO或窗口插件，只把正式消息通道送入生产系统；测试不会接触用户场景文件。
        app.init_resource::<Assets<SceneManifestAsset>>()
            .add_message::<AssetEvent<SceneManifestAsset>>()
            .add_message::<ResidentAssetEvent<SceneManifestAsset>>()
            .add_message::<AssetLoadFailedEvent<SceneManifestAsset>>()
            .add_message::<AssetLoadFailedEvent<WorldAsset>>()
            .add_systems(Update, observe_dynamic_scene_assets);
        let (registered, projection, other) = {
            let mut assets = app.world_mut().resource_mut::<Assets<SceneManifestAsset>>();
            (
                assets.add(manifest(map.clone(), "正式清单")),
                assets.add(manifest(map.clone(), "同图私有投影")),
                assets.add(manifest(other_scene.clone(), "其他场景")),
            )
        };
        app.insert_resource(ActiveDynamicScene {
            scene_id: Some(map.clone()),
        })
        .insert_resource(SceneContentRegistry {
            catalog: Handle::default(),
            revision: 3,
            scenes: BTreeMap::from([
                (map.clone(), handles(registered.clone())),
                (other_scene, handles(other.clone())),
            ]),
            last_error: None,
        })
        .insert_resource(DynamicSceneRuntimeState {
            dirty: false,
            catalog_revision: 3,
            environment_revision: INITIAL_ENVIRONMENT_REVISION,
            ..default()
        })
        .insert_resource(DynamicSceneRuntimeStatus {
            phase: DynamicSceneRuntimePhase::Ready,
            scene_id: Some(map.clone()),
            detail: "原完整场景".into(),
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
        let state = self.app.world().resource::<DynamicSceneRuntimeState>();
        assert!(!state.dirty, "无关清单不能重启当前地形准备");
        assert_eq!(state.environment_revision, INITIAL_ENVIRONMENT_REVISION);
        let status = self.app.world().resource::<DynamicSceneRuntimeStatus>();
        assert_eq!(status.phase, DynamicSceneRuntimePhase::Ready);
        assert_eq!(status.detail, "原完整场景");
    }
}

#[test]
fn unrelated_manifest_lifecycle_and_failures_leave_the_current_scene_untouched() {
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
        // 故意共用当前场景路径：失败只能依据AssetId归属，不能凭文件名误认同图投影。
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
    let state = fixture.app.world().resource::<DynamicSceneRuntimeState>();
    assert!(state.dirty);
    assert_eq!(state.environment_revision, INITIAL_ENVIRONMENT_REVISION + 1);
    fixture
        .app
        .world_mut()
        .resource_mut::<DynamicSceneRuntimeState>()
        .dirty = false;
    // 模拟地形系统处理完成；同一轮剩余消息不能在后续帧再次制造环境修订。
    for _ in 0..4 {
        fixture.app.update();
        let state = fixture.app.world().resource::<DynamicSceneRuntimeState>();
        assert!(!state.dirty, "已消费的混合通知不能连续重刷");
        assert_eq!(state.environment_revision, INITIAL_ENVIRONMENT_REVISION + 1);
    }
}

#[derive(Resource, Default)]
struct RuntimeManifest(Option<Handle<SceneManifestAsset>>);

fn capture_runtime_manifest(
    active: InstanceRes<ActiveDynamicScene>,
    catalog: crate::scenes::dynamic_scene::sources::SceneRuntimeCatalog,
    mut probe: ResMut<RuntimeManifest>,
) {
    probe.0 = active
        .scene_id
        .as_ref()
        .and_then(|map| catalog.handles(map))
        .map(|handles| handles.manifest.clone());
}

#[test]
fn the_parent_container_accepts_its_actual_runtime_catalog_projection() {
    let mut fixture = Fixture::new();
    fixture
        .app
        .init_resource::<feathrise_scene_content::SceneInstanceAuthorPreview>()
        .init_resource::<RuntimeManifest>()
        .add_systems(Update, capture_runtime_manifest);
    crate::scenes::dynamic_scene::sources::install(&mut fixture.app);
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
    let state = fixture.app.world().resource::<DynamicSceneRuntimeState>();
    assert!(state.dirty, "容器实际投影的Modified仍须使当前环境换代");
    assert_eq!(state.environment_revision, INITIAL_ENVIRONMENT_REVISION + 1);
    fixture
        .app
        .world_mut()
        .resource_mut::<DynamicSceneRuntimeState>()
        .dirty = false;
    fixture.app.update();
    let state = fixture.app.world().resource::<DynamicSceneRuntimeState>();
    assert!(!state.dirty);
    assert_eq!(state.environment_revision, INITIAL_ENVIRONMENT_REVISION + 1);
}

#[test]
fn an_instance_uses_its_registered_projection_and_drains_the_asset_journal() {
    let mut fixture = Fixture::new();
    let child_scene = SceneId::parse("MAP-observer-other").unwrap();
    let child = fixture.app.world_mut().spawn_empty().id();
    let projection = fixture
        .app
        .world_mut()
        .resource_mut::<Assets<SceneManifestAsset>>()
        .add(manifest(child_scene.clone(), "子实例合法投影"));
    // 父图选择与子实例选择不同，且子实例登记合法投影，证明监听器同时遵循两个域参数。
    fixture
        .app
        .insert_resource(SceneRuntimeScope {
            instance: Some(child),
            ..default()
        })
        .insert_resource(InstanceResourceTable::<ActiveDynamicScene> {
            values: BTreeMap::from([(
                child,
                ActiveDynamicScene {
                    scene_id: Some(child_scene.clone()),
                },
            )]),
        })
        .insert_resource(InstanceResourceTable::<SceneContentRegistry> {
            values: BTreeMap::from([(
                child,
                SceneContentRegistry {
                    catalog: Handle::default(),
                    revision: 5,
                    scenes: BTreeMap::from([(child_scene.clone(), handles(projection.clone()))]),
                    last_error: None,
                },
            )]),
        })
        .insert_resource(InstanceResourceTable::<DynamicSceneRuntimeState> {
            values: BTreeMap::from([(
                child,
                DynamicSceneRuntimeState {
                    dirty: false,
                    catalog_revision: 5,
                    environment_revision: 23,
                    ..default()
                },
            )]),
        })
        .insert_resource(InstanceResourceTable::<DynamicSceneRuntimeStatus> {
            values: BTreeMap::from([(
                child,
                DynamicSceneRuntimeStatus {
                    phase: DynamicSceneRuntimePhase::Ready,
                    scene_id: Some(child_scene.clone()),
                    ..default()
                },
            )]),
        });
    install_instance_asset_journal::<SceneManifestAsset>(&mut fixture.app);
    for id in [fixture.registered.id(), fixture.other.id()] {
        fixture
            .app
            .world_mut()
            .write_message(AssetEvent::Modified { id });
        fixture
            .app
            .world_mut()
            .write_message(failure(id, &child_scene));
    }
    fixture.app.update();
    let states = fixture
        .app
        .world()
        .resource::<InstanceResourceTable<DynamicSceneRuntimeState>>();
    assert!(!states.values[&child].dirty);
    assert_eq!(states.values[&child].environment_revision, 23);
    assert_eq!(
        fixture
            .app
            .world()
            .resource::<InstanceResourceTable<DynamicSceneRuntimeStatus>>()
            .values[&child]
            .phase,
        DynamicSceneRuntimePhase::Ready
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
        .resource::<InstanceResourceTable<DynamicSceneRuntimeState>>();
    assert!(
        states.values[&child].dirty,
        "域登记的合法投影Modified必须生效"
    );
    assert_eq!(states.values[&child].environment_revision, 24);
    fixture
        .app
        .world_mut()
        .resource_mut::<InstanceResourceTable<DynamicSceneRuntimeState>>()
        .values
        .get_mut(&child)
        .unwrap()
        .dirty = false;
    for _ in 0..4 {
        fixture.app.update();
        let states = fixture
            .app
            .world()
            .resource::<InstanceResourceTable<DynamicSceneRuntimeState>>();
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
    let status = fixture.app.world().resource::<DynamicSceneRuntimeStatus>();
    assert_eq!(status.phase, DynamicSceneRuntimePhase::Failed);
    assert!(status.detail.contains("动态场景清单加载失败"));
    assert!(status.detail.contains(&fixture.map.to_string()));
}

#[test]
fn catalog_revision_changes_still_invalidate_the_environment() {
    let mut fixture = Fixture::new();
    fixture
        .app
        .world_mut()
        .resource_mut::<SceneContentRegistry>()
        .revision += 1;
    fixture.app.update();
    let state = fixture.app.world().resource::<DynamicSceneRuntimeState>();
    assert!(state.dirty);
    assert_eq!(state.environment_revision, INITIAL_ENVIRONMENT_REVISION + 1);
}
