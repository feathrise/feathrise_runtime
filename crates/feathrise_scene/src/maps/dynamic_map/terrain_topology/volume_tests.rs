//! 体积候选通过生产联合发布系统验证，不能用高度完成代替模型、拾取和物理完成。
use super::*;
use crate::shared::placement_data::terrain_topology_test_support::{
    collision_fixture, install_proxy_observers, physical_collider,
};
use avian3d::prelude::Collider;

fn setup() -> (App, Entity, feathrise_map_content::MapPlacementRecord) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    install_proxy_observers(&mut app);
    let map = MapId("MAP-volume-publication".into());
    let terrain = app.world_mut().spawn_empty().id();
    let record: feathrise_map_content::MapPlacementRecord = serde_json::from_value(serde_json::json!({
        "id":"volume-test","asset_path":"maps/volume/v.glb","translation":[0,0,0],"yaw_degrees":0,"scale":[1,1,1],
        "terrain_patch":{"sculpted_volume":true,"cutout":{"min":[0,0],"max":[8,8]}}
    })).unwrap();
    let root = app
        .world_mut()
        .spawn((
            Visibility::Hidden,
            GlobalTransform::IDENTITY,
            crate::MapPlacementInstance::new(&map, &record),
        ))
        .id();
    let shape = Collider::trimesh(
        vec![
            Vec3::ZERO,
            Vec3::new(0., 4., 8.),
            Vec3::new(8., 4., 8.),
            Vec3::new(8., 0., 0.),
        ],
        vec![[0, 1, 2], [0, 2, 3]],
    );
    app.world_mut().entity_mut(root).insert(collision_fixture(
        1,
        shape,
        Arc::from([(root, Handle::default())]),
    ));
    let hole = record
        .terrain_cutout(Rect::from_corners(Vec2::ZERO, Vec2::splat(100.)))
        .unwrap()
        .unwrap();
    app.insert_resource(ActiveDynamicMap {
        map_id: Some(map.clone()),
    })
    .insert_resource(DynamicMapRuntimeState {
        active_map_id: Some(map.clone()),
        root: Some(terrain),
        sparse_height_path: Some("maps/volume/height.json".into()),
        ..default()
    })
    .insert_resource(TerrainVolumePreviewRequest {
        target: Some(TerrainVolumePreviewTarget {
            map_id: map,
            entity: root,
            record: record.clone(),
            world_cutout: hole,
            removal: false,
        }),
        ..default()
    })
    .init_resource::<TerrainTopologyRuntime>()
    .init_resource::<StaticTerrainRaytracingSources>()
    .add_systems(
        Update,
        (collect_terrain_topology, publish_terrain_topology).chain(),
    );
    (app, root, record)
}
fn adopt_height(app: &mut App) {
    let holes = app
        .world()
        .resource::<TerrainTopologyRuntime>()
        .desired
        .clone();
    app.world_mut()
        .resource_mut::<TerrainTopologyRuntime>()
        .adopt(holes);
}

