//! 流送任务换代、编辑暂停、超时以及高度碰撞采用前的完整旧窗口保持。
use super::super::{resources::*, terrain_topology::*};
use super::resources::Runtime;
use super::types::*;
use super::*;
use crate::shared::placement_data::terrain_topology_test_support::{
    collision_fixture, install_proxy_observers,
};
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    tasks::IoTaskPool,
};
use feathrise_map_content::{ActiveDynamicMap, MapId, MapPlacementRecord};
use sha2::Digest;
use std::sync::Arc;

fn mesh() -> Mesh {
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    m.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![[0., 0., 0.], [0., 0., 8.], [8., 0., 0.]],
    );
    m.insert_indices(Indices::U32(vec![0, 1, 2]));
    m
}
fn setup() -> (App, Identity) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    install_proxy_observers(&mut app);
    let id = MapId("MAP-stream-test".into());
    let root = app.world_mut().spawn_empty().id();
    let record:MapPlacementRecord=serde_json::from_value(serde_json::json!({"id":"volume","asset_path":"maps/STREAM/volume_sculpt/v-test.glb","translation":[0,0,0],"yaw_degrees":0,"scale":[1,1,1],"terrain_patch":{"sculpted_volume":true,"streamed_volume":true,"volume_cell_size":0.25,"cutouts":[{"min":[0,0],"max":[8,8]}]}})).unwrap();
    let identity = Identity {
        lods: default(),
        map: id.clone(),
        root,
        record: record.clone(),
        center: [0; 3],
        physics_center: None,
    };
    app.world_mut().spawn((
        Camera::default(),
        super::super::sparse_runtime::DynamicClipmapAuthoringView::default(),
    ));
    app.insert_resource(ActiveDynamicMap {
        map_id: Some(id.clone()),
    })
    .insert_resource(DynamicMapRuntimeState {
        active_map_id: Some(id.clone()),
        root: Some(root),
        sparse_height_path: Some("height".into()),
        ready_released: true,
        ..default()
    })
    .insert_resource(TerrainVolumeStreamControl {
        draft: Some((id, record)),
        ..default()
    })
    .init_resource::<TerrainVolumeStreamStatus>()
    .init_resource::<TerrainVolumePreviewRequest>()
    .init_resource::<Runtime>()
    .init_resource::<DynamicMapRuntimeStatus>()
    .init_resource::<TerrainTopologyRuntime>()
    .init_resource::<StaticTerrainRaytracingSources>()
    .init_resource::<Assets<Mesh>>()
    .init_resource::<Assets<StandardMaterial>>()
    .add_systems(
        Update,
        (
            systems::drive.in_set(TerrainVolumeStreamSystems::Drive),
            collect_terrain_topology,
            publish_terrain_topology,
        )
            .chain(),
    );
    crate::shared::placement_data::terrain_topology_test_support::install_mesh_upload(&mut app);
    (app, identity)
}
fn work(app: &mut App, identity: Identity, mesh: Option<Mesh>, started: f64) {
    let record = identity.record.clone();
    app.world_mut().resource_mut::<Runtime>().work = Some(Work {
        identity,
        task: IoTaskPool::get().spawn(async move {
            Ok(Built {
                record,
                meshes: mesh.into_iter().collect(),
                fields: default(),
            })
        }),
        started,
        progress: Default::default(),
        observed: 0,
    });
}
fn tick(app: &mut App, predicate: impl Fn(&World) -> bool) {
    for _ in 0..100 {
        app.update();
        if predicate(app.world()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("单飞任务未按预期收口");
}
fn adopt_height(app: &mut App) {
    let h = app
        .world()
        .resource::<TerrainTopologyRuntime>()
        .desired
        .clone();
    app.world_mut()
        .resource_mut::<TerrainTopologyRuntime>()
        .adopt(h);
}
#[test]
fn volume_stream_initial_missing_graphics_reports_failure_instead_of_waiting_forever() {
    let (mut app, id) = setup();
    app.world_mut()
        .resource_mut::<DynamicMapRuntimeState>()
        .ready_released = false;
    app.world_mut()
        .remove_resource::<Assets<StandardMaterial>>();
    work(&mut app, id.clone(), Some(mesh()), 0.);
    tick(&mut app, |w| {
        w.resource::<Runtime>().failed.as_ref() == Some(&id)
    });
    let status = app.world().resource::<DynamicMapRuntimeStatus>();
    assert_eq!(status.phase, DynamicMapRuntimePhase::Failed);
    assert!(status.detail.contains("未安装"));
    assert!(!app.world().resource::<TerrainVolumeStreamStatus>().ready);
    assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 0);
    assert!(
        app.world()
            .resource::<TerrainVolumePreviewRequest>()
            .target
            .is_none()
    );
}

#[test]
fn volume_stream_reuses_editor_root_only_when_view_and_player_foci_match() {
    for moved in 0..3 {
        let (mut app, id) = setup();
        work(&mut app, id.clone(), Some(mesh()), 0.);
        tick(&mut app, |w| w.resource::<Runtime>().pending.is_some());
        let root = app
            .world()
            .resource::<Runtime>()
            .pending
            .as_ref()
            .unwrap()
            .root;
        app.world_mut().entity_mut(root).insert((
            GlobalTransform::IDENTITY,
            collision_fixture(
                1,
                avian3d::prelude::Collider::trimesh(
                    vec![Vec3::ZERO, Vec3::Z * 8., Vec3::X * 8.],
                    vec![[0, 1, 2]],
                ),
                Arc::from([(root, Handle::default())]),
            ),
        ));
        app.update();
        adopt_height(&mut app);
        tick(&mut app, |w| {
            w.resource::<Runtime>().active.as_ref() == Some(&id)
        });
        // 模拟F12刚采用同一版本的新根，观察焦点或玩家已在等待期间换到远处。
        let mut runtime = app.world_mut().resource_mut::<Runtime>();
        runtime.active = None;
        runtime.observed_root = None;
        drop(runtime);
        if moved == 1 {
            app.world_mut()
                .resource_mut::<TerrainVolumeStreamControl>()
                .focus_override = Some(Vec3::new(804., 4., 804.));
        } else if moved == 2 {
            app.world_mut().spawn((
                crate::SceneRuntimeObserver,
                Transform::from_translation(Vec3::new(804., 4., 804.)),
            ));
        }
        app.update();
        let runtime = app.world().resource::<Runtime>();
        if moved == 0 {
            assert_eq!(runtime.active.as_ref().map(|i| i.center), Some([0; 3]));
            assert!(runtime.work.is_none());
        } else {
            assert!(runtime.active.is_none());
            let next = &runtime
                .work
                .as_ref()
                .expect("焦点改变必须重新加载")
                .identity;
            if moved == 1 {
                assert_eq!(next.center, [100, 0, 100]);
            } else {
                assert_eq!(next.physics_center, Some([100, 0, 100]));
            }
        }
        assert_eq!(
            app.world().get::<Visibility>(root),
            Some(&Visibility::Inherited)
        );
    }
}

#[test]
fn volume_stream_window_waits_for_joint_adoption_and_empty_source_removal_preserves_old_until_ready()
 {
    let (mut app, id) = setup();
    work(&mut app, id.clone(), Some(mesh()), 0.);
    tick(&mut app, |w| w.resource::<Runtime>().pending.is_some());
    let root = app
        .world()
        .resource::<Runtime>()
        .pending
        .as_ref()
        .unwrap()
        .root;
    assert_eq!(
        app.world().get::<Visibility>(root),
        Some(&Visibility::Hidden)
    );
    app.world_mut().entity_mut(root).insert((
        GlobalTransform::IDENTITY,
        collision_fixture(
            1,
            avian3d::prelude::Collider::trimesh(
                vec![Vec3::ZERO, Vec3::Z * 8., Vec3::X * 8.],
                vec![[0, 1, 2]],
            ),
            Arc::from([(root, Handle::default())]),
        ),
    ));
    app.update();
    assert!(
        !app.world()
            .resource::<TerrainVolumePreviewRequest>()
            .is_adopted(root, false)
    );
    adopt_height(&mut app);
    tick(&mut app, |w| {
        w.resource::<Runtime>().active.as_ref() == Some(&id)
    });
    assert_eq!(
        app.world().get::<Visibility>(root),
        Some(&Visibility::Inherited)
    );
    assert!(app.world().resource::<TerrainVolumeStreamStatus>().ready);
    let mut far = id.clone();
    far.center = [100, 0, 100];
    app.world_mut()
        .resource_mut::<TerrainVolumeStreamControl>()
        .focus_override = Some(Vec3::new(804., 4., 804.));
    work(&mut app, far.clone(), None, 0.);
    tick(&mut app, |w| w.resource::<Runtime>().pending.is_some());
    assert_eq!(
        app.world().get::<Visibility>(root),
        Some(&Visibility::Inherited)
    );
    assert!(
        !app.world()
            .resource::<TerrainVolumePreviewRequest>()
            .is_adopted(root, true)
    );
    adopt_height(&mut app);
    tick(&mut app, |w| {
        w.resource::<Runtime>().active.as_ref() == Some(&far)
    });
    assert_eq!(
        app.world().get::<Visibility>(root),
        Some(&Visibility::Hidden)
    );
    assert!(
        app.world()
            .resource::<TerrainTopologyRuntime>()
            .applied
            .is_empty()
    );
    assert_eq!(
        app.world()
            .resource::<StaticTerrainRaytracingSources>()
            .meshes()
            .count(),
        0
    );
}
#[test]
fn volume_stream_late_cancelled_changed_map_or_source_results_never_register_mesh() {
    for reason in 0..3 {
        let (mut app, id) = setup();
        work(&mut app, id.clone(), Some(mesh()), 0.);
        let mut control = app.world_mut().resource_mut::<TerrainVolumeStreamControl>();
        if reason == 0 {
            control.editing = true;
        } else if reason == 1 {
            control.draft = None;
        } else {
            control.draft.as_mut().unwrap().0 = MapId("other".into());
        }
        drop(control);
        tick(&mut app, |w| w.resource::<Runtime>().work.is_none());
        assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 0);
        assert!(
            app.world()
                .resource::<TerrainVolumePreviewRequest>()
                .target
                .is_none()
        );
        assert!(!app.world().resource::<TerrainVolumeStreamStatus>().ready);
    }
}
#[test]
fn volume_stream_large_job_is_not_rejected_by_elapsed_time_and_editor_pause_holds_slot() {
    let (mut app, id) = setup();
    work(&mut app, id.clone(), Some(mesh()), -31.);
    tick(&mut app, |w| w.resource::<Runtime>().work.is_none());
    assert!(app.world().resource::<Runtime>().pending.is_some());
    let (mut app, id) = setup(); // 编辑取消独立测试一份尚未完成的任务，不借用上一候选。
    let (tx, rx) = std::sync::mpsc::channel();
    let record = id.record.clone();
    app.world_mut().resource_mut::<Runtime>().work = Some(Work {
        identity: id.clone(),
        task: IoTaskPool::get().spawn(async move {
            rx.recv().unwrap();
            Ok(Built {
                fields: default(),
                record,
                meshes: vec![mesh()],
            })
        }),
        started: 0.,
        progress: Default::default(),
        observed: 0,
    });
    app.world_mut()
        .resource_mut::<TerrainVolumeStreamControl>()
        .editing = true;
    app.update();
    assert!(
        app.world()
            .resource::<TerrainVolumeStreamControl>()
            .native_busy
    );
    assert!(app.world().resource::<Runtime>().work.is_some());
    tx.send(()).unwrap();
    tick(&mut app, |w| w.resource::<Runtime>().work.is_none());
    assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 0);
}

