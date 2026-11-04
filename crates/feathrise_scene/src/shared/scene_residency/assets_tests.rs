//! 资产通知消费确认、同帧切图恢复与已释放依赖的固定预算退役回归。

use super::*;
use feathrise_scene_core::residency::{ResidentAssetEventReader, SceneResidencyRuntimeSet};

#[derive(Resource, Default)]
struct TestRuntime {
    handle: Option<Handle<Mesh>>,
    received: usize,
    freeze_before_consumption: bool,
    watch_all: bool,
}

fn fixture() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::asset::AssetPlugin::default(),
        feathrise_scene_core::residency::SceneResidencyResourcePlugin,
    ))
    .init_asset::<Mesh>()
    .init_resource::<ResidentScenes>()
    .init_resource::<AssetOwners>()
    .init_resource::<TestRuntime>()
    .insert_resource(SceneResidencyActivity {
        enabled: true,
        active: Some(ResidentMapToken(2)),
        ..default()
    });
    watch::<Mesh>(&mut app);
    app.add_systems(
        Update,
        (|runtime: Res<TestRuntime>, mut activity: ResMut<SceneResidencyActivity>| {
            if runtime.freeze_before_consumption {
                activity.switching = true;
            }
        })
        .before(SceneResidencyRuntimeSet),
    )
    .add_systems(
        Update,
        (|mut events: ResidentAssetEventReader<Mesh>, mut runtime: ResMut<TestRuntime>| {
            for event in events.read() {
                if (runtime.watch_all && matches!(event, AssetEvent::Modified { .. }))
                    || runtime
                        .handle
                        .as_ref()
                        .is_some_and(|handle| event.is_modified(handle.id()))
                {
                    runtime.received += 1;
                }
            }
        })
        .in_set(SceneResidencyRuntimeSet),
    );
    app
}

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

fn activate(app: &mut App, token: ResidentMapToken, handle: Option<Handle<Mesh>>, freeze: bool) {
    {
        let mut activity = app.world_mut().resource_mut::<SceneResidencyActivity>();
        activity.active = Some(token);
        activity.switching = false;
    }
    {
        let mut runtime = app.world_mut().resource_mut::<TestRuntime>();
        runtime.handle = handle;
        runtime.freeze_before_consumption = freeze;
    }
    app.world_mut().trigger(ResidentSceneActivated(token));
}

#[test]
fn replay_survives_switch_starting_before_runtime_consumption() {
    let mut app = fixture();
    let (a, b) = (ResidentMapToken(1), ResidentMapToken(2));
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Mesh::from(Cuboid::new(1.0, 1.0, 1.0)));
    let entity = app
        .world_mut()
        .spawn((ResidentMapEntity(a), Mesh3d(mesh.clone())))
        .id();
    register_owned_assets(app.world_mut(), entity);
    settle(&mut app); // 初始Added结清，仅观察后续Modified。
    for interrupted in [false, true] {
        activate(&mut app, b, None, false);
        app.world_mut().resource_mut::<TestRuntime>().received = 0;
        app.world_mut()
            .resource_mut::<Messages<AssetEvent<Mesh>>>()
            .write(AssetEvent::Modified { id: mesh.id() });
        settle(&mut app);
        activate(&mut app, a, Some(mesh.clone()), interrupted);
        app.update(); // PreUpdate回放，Update中的标签请求可以先于地图消费者冻结运行时。
        if interrupted {
            assert_eq!(app.world().resource::<TestRuntime>().received, 0);
            activate(&mut app, b, None, false);
            settle(&mut app); // 独立消息已经过期，恢复必须从未确认的通知重新回放。
            activate(&mut app, a, Some(mesh.clone()), false);
        }
        settle(&mut app);
        assert_eq!(app.world().resource::<TestRuntime>().received, 1);
        activate(&mut app, b, None, false);
        settle(&mut app);
        activate(&mut app, a, Some(mesh.clone()), false);
        settle(&mut app);
        assert_eq!(app.world().resource::<TestRuntime>().received, 1); // 已确认后再次返回不重复热更新。
    }
}