#[test]
fn volume_stream_formal_save_keeps_resident_root_and_rebases_formal_generation() {
    use crate::shared::placement_data::terrain_topology_test_support::closing_candidate;
    let (mut app, root, mut record) = setup();
    let patch = record.terrain_patch.as_mut().unwrap();
    patch.streamed_volume = true;
    patch.volume_cell_size = Some(0.25);
    let map = app
        .world()
        .resource::<ActiveDynamicMap>()
        .map_id
        .clone()
        .unwrap();
    app.world_mut()
        .entity_mut(root)
        .insert(crate::MapPlacementInstance::new(&map, &record));
    app.world_mut()
        .resource_mut::<TerrainVolumePreviewRequest>()
        .target
        .as_mut()
        .unwrap()
        .record = record;
    app.update();
    adopt_height(&mut app);
    app.update();
    let container = app.world_mut().spawn(Visibility::Hidden).id();
    app.insert_resource(closing_candidate(map, container));
    app.update();
    assert!(app.world().get_entity(root).is_ok());
    assert!(physical_collider(app.world(), root).is_some());
    assert!(
        app.world()
            .resource::<TerrainVolumePreviewRequest>()
            .is_adopted(root, false)
    );
    let topology = app.world().resource::<TerrainTopologyRuntime>();
    assert_eq!(topology.applied_candidate, Some(([4, 9], 0)));
    assert!(!topology.applied.is_empty());
    let placement = app.world().resource::<MapPlacementRuntimeState>();
    assert!(placement.terrain_holes_applied.is_empty());
    assert_eq!(placement.terrain_mesh_generation, Some([4, 9]));
    {
        let mut state = app.world_mut().resource_mut::<MapPlacementRuntimeState>();
        state.revision = 10;
        state.pending.as_mut().unwrap().revision = 10;
        state.scene_load.as_mut().unwrap().revision = 10;
        state.staging.as_mut().unwrap().revision = 10;
    }
    app.update();
    assert_eq!(
        app.world().get::<Visibility>(root),
        Some(&Visibility::Inherited)
    );
    assert!(physical_collider(app.world(), root).is_some());
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
            .applied_candidate,
        Some(([4, 10], 0))
    );
}

#[test]
fn volume_scene_exit_clears_pending_or_active_physics_pick_and_ray_sources() {
    for adopted in [false, true] {
        let (mut app, root, _) = setup();
        app.update();
        if adopted {
            adopt_height(&mut app);
            app.update();
            assert!(physical_collider(app.world(), root).is_some());
        }
        app.add_systems(Last, clear_terrain_topology);
        app.update();
        assert!(app.world().get_entity(root).is_err());
        assert!(physical_collider(app.world(), root).is_none());
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<Entity, With<Collider>>()
                .iter(world)
                .count(),
            0,
            "离场不得遗留脱离作者根的独立物理实体"
        );
        let topology = app.world().resource::<TerrainTopologyRuntime>();
        assert!(topology.map.is_none() && topology.root.is_none());
        assert!(topology.applied.is_empty() && topology.desired.is_empty());
        assert!(topology.surfaces.is_empty() && topology.volume.pending.is_none());
        assert!(topology.volume.base.is_none());
        assert_eq!(
            app.world()
                .resource::<StaticTerrainRaytracingSources>()
                .meshes()
                .count(),
            0
        );
        let request = app.world().resource::<TerrainVolumePreviewRequest>();
        assert!(request.target.is_none() && request.adopted_root().is_none());
    }
}

#[test]
fn volume_preview_waits_for_height_and_jointly_publishes_mesh_pick_and_physics() {
    let (mut app, root, _) = setup();
    app.update();
    assert!(
        !app.world()
            .resource::<TerrainTopologyRuntime>()
            .desired
            .is_empty()
    );
    assert_eq!(
        app.world().get::<Visibility>(root),
        Some(&Visibility::Hidden)
    );
    assert!(physical_collider(app.world(), root).is_none());
    assert_eq!(
        app.world()
            .resource::<StaticTerrainRaytracingSources>()
            .meshes()
            .count(),
        0
    );
    adopt_height(&mut app);
    app.update();
    assert!(
        app.world()
            .resource::<TerrainVolumePreviewRequest>()
            .is_adopted(root, false)
    );
    assert_eq!(
        app.world().get::<Visibility>(root),
        Some(&Visibility::Inherited)
    );
    assert!(physical_collider(app.world(), root).is_some());
    assert_eq!(
        app.world()
            .resource::<StaticTerrainRaytracingSources>()
            .meshes()
            .count(),
        1
    );
    let query = &app.world().resource::<TerrainTopologyRuntime>().surfaces[0];
    assert!(query.ray(Vec3::new(4., 10., 4.), -Vec3::Y, 100.).is_some());
}