#[test]
fn volume_stream_active_proxy_preparation_does_not_expire_by_total_duration() {
    use std::sync::atomic::Ordering;
    let (mut app, id) = setup();
    work(&mut app, id.clone(), Some(mesh()), -31.);
    app.world()
        .resource::<Runtime>()
        .work
        .as_ref()
        .unwrap()
        .progress
        .store(1, Ordering::Relaxed);
    tick(&mut app, |w| w.resource::<Runtime>().pending.is_some());
    assert!(app.world().resource::<Runtime>().failed.is_none());
    assert!(
        app.world()
            .resource::<Runtime>()
            .pending
            .as_ref()
            .unwrap()
            .identity
            == id
    );
    assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 1);
}

#[test]
fn volume_stream_real_saved_root_keeps_far_geometry_and_switches_after_joint_adoption() {
    use feathrise_map_terrain::volume::{
        VolumeBrush, VolumeBrushKind,
        sparse::{paged::*, *},
    };
    fn queue(app: &mut App, id: &Identity, built: Built) -> Entity {
        app.world_mut().resource_mut::<Runtime>().work = Some(Work {
            identity: id.clone(),
            task: IoTaskPool::get().spawn(async move { Ok(built) }),
            started: 0.,
            progress: Default::default(),
            observed: 0,
        });
        tick(app, |w| w.resource::<Runtime>().pending.is_some());
        app.world()
            .resource::<Runtime>()
            .pending
            .as_ref()
            .unwrap()
            .root
    }
    fn ready(app: &mut App, root: Entity) {
        app.world_mut().entity_mut(root).insert((
            GlobalTransform::IDENTITY,
            collision_fixture(
                1,
                avian3d::prelude::Collider::trimesh(
                    vec![Vec3::ZERO, Vec3::Z * 8., Vec3::X * 8.],
                    vec![[0, 1, 2]],
                ),
                Arc::from([(root, Handle::default())]),
            ),
        ));
    }
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let temp = Temp(std::env::temp_dir().join(format!(
            "rpg-scene-volume-stream-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    std::fs::create_dir(&temp.0).unwrap();
    let objects = temp.0.join("maps/STREAM/volume_sculpt/objects");
    let data = apply_sparse_brush(
        &new_sparse_volume(0.125, Arc::default()).unwrap(),
        VolumeBrush {
            kind: VolumeBrushKind::Dig,
            from: [4., 0., 4.],
            to: [4., 0., 4.],
            radius: 2.,
            strength: 1.,
            ..Default::default()
        },
        Rect::from_corners(Vec2::splat(-64.), Vec2::splat(64.)),
        |_| Ok(0.07),
    )
    .unwrap();
    let data = apply_sparse_brush(
        &data,
        VolumeBrush {
            kind: VolumeBrushKind::Dig,
            from: [44., 0., 44.],
            to: [44., 0., 44.],
            radius: 2.,
            strength: 1.,
            ..Default::default()
        },
        Rect::from_corners(Vec2::splat(-64.), Vec2::splat(64.)),
        |_| Ok(0.07),
    )
    .unwrap();
    let source = write_delta(
        &objects,
        &PagedVolume::new(0.125).unwrap(),
        prepare_delta(None, &data).unwrap(),
    )
    .unwrap();
    let bytes = encode_paged_volume(&source).unwrap();
    let hash: String = sha2::Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let (mut app, mut id) = setup();
    id.record.asset_path = format!("maps/STREAM/volume_sculpt/v-{hash}.glb");
    let patch = id.record.terrain_patch.as_mut().unwrap();
    patch.volume_cell_size = Some(0.125);
    patch.cutouts = source
        .footprint()
        .into_iter()
        .map(|h| feathrise_map_content::MapTerrainCutout {
            min: h[0],
            max: h[1],
        })
        .collect();
    std::fs::write(temp.0.join(&id.record.asset_path), &bytes).unwrap();
    let built = bevy::tasks::block_on(jobs::build_at(&temp.0, id.clone())).unwrap();
    assert!(built.meshes.iter().map(Mesh::count_vertices).sum::<usize>() > 3);
    assert_eq!(built.record.asset_path, id.record.asset_path);
    app.world_mut()
        .resource_mut::<TerrainVolumeStreamControl>()
        .draft = Some((id.map.clone(), id.record.clone()));
    let old = queue(&mut app, &id, built);
    ready(&mut app, old);
    app.update();
    assert!(
        !app.world()
            .resource::<TerrainVolumePreviewRequest>()
            .is_adopted(old, false)
    );
    adopt_height(&mut app);
    tick(&mut app, |w| {
        w.resource::<Runtime>().active.as_ref() == Some(&id)
    });
    assert_eq!(
        app.world().get::<Visibility>(old),
        Some(&Visibility::Inherited)
    );
    id.center = [100, 0, 100];
    let built = bevy::tasks::block_on(jobs::build_at(&temp.0, id.clone())).unwrap();
    assert_eq!(
        built.record.terrain_patch.as_ref().unwrap().cutouts.len(),
        2
    );
    let points: Vec<_> = built
        .meshes
        .iter()
        .flat_map(|m| {
            let bevy::mesh::VertexAttributeValues::Float32x3(points) =
                m.attribute(Mesh::ATTRIBUTE_POSITION).unwrap()
            else {
                panic!()
            };
            points.iter()
        })
        .collect();
    assert!(points.iter().any(|p| p[0] < 8.) && points.iter().any(|p| p[0] > 40.));
    // 使用真实远景读取结果驱动运行时，不允许走mesh=None的洞口移除路径。
    let mut control = app.world_mut().resource_mut::<TerrainVolumeStreamControl>();
    control.draft = Some((id.map.clone(), id.record.clone()));
    control.focus_override = Some(Vec3::new(402., 2., 402.));
    drop(control);
    let root = queue(&mut app, &id, built);
    assert!(
        !app.world()
            .resource::<TerrainVolumePreviewRequest>()
            .target
            .as_ref()
            .unwrap()
            .removal
    );
    assert_eq!(
        app.world().get::<Visibility>(root),
        Some(&Visibility::Hidden)
    );
    assert_eq!(
        app.world().get::<Visibility>(old),
        Some(&Visibility::Inherited)
    );
    assert!(
        !app.world()
            .resource::<TerrainVolumePreviewRequest>()
            .is_adopted(root, false)
    );
    // 洞口覆盖未变化，原高度裁切信用可复用；新的粗细网格碰撞仍必须独立Ready。
    ready(&mut app, root);
    tick(&mut app, |w| {
        w.resource::<Runtime>().active.as_ref() == Some(&id)
    });
    assert_eq!(
        app.world().get::<Visibility>(root),
        Some(&Visibility::Inherited)
    );
    assert!(
        app.world()
            .get::<Visibility>(old)
            .is_none_or(|v| *v == Visibility::Hidden)
    );
    assert_eq!(
        app.world()
            .resource::<StaticTerrainRaytracingSources>()
            .meshes()
            .count(),
        1
    );
    assert_eq!(
        app.world()
            .resource::<TerrainTopologyRuntime>()
            .applied
            .len(),
        2
    );
    id.record.terrain_patch.as_mut().unwrap().volume_cell_size = Some(0.25);
    assert!(bevy::tasks::block_on(jobs::build_at(&temp.0, id)).is_err());
}