#[test]
fn native_change_survives_switch_starting_in_the_arrival_frame() {
    let mut app = fixture();
    let (a, b) = (ResidentMapToken(1), ResidentMapToken(2));
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Mesh::from(Cuboid::new(1.0, 1.0, 1.0)));
    let entity = app
        .world_mut()
        .spawn((ResidentMapEntity(a), Mesh3d(mesh.clone())))
        .id();
    register_owned_assets(app.world_mut(), entity);
    activate(&mut app, a, Some(mesh.clone()), false);
    settle(&mut app);
    app.world_mut().resource_mut::<TestRuntime>().received = 0;
    app.world_mut()
        .resource_mut::<TestRuntime>()
        .freeze_before_consumption = true;
    app.world_mut()
        .resource_mut::<Messages<AssetEvent<Mesh>>>()
        .write(AssetEvent::Modified { id: mesh.id() });
    app.update();
    assert_eq!(app.world().resource::<TestRuntime>().received, 0);
    activate(&mut app, b, None, false);
    settle(&mut app);
    activate(&mut app, a, Some(mesh), false);
    settle(&mut app);
    assert_eq!(app.world().resource::<TestRuntime>().received, 1);
}

#[test]
fn shared_replay_survives_switch_without_marking_the_version_consumed() {
    let mut app = fixture();
    let (a, b) = (ResidentMapToken(1), ResidentMapToken(2));
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Mesh::from(Cuboid::new(1.0, 1.0, 1.0)));
    settle(&mut app); // 共享资源只有强句柄，不挂在任何地图实体上。
    app.world_mut()
        .resource_mut::<Messages<AssetEvent<Mesh>>>()
        .write(AssetEvent::Modified { id: mesh.id() });
    settle(&mut app);
    activate(&mut app, a, Some(mesh.clone()), true);
    app.update();
    assert_eq!(app.world().resource::<TestRuntime>().received, 0);
    activate(&mut app, b, None, false);
    settle(&mut app);
    activate(&mut app, a, Some(mesh.clone()), false);
    settle(&mut app);
    assert_eq!(app.world().resource::<TestRuntime>().received, 1);
    activate(&mut app, b, None, false);
    settle(&mut app);
    activate(&mut app, a, Some(mesh), false);
    settle(&mut app);
    assert_eq!(app.world().resource::<TestRuntime>().received, 1);
}

#[test]
fn interrupted_replay_preserves_the_entire_batch_and_resumes_with_the_original_budget() {
    let mut app = fixture();
    let (a, b) = (ResidentMapToken(1), ResidentMapToken(2));
    let mut handles = Vec::new();
    for _ in 0..EVENT_BUDGET + 1 {
        let handle = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(Mesh::from(Cuboid::new(1.0, 1.0, 1.0)));
        let entity = app
            .world_mut()
            .spawn((ResidentMapEntity(a), Mesh3d(handle.clone())))
            .id();
        register_owned_assets(app.world_mut(), entity);
        handles.push(handle);
    }
    settle(&mut app);
    activate(&mut app, a, None, false);
    settle(&mut app); // 初始Added全部确认，再单独检验129条真实热更新。
    activate(&mut app, b, None, false);
    for handle in &handles {
        app.world_mut()
            .resource_mut::<Messages<AssetEvent<Mesh>>>()
            .write(AssetEvent::Modified { id: handle.id() });
    }
    settle(&mut app);
    activate(&mut app, a, None, true);
    app.world_mut().resource_mut::<TestRuntime>().watch_all = true;
    app.update();
    assert_eq!(app.world().resource::<TestRuntime>().received, 0);
    assert_eq!(
        app.world().resource::<AssetChanges<Mesh>>().maps[&a]
            .pending
            .len(),
        handles.len()
    );
    app.world_mut().resource_mut::<TestRuntime>().watch_all = false;
    activate(&mut app, b, None, false);
    settle(&mut app);
    activate(&mut app, a, None, false);
    app.world_mut().resource_mut::<TestRuntime>().watch_all = true;
    app.update();
    assert_eq!(app.world().resource::<TestRuntime>().received, EVENT_BUDGET);
    app.update();
    assert_eq!(
        app.world().resource::<TestRuntime>().received,
        handles.len()
    );
    settle(&mut app);
    assert_eq!(
        app.world().resource::<TestRuntime>().received,
        handles.len()
    );
    assert!(
        app.world().resource::<AssetChanges<Mesh>>().maps[&a]
            .pending
            .is_empty()
    );
}