#[test]
fn volume_creation_undo_restores_height_before_removing_pick_rt_and_real_collider() {
    let (mut app, root, _) = setup();
    app.update();
    adopt_height(&mut app);
    app.update();
    app.world_mut()
        .resource_mut::<TerrainVolumePreviewRequest>()
        .target
        .as_mut()
        .unwrap()
        .removal = true;
    app.update();
    assert!(
        app.world()
            .resource::<TerrainTopologyRuntime>()
            .desired
            .is_empty()
    );
    assert!(
        !app.world()
            .resource::<TerrainVolumePreviewRequest>()
            .is_adopted(root, true)
    );
    assert!(physical_collider(app.world(), root).is_some()); // 脚下旧支撑保留到高度面恢复。
    adopt_height(&mut app);
    app.update();
    assert!(
        app.world()
            .resource::<TerrainVolumePreviewRequest>()
            .is_adopted(root, true)
    );
    assert!(physical_collider(app.world(), root).is_none());
    assert!(
        app.world()
            .resource::<TerrainTopologyRuntime>()
            .surfaces
            .is_empty()
    );
    assert_eq!(
        app.world()
            .resource::<StaticTerrainRaytracingSources>()
            .meshes()
            .count(),
        0
    );
    assert_eq!(
        app.world().get::<Visibility>(root),
        Some(&Visibility::Hidden)
    );
}

#[test]
fn volume_pending_hot_reload_is_rejected_even_when_hole_rect_is_unchanged() {
    let (mut app, root, _) = setup();
    app.update();
    app.world_mut()
        .entity_mut(root)
        .remove::<TerrainPatchColliderReady>();
    adopt_height(&mut app);
    app.update();
    let topology = app.world().resource::<TerrainTopologyRuntime>();
    assert!(topology.volume.pending.is_none());
    assert!(topology.desired.is_empty());
    assert!(
        !app.world()
            .resource::<TerrainVolumePreviewRequest>()
            .is_adopted(root, false)
    );
    assert!(physical_collider(app.world(), root).is_none());
    assert_eq!(
        app.world().get::<Visibility>(root),
        Some(&Visibility::Hidden)
    );
}

#[test]
fn volume_cancel_waits_for_height_restoration_and_then_retires_owned_root() {
    let (mut app, root, _) = setup();
    app.update();
    adopt_height(&mut app);
    app.update();
    app.world_mut()
        .resource_mut::<TerrainVolumePreviewRequest>()
        .target = None;
    app.update();
    assert!(physical_collider(app.world(), root).is_some());
    adopt_height(&mut app);
    app.update();
    assert!(app.world().get_entity(root).is_err());
    assert!(
        app.world()
            .resource::<TerrainTopologyRuntime>()
            .applied
            .is_empty()
    );
    assert!(
        app.world()
            .resource::<TerrainVolumePreviewRequest>()
            .adopted_root()
            .is_none()
    );
}

