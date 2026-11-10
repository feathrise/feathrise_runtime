//! 正式洞口与三维网格采用证明同批发布，旧完整状态不能被候选前缀覆盖。
use super::*;
use crate::shared::placement_data::terrain_topology_test_support::{
    closing_candidate, collision_fixture, install_proxy_observers, physical_collider,
};

/// 删除最后一个洞穴同样必须取得完整候选资格，空摆放文件不能绕过换代门。
fn closing_last_cave() -> (App, Entity, Arc<[SceneTerrainCutout]>) {
    let mut app = App::new();
    install_proxy_observers(&mut app);
    let id = SceneId("MAP-cave-publication".into());
    let root = app.world_mut().spawn_empty().id();
    let container = app.world_mut().spawn(Visibility::Hidden).id();
    let holes: Arc<[SceneTerrainCutout]> = Arc::from([SceneTerrainCutout {
        min: [-1.0, -1.0],
        max: [1.0, 1.0],
    }]);
    app.insert_resource(ActiveDynamicScene {
        scene_id: Some(id.clone()),
    })
    .insert_resource(DynamicSceneRuntimeState {
        active_scene_id: Some(id.clone()),
        root: Some(root),
        sparse_height_path: Some("scenes/cave/height.scene_height_manifest.json".into()),
        ..default()
    })
    .insert_resource(TerrainTopologyRuntime {
        scene_id: Some(id.clone()),
        root: Some(root),
        desired: holes.clone(),
        applied: holes.clone(),
        ..default()
    })
    .insert_resource(closing_candidate(id, container))
    .init_resource::<crate::ScenePlacementRuntimeStatus>()
    .init_resource::<StaticTerrainRaytracingSources>()
    .add_systems(Update, collect_terrain_topology);
    (app, container, holes)
}

#[test]
fn dirty_failed_or_removed_candidate_keeps_last_complete_holes() {
    let (mut app, container, holes) = closing_last_cave();
    app.world_mut()
        .resource_mut::<ScenePlacementRuntimeState>()
        .dirty = true;
    app.update();
    assert_eq!(
        app.world().resource::<TerrainTopologyRuntime>().desired,
        holes
    );
    app.world_mut()
        .resource_mut::<ScenePlacementRuntimeState>()
        .dirty = false;
    app.world_mut()
        .resource_mut::<crate::ScenePlacementRuntimeStatus>()
        .phase = crate::ScenePlacementRuntimePhase::Failed;
    app.update();
    assert_eq!(
        app.world().resource::<TerrainTopologyRuntime>().desired,
        holes
    );
    app.world_mut()
        .resource_mut::<crate::ScenePlacementRuntimeStatus>()
        .phase = crate::ScenePlacementRuntimePhase::Building;
    app.world_mut()
        .resource_mut::<ScenePlacementRuntimeState>()
        .asset_retry_requested = true; // 碰撞热更完成后可留下唤醒标志；完整Ready证明仍拥有采用权。
    app.update();
    assert!(
        app.world()
            .resource::<TerrainTopologyRuntime>()
            .desired
            .is_empty()
    );
    app.world_mut().entity_mut(container).remove::<Visibility>();
    app.update();
    let topology = app.world().resource::<TerrainTopologyRuntime>();
    assert_eq!(topology.desired, holes);
    assert!(topology.candidate.is_none());
}

#[test]
fn pending_asset_recheck_and_catalog_change_revoke_topology_candidate() {
    let (mut app, _, holes) = closing_last_cave();
    app.update();
    assert!(
        app.world()
            .resource::<TerrainTopologyRuntime>()
            .candidate
            .is_some()
    );
    let scene = Handle::<bevy::world_serialization::WorldAsset>::default().id();
    app.world_mut()
        .resource_mut::<ScenePlacementRuntimeState>()
        .scene_load
        .as_mut()
        .unwrap()
        .scene_checks
        .push_back(scene);
    app.update();
    assert_eq!(
        app.world().resource::<TerrainTopologyRuntime>().desired,
        holes
    );
    app.world_mut()
        .resource_mut::<ScenePlacementRuntimeState>()
        .scene_load
        .as_mut()
        .unwrap()
        .scene_checks
        .clear();
    app.insert_resource(feathrise_scene_content::SceneContentRegistry {
        catalog: Handle::default(),
        revision: 0,
        scenes: default(),
        last_error: None,
    });
    app.update();
    assert!(
        app.world()
            .resource::<TerrainTopologyRuntime>()
            .candidate
            .is_some()
    );
    app.world_mut()
        .resource_mut::<feathrise_scene_content::SceneContentRegistry>()
        .revision += 1;
    app.update();
    let topology = app.world().resource::<TerrainTopologyRuntime>();
    assert_eq!(topology.desired, holes);
    assert!(topology.candidate.is_none());
}