#[test]
fn unused_meshes_do_not_accumulate_in_map_dependency_index() {
    let mut app = fixture();
    let token = ResidentMapToken(1);
    for _ in 0..16 {
        let handle = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(Mesh::from(Cuboid::new(1.0, 1.0, 1.0)));
        let id = handle.id();
        let entity = app
            .world_mut()
            .spawn((ResidentMapEntity(token), Mesh3d(handle)))
            .id();
        register_owned_assets(app.world_mut(), entity);
        app.world_mut().resource_mut::<AssetOwners>().remove(entity);
        app.world_mut().despawn(entity);
        app.world_mut()
            .resource_mut::<Messages<AssetEvent<Mesh>>>()
            .write(AssetEvent::Unused { id });
        app.update();
    }
    assert!(app.world().resource::<AssetOwners>().entities.is_empty());
    let changes = app.world().resource::<AssetChanges<Mesh>>();
    let map = &changes.maps[&token];
    assert!(map.known.is_empty() && map.indexed.is_empty() && map.pending.is_empty());
    assert!(changes.dependents.is_empty());
}

#[test]
fn unused_shared_dependency_retires_with_a_fixed_visit_budget() {
    let mut app = fixture();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Mesh::from(Cuboid::new(1.0, 1.0, 1.0)));
    let id = handle.id();
    let count = EVENT_BUDGET + 32;
    for token in 10..10 + count as u64 {
        let entity = app
            .world_mut()
            .spawn((
                ResidentMapEntity(ResidentMapToken(token)),
                Mesh3d(handle.clone()),
            ))
            .id();
        register_owned_assets(app.world_mut(), entity);
        app.world_mut().resource_mut::<AssetOwners>().remove(entity);
        app.world_mut().despawn(entity);
    }
    drop(handle);
    app.world_mut()
        .resource_mut::<Messages<AssetEvent<Mesh>>>()
        .write(AssetEvent::Unused { id });
    app.update();
    let remaining = app
        .world()
        .resource::<AssetChanges<Mesh>>()
        .maps
        .values()
        .filter(|map| !map.known.is_empty())
        .count();
    assert_eq!(remaining, count - EVENT_BUDGET);
    settle(&mut app);
    let changes = app.world().resource::<AssetChanges<Mesh>>();
    assert!(
        changes
            .maps
            .values()
            .all(|map| map.known.is_empty() && map.pending.is_empty())
    );
    assert!(changes.unused.is_empty() && changes.dependents.is_empty());
}

#[test]
fn closing_map_retires_reverse_index_without_removing_live_shared_dependencies() {
    let mut app = fixture();
    let (closed, live) = (ResidentMapToken(1), ResidentMapToken(2));
    let mut handles = Vec::new();
    for _ in 0..EVENT_BUDGET * 2 + 1 {
        let handle = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(Mesh::from(Cuboid::new(1.0, 1.0, 1.0)));
        let mut changes = app.world_mut().resource_mut::<AssetChanges<Mesh>>();
        remember(&mut changes, closed, handle.id());
        remember(&mut changes, live, handle.id());
        handles.push(handle);
    }
    let mut scenes = ResidentScenes::default();
    forget_type::<Mesh>(app.world_mut(), closed, &mut scenes);
    settle(&mut app);
    let changes = app.world().resource::<AssetChanges<Mesh>>();
    assert!(!changes.maps.contains_key(&closed));
    assert!(changes.retired.is_empty());
    assert_eq!(changes.maps[&live].known.len(), handles.len());
    assert!(
        handles
            .iter()
            .all(|handle| changes.dependents[&handle.id()] == HashSet::from([live]))
    );
}