#[test]
fn volume_expanded_coverage_and_undo_replace_formal_holes_with_one_joint_mesh_root() {
    let (mut app, preview, original) = setup();
    let map = app
        .world()
        .resource::<ActiveDynamicMap>()
        .map_id
        .clone()
        .unwrap();
    let terrain = app
        .world()
        .resource::<DynamicMapRuntimeState>()
        .root
        .unwrap();
    let bounds = Rect::from_corners(Vec2::splat(-128.), Vec2::splat(128.));
    let old_holes = original.terrain_cutouts(bounds).unwrap();
    let shape = app
        .world()
        .get::<TerrainPatchColliderReady>(preview)
        .unwrap()
        .collider
        .clone();
    let formal = app
        .world_mut()
        .spawn((
            Visibility::Inherited,
            GlobalTransform::IDENTITY,
            crate::MapPlacementInstance::new(&map, &original),
        ))
        .id();
    app.world_mut().entity_mut(formal).insert(collision_fixture(
        2,
        shape.clone(),
        Arc::from([(formal, Handle::default())]),
    ));
    app.world_mut()
        .entity_mut(formal)
        .insert(crate::shared::placement_data::InstallTerrainPatchCollider);
    let formal_ready = app
        .world()
        .get::<TerrainPatchColliderReady>(formal)
        .unwrap();
    let query = TerrainMeshQuery::new(formal_ready.collider.clone(), Mat4::IDENTITY).unwrap();
    let batch = formal_ready.meshes.clone();
    {
        let mut topology = app.world_mut().resource_mut::<TerrainTopologyRuntime>();
        topology.map = Some(map.clone());
        topology.root = Some(terrain);
        topology.applied = old_holes.clone().into();
        topology.surfaces = Arc::from([query]);
        topology.active_roots = vec![formal];
    }
    app.world_mut()
        .resource_mut::<StaticTerrainRaytracingSources>()
        .batches = vec![batch];
    let mut expanded = original.clone();
    let patch = expanded.terrain_patch.as_mut().unwrap();
    patch.cutout = None;
    patch.cutouts = vec![
        old_holes[0],
        MapTerrainCutout {
            min: [32., 0.],
            max: [40., 8.],
        },
    ];
    app.world_mut()
        .entity_mut(preview)
        .insert(crate::MapPlacementInstance::new(&map, &expanded));
    let hole = expanded.terrain_cutout(bounds).unwrap().unwrap();
    {
        let mut request = app
            .world_mut()
            .resource_mut::<TerrainVolumePreviewRequest>();
        let target = request.target.as_mut().unwrap();
        target.record = expanded.clone();
        target.world_cutout = hole;
    }
    app.update();
    assert_eq!(
        app.world()
            .resource::<TerrainTopologyRuntime>()
            .desired
            .as_ref(),
        expanded.terrain_cutouts(bounds).unwrap()
    );
    assert!(physical_collider(app.world(), formal).is_some());
    assert!(physical_collider(app.world(), preview).is_none());
    adopt_height(&mut app);
    app.update();
    assert!(physical_collider(app.world(), formal).is_none());
    assert!(physical_collider(app.world(), preview).is_some());
    assert_eq!(
        app.world()
            .resource::<StaticTerrainRaytracingSources>()
            .meshes()
            .count(),
        1
    );
    assert!(
        !app.world()
            .resource::<TerrainTopologyRuntime>()
            .applied
            .iter()
            .any(|h| h.contains(Vec2::new(20., 4.))),
        "断开覆盖之间保留基础地表"
    );

    let restored = app
        .world_mut()
        .spawn((
            Visibility::Hidden,
            GlobalTransform::IDENTITY,
            crate::MapPlacementInstance::new(&map, &original),
        ))
        .id();
    app.world_mut()
        .entity_mut(restored)
        .insert(collision_fixture(
            3,
            shape,
            Arc::from([(restored, Handle::default())]),
        ));
    app.world_mut()
        .resource_mut::<TerrainVolumePreviewRequest>()
        .target = Some(TerrainVolumePreviewTarget {
        map_id: map,
        entity: restored,
        record: original.clone(),
        world_cutout: old_holes[0],
        removal: false,
    });
    app.update();
    assert_eq!(
        app.world()
            .resource::<TerrainTopologyRuntime>()
            .desired
            .as_ref(),
        old_holes
    );
    assert!(
        physical_collider(app.world(), preview).is_some(),
        "撤销扩展须等基础地表恢复"
    );
    adopt_height(&mut app);
    app.update();
    assert!(app.world().get_entity(preview).is_err());
    assert!(physical_collider(app.world(), restored).is_some());
    assert_eq!(
        app.world()
            .resource::<TerrainTopologyRuntime>()
            .applied
            .as_ref(),
        old_holes
    );
    app.world_mut()
        .resource_mut::<TerrainVolumePreviewRequest>()
        .target = None;
    app.update();
    adopt_height(&mut app);
    app.update();
    assert!(physical_collider(app.world(), formal).is_some());
    assert!(app.world().get_entity(restored).is_err());
    assert_eq!(
        app.world()
            .resource::<StaticTerrainRaytracingSources>()
            .meshes()
            .count(),
        1
    );
}