#[test]
fn geometry_proof_and_raytracing_sources_wait_for_collision_holes() {
    let map = SceneId("MAP-cave-publication".into());
    let hole = Arc::from([SceneTerrainCutout {
        min: [-1.0, -1.0],
        max: [1.0, 1.0],
    }]);
    let mut topology = TerrainTopologyRuntime {
        scene_id: Some(map.clone()),
        desired: hole,
        ..default()
    };
    let mesh = Handle::<Mesh>::default();
    let entity = Entity::from_raw_u32(12).unwrap();
    topology.candidate = Some((
        [4, 9],
        2,
        Arc::default(),
        vec![Arc::from([(entity, mesh)])],
        Vec::new(),
    ));
    let state = ScenePlacementRuntimeState {
        active_scene_id: Some(map),
        ..default()
    };
    let mut app = App::new();
    app.insert_resource(topology)
        .insert_resource(state)
        .init_resource::<StaticTerrainRaytracingSources>()
        .add_systems(Update, publish_terrain_topology);
    app.update();
    assert_eq!(
        app.world()
            .resource::<ScenePlacementRuntimeState>()
            .terrain_mesh_generation,
        None
    );
    assert_eq!(
        app.world()
            .resource::<StaticTerrainRaytracingSources>()
            .meshes()
            .count(),
        0
    );
    let desired = app
        .world()
        .resource::<TerrainTopologyRuntime>()
        .desired
        .clone();
    app.world_mut()
        .resource_mut::<TerrainTopologyRuntime>()
        .adopt(desired); // 模拟正式后台碰撞整批成功采用。
    app.update();
    assert_eq!(
        app.world()
            .resource::<ScenePlacementRuntimeState>()
            .terrain_mesh_generation,
        Some([4, 9])
    );
    assert_eq!(
        app.world()
            .resource::<StaticTerrainRaytracingSources>()
            .meshes()
            .next()
            .unwrap()
            .0,
        entity
    );
    let revision = app
        .world()
        .resource::<StaticTerrainRaytracingSources>()
        .revision();
    app.update();
    assert_eq!(
        app.world()
            .resource::<StaticTerrainRaytracingSources>()
            .revision(),
        revision
    ); // 稳定帧不重建TLAS。
    app.world_mut()
        .resource_mut::<TerrainTopologyRuntime>()
        .candidate = None;
    app.update();
    assert_eq!(
        app.world()
            .resource::<StaticTerrainRaytracingSources>()
            .meshes()
            .count(),
        1
    ); // 取消新候选保留旧完整集合。
}

fn floor_collider(y: f32) -> avian3d::prelude::Collider {
    avian3d::prelude::Collider::try_trimesh_with_config(
        vec![
            Vec3::new(-2.0, y, -2.0),
            Vec3::new(-2.0, y, 2.0),
            Vec3::new(2.0, y, 2.0),
            Vec3::new(2.0, y, -2.0),
        ],
        vec![[0, 1, 2], [0, 2, 3]],
        avian3d::prelude::TrimeshFlags::FIX_INTERNAL_EDGES,
    )
    .unwrap()
}

