//! 自有内存地图驱动真实姿态消费者；覆盖按住、取消、正式来源接回及迟到拒绝。
use super::*;
#[path = "object_pose_loading_tests.rs"]
mod loading;
use feathrise_map_content::{MAP_SCHEMA_VERSION, MapContentHandles, MapTerrainDescriptor};
use feathrise_scene::runtime_test_support::{
    apply_terrain_pose_preview, install_active_terrain_instances, install_active_sub_map_instance,
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

struct Fixture {
    app: App,
    map: MapId,
    manifest: Handle<MapManifestAsset>,
    terrain: TerrainId,
    root: Entity,
}
fn fixture() -> Fixture {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<Assets<MapManifestAsset>>()
        .init_resource::<MapInstanceAuthorPreview>()
        .init_resource::<TerrainObjectTransformPreview>();
    let map = MapId::parse("MAP-object-pose").unwrap();
    let terrain = TerrainId::parse("ground").unwrap();
    let object = MapTerrainObject {
        id: terrain.clone(),
        display_name: "真实地形根".into(),
        enabled: true,
        transform: TerrainObjectTransform::default(),
        world_min: [-4.; 2],
        world_max: [4.; 2],
        terrain: MapTerrainDescriptor::empty(),
        voxel: None,
        surface_path: None,
        vegetation_path: None,
        water_path: None,
        puddles: None,
        clipmap_stitches: Arc::from([]),
        mesh_lod: None,
    };
    let root = install_active_terrain_instances(app.world_mut(), &map, &[object.clone()])[0];
    let manifest = app
        .world_mut()
        .resource_mut::<Assets<MapManifestAsset>>()
        .add(MapManifestAsset {
            schema_version: MAP_SCHEMA_VERSION,
            id: map.clone(),
            display_name: "自有整体姿态回归".into(),
            notes: String::new(),
            runtime_scene: "data_driven".into(),
            world_min: [-8.; 2],
            world_max: [8.; 2],
            default_spawn: [0.; 3],
            environment: default(),
            terrain: MapTerrainDescriptor::empty(),
            terrains: Some(Arc::from([object])),
            sub_maps: Arc::from([MapInstancePlacement {
                id: MapInstanceId::parse("child").unwrap(),
                source_map_id: MapId::parse("MAP-child").unwrap(),
                label: "原子图".into(),
                transform: default(),
            }]),
            puddles: None,
            asset_roots: Vec::new(),
            vegetation_paint_groups: default(),
        });
    let child = app.world().resource::<Assets<MapManifestAsset>>().get(&manifest).unwrap().sub_maps[0].clone();
    install_active_sub_map_instance(app.world_mut(), &map, &child);
    app.insert_resource(ActiveDynamicMap {
        map_id: Some(map.clone()),
    })
    .insert_resource(MapContentRegistry {
        catalog: default(),
        revision: 1,
        last_error: None,
        maps: BTreeMap::from([(
            map.clone(),
            MapContentHandles {
                manifest: manifest.clone(),
                portals: default(),
                placements: default(),
                characters_path: None,
                surface: None,
                vegetation: None,
                water: None,
                boundaries: None,
            },
        )]),
    });
    Fixture {
        app,
        map,
        manifest,
        terrain,
        root,
    }
}
fn send(
    f: &mut Fixture,
    state: &mut State,
    pose: TerrainObjectTransform,
    active: bool,
    revision: u64,
) {
    state
        .start(
            f.app.world(),
            Prepared {
                map: f.map.clone(),
                revision,
                active,
                target: Target::Terrain {
                    id: f.terrain.clone(),
                    pose: Some(pose),
                },
            },
        )
        .unwrap();
    settle(f, state);
}
fn settle(f: &mut Fixture, state: &mut State) {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let done = state.advance(f.app.world_mut()).unwrap();
        apply_terrain_pose_preview(f.app.world_mut());
        if done {
            break;
        }
        assert!(Instant::now() < until, "整体姿态后台槽应在有限时间收口");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn object_pose_real_terrain_consumer_moves_rotates_scales_and_cancel_restores_same_root() {
    let mut f = fixture();
    let mut state = State::default();
    let base = *f.app.world().get::<Transform>(f.root).unwrap();
    let pose = TerrainObjectTransform {
        translation: [8., 2., -3.],
        yaw_degrees: 29.,
        pitch_degrees: 12.,
        roll_degrees: -4.,
        scale: [2., 1., 3.],
    };
    send(&mut f, &mut state, pose, true, 1);
    assert!(
        f.app
            .world()
            .get::<Transform>(f.root)
            .unwrap()
            .to_matrix()
            .abs_diff_eq(pose.as_transform().to_matrix(), 0.00001)
    );
    assert!(
        f.app
            .world()
            .resource::<MapInstanceAuthorPreview>()
            .map_id
            .is_none(),
        "按住不重发整份地形定义"
    );
    send(
        &mut f,
        &mut state,
        TerrainObjectTransform::default(),
        false,
        2,
    );
    assert_eq!(*f.app.world().get::<Transform>(f.root).unwrap(), base);
    assert_eq!(
        f.app
            .world()
            .get::<TerrainObjectInstance>(f.root)
            .unwrap()
            .definition
            .transform,
        TerrainObjectTransform::default()
    );
    assert!(
        f.app
            .world()
            .resource::<Assets<MapManifestAsset>>()
            .get(&f.manifest)
            .unwrap()
            .terrains
            .as_ref()
            .unwrap()[0]
            .transform
            == TerrainObjectTransform::default(),
        "预览不写正式资产"
    );
}

#[test]
fn object_pose_end_keeps_real_pose_until_formal_definition_adopts_without_old_pose_flash() {
    let mut f = fixture();
    let mut state = State::default();
    let pose = TerrainObjectTransform {
        translation: [9., 0., 0.],
        ..default()
    };
    send(&mut f, &mut state, pose, true, 1);
    send(&mut f, &mut state, pose, false, 2);
    state.snapshot_started().unwrap();
    assert_eq!(
        f.app
            .world()
            .get::<Transform>(f.root)
            .unwrap()
            .translation
            .x,
        9.
    );
    let mut next = f
        .app
        .world()
        .get::<TerrainObjectInstance>(f.root)
        .unwrap()
        .definition
        .as_ref()
        .clone();
    next.transform = pose;
    f.app
        .world_mut()
        .resource_mut::<Assets<MapManifestAsset>>()
        .get_mut(&f.manifest)
        .unwrap()
        .terrains = Some(Arc::from([next.clone()]));
    state.advance(f.app.world_mut()).unwrap();
    apply_terrain_pose_preview(f.app.world_mut());
    assert_eq!(
        f.app
            .world()
            .get::<Transform>(f.root)
            .unwrap()
            .translation
            .x,
        9.,
        "清单到达但真实定义未采用时保留可见姿态"
    );
    // 模拟原实例图唯一的定义采用边沿；实体姿态仍由原消费者写入，不由验收直接设置。
    f.app
        .world_mut()
        .get_mut::<TerrainObjectInstance>(f.root)
        .unwrap()
        .definition = Arc::new(next);
    settle(&mut f, &mut state);
    assert!(
        f.app
            .world()
            .resource::<TerrainObjectTransformPreview>()
            .pose
            .is_none()
    );
    assert_eq!(
        f.app
            .world()
            .get::<Transform>(f.root)
            .unwrap()
            .translation
            .x,
        9.
    );
}

#[test]
fn object_pose_submap_updates_only_stable_reference_and_cancel_keeps_original_source() {
    let mut f = fixture();
    let mut state = State::default();
    let id = MapInstanceId::parse("child").unwrap();
    for (revision, x, active) in [(1, 8., true), (2, 9., true), (3, 0., false)] {
        state
            .start(
                f.app.world(),
                Prepared {
                    map: f.map.clone(),
                    revision,
                    active,
                    target: Target::SubMap {
                        id: id.clone(),
                        pose: Some(MapInstanceTransform {
                            translation: [x, 0., 0.],
                            yaw_degrees: 30.,
                            ..default()
                        }),
                    },
                },
            )
            .unwrap();
        settle(&mut f, &mut state);
        let preview = f.app.world().resource::<MapInstanceAuthorPreview>();
        assert_eq!(preview.instances[0].transform.translation[0], x);
        assert_eq!(preview.instances[0].id, id);
        assert_eq!(
            preview.instances[0].source_map_id,
            MapId::parse("MAP-child").unwrap()
        );
        assert_eq!(
            f.app
                .world()
                .resource::<Assets<MapManifestAsset>>()
                .get(&f.manifest)
                .unwrap()
                .sub_maps[0]
                .transform
                .translation[0],
            0.
        );
    }
}

#[test]
fn object_pose_rejects_late_source_and_does_not_clear_another_publisher() {
    let mut f = fixture();
    let mut state = State::default();
    state
        .start(
            f.app.world(),
            Prepared {
                map: f.map.clone(),
                revision: 1,
                active: true,
                target: Target::SubMap {
                    id: MapInstanceId::parse("child").unwrap(),
                    pose: Some(MapInstanceTransform {
                        translation: [8., 0., 0.],
                        ..default()
                    }),
                },
            },
        )
        .unwrap();
    f.app
        .world_mut()
        .resource_mut::<Assets<MapManifestAsset>>()
        .get_mut(&f.manifest)
        .unwrap()
        .sub_maps = Arc::from([]);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if state.advance(f.app.world_mut()).is_err() {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        f.app
            .world()
            .resource::<MapInstanceAuthorPreview>()
            .map_id
            .is_none()
    );
    let mut f = fixture();
    let mut state = State::default();
    send(
        &mut f,
        &mut state,
        TerrainObjectTransform {
            translation: [4., 0., 0.],
            ..default()
        },
        false,
        1,
    );
    f.app
        .world_mut()
        .resource_mut::<MapInstanceAuthorPreview>()
        .revision += 1; // 另一写者取得新信用。
    let adopted = f
        .app
        .world()
        .resource::<MapInstanceAuthorPreview>()
        .instances
        .clone();
    f.app.world_mut().resource_mut::<ActiveDynamicMap>().map_id = None;
    settle(&mut f, &mut state);
    assert!(Arc::ptr_eq(
        &adopted,
        &f.app
            .world()
            .resource::<MapInstanceAuthorPreview>()
            .instances
    ));
    assert!(
        f.app
            .world()
            .resource::<TerrainObjectTransformPreview>()
            .pose
            .is_none()
    );
}