#[test]
fn active_hot_reload_waits_for_all_current_colliders_then_replaces_queries_and_sources() {
    let (mut app, _, holes) = closing_last_cave();
    app.world_mut()
        .remove_resource::<ScenePlacementRuntimeState>();
    let id = app
        .world()
        .resource::<TerrainTopologyRuntime>()
        .scene_id
        .clone()
        .unwrap();
    let record: feathrise_scene_content::ScenePlacementRecord = serde_json::from_str(
        r#"{"id":"cave","asset_path":"scenes/cave.glb","translation":[0,0,0],"yaw_degrees":0,"scale":[1,1,1],"terrain_patch":{}}"#,
    )
    .unwrap();
    let source_a = app.world_mut().spawn_empty().id();
    let source_b = app.world_mut().spawn_empty().id();
    let old_batches: Vec<Arc<[(Entity, Handle<Mesh>)]>> = vec![
        Arc::from([(source_a, Handle::default())]),
        Arc::from([(source_b, Handle::default())]),
    ];
    let old_shapes = [floor_collider(-3.0), floor_collider(-5.0)];
    let roots: Vec<_> = old_shapes
        .iter()
        .zip(&old_batches)
        .map(|(shape, batch)| {
            app.world_mut()
                .spawn((
                    crate::ScenePlacementInstance::new(&id, &record),
                    collision_fixture(1, shape.clone(), batch.clone()),
                    crate::shared::placement_data::InstallTerrainPatchCollider,
                    GlobalTransform::default(),
                ))
                .id()
        })
        .collect();
    {
        let mut topology = app.world_mut().resource_mut::<TerrainTopologyRuntime>();
        topology.active_roots = roots.clone();
        topology.surfaces = old_shapes
            .iter()
            .map(|shape| TerrainMeshQuery::new(shape.clone(), Mat4::IDENTITY).unwrap())
            .collect::<Vec<_>>()
            .into();
    }
    app.world_mut()
        .resource_mut::<StaticTerrainRaytracingSources>()
        .batches = old_batches;
    let previous_queries = app
        .world()
        .resource::<TerrainTopologyRuntime>()
        .surfaces
        .clone();
    app.update();
    assert_eq!(app.world().resource::<TerrainTopologyRuntime>().revision, 0);

    // 一个模型已更新、另一个仍在烘焙时，拾取与光追必须保持上一整套来源。
    let source_c = app.world_mut().spawn_empty().id();
    let source_d = app.world_mut().spawn_empty().id();
    let batch_c = Arc::from([(source_c, Handle::default())]);
    let batch_d = Arc::from([(source_d, Handle::default())]);
    let current_a = collision_fixture(2, floor_collider(-6.0), batch_c);
    let current_b = collision_fixture(2, floor_collider(-8.0), batch_d);
    app.world_mut().entity_mut(roots[0]).insert(current_a.0);
    app.world_mut().entity_mut(roots[1]).insert(current_b);
    app.update();
    let topology = app.world().resource::<TerrainTopologyRuntime>();
    assert!(Arc::ptr_eq(&topology.surfaces, &previous_queries));
    assert_eq!(topology.applied, holes);
    assert_eq!(topology.revision, 0);
    let sources = app.world().resource::<StaticTerrainRaytracingSources>();
    assert_eq!(sources.revision(), 0);
    assert_eq!(
        sources.meshes().map(|item| item.0).collect::<Vec<_>>(),
        vec![source_a, source_b]
    );
    for (root, expected) in roots.iter().zip([13.0, 15.0]) {
        assert_eq!(
            physical_collider(app.world(), *root)
                .unwrap()
                .shape()
                .cast_local_ray(
                    &avian3d::parry::query::Ray::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y),
                    30.0,
                    false
                ),
            Some(expected)
        );
    }

    app.world_mut().entity_mut(roots[0]).insert(current_a.1);
    app.update();
    let topology = app.world().resource::<TerrainTopologyRuntime>();
    assert_eq!(topology.revision, 1);
    assert_eq!(topology.applied, holes);
    let ray =
        |surface: &TerrainMeshQuery| surface.ray(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 30.0);
    assert_eq!(ray(&topology.surfaces[0]), Some(16.0));
    assert_eq!(ray(&topology.surfaces[1]), Some(18.0));
    for (root, expected) in roots.iter().zip([16.0, 18.0]) {
        assert_eq!(
            physical_collider(app.world(), *root)
                .unwrap()
                .shape()
                .cast_local_ray(
                    &avian3d::parry::query::Ray::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y),
                    30.0,
                    false
                ),
            Some(expected)
        );
    }
    let sources = app.world().resource::<StaticTerrainRaytracingSources>();
    assert_eq!(sources.revision(), 1);
    assert_eq!(
        sources.meshes().map(|item| item.0).collect::<Vec<_>>(),
        vec![source_c, source_d]
    );
    app.update();
    assert_eq!(app.world().resource::<TerrainTopologyRuntime>().revision, 1);
    assert_eq!(
        app.world()
            .resource::<StaticTerrainRaytracingSources>()
            .revision(),
        1
    );

    // 根变换也属于几何身份；不重新烘焙局部BVH，但必须同步新的世界射线和TLAS。
    app.world_mut()
        .entity_mut(roots[0])
        .insert(GlobalTransform::from(Transform::from_xyz(0.0, 2.0, 0.0)));
    app.update();
    let topology = app.world().resource::<TerrainTopologyRuntime>();
    assert_eq!(ray(&topology.surfaces[0]), Some(14.0));
    assert_eq!(topology.revision, 2);
    assert_eq!(
        app.world()
            .resource::<StaticTerrainRaytracingSources>()
            .revision(),
        2
    );
}
