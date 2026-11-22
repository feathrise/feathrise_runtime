//! 使用正式捕获/后台任务/采用系统复验同句柄Mesh热更与真实洞底。
use super::*;
use feathrise_scene_content::{SceneId, ScenePlacementRecord};
use feathrise_scene_core::residency::SceneInstanceOwner;

fn installed(world: &World, root: Entity) -> &Collider {
    world
        .get::<Collider>(world.get::<TerrainColliderProxy>(root).unwrap().0)
        .unwrap()
}

fn wait_ready(app: &mut App, root: Entity) {
    wait_ready_within(app, root, 5);
}

fn wait_ready_within(app: &mut App, root: Entity, seconds: u64) {
    let deadline = Instant::now() + std::time::Duration::from_secs(seconds);
    loop {
        app.update();
        let ready = app.world().get::<TerrainPatchColliderReady>(root);
        let request = app
            .world()
            .get::<TerrainPatchCollisionRequest>(root)
            .unwrap();
        if ready.is_some_and(|ready| ready.matches(request)) {
            break;
        }
        assert!(Instant::now() < deadline, "有限后台碰撞未完成");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn floor(y: f32) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::MAIN_WORLD,
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [-2.0, y, -2.0],
            [-2.0, y, 2.0],
            [2.0, y, 2.0],
            [2.0, y, -2.0],
        ],
    );
    mesh.insert_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]));
    mesh
}

fn source_collision_app() -> App {
    use avian3d::prelude::PhysicsPlugins;
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, PhysicsPlugins::default()))
        .init_resource::<Assets<Mesh>>()
        .add_message::<AssetEvent<Mesh>>()
        .init_resource::<ScenePlacementRuntimeState>()
        .init_resource::<ScenePlacementRuntimeStatus>()
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(17)))
        .add_observer(discard_partition_ready)
        .add_observer(discard_partition_preparing)
        .add_systems(Update, drive_terrain_collision);
    super::super::terrain_topology_test_support::install_proxy_observers(&mut app);
    app.finish();
    app.cleanup();
    app
}

#[test]
fn source_collision_nested_batch_uses_both_publication_and_author_gates() {
    use crate::{SubsceneActive, SubsceneInstance};
    use avian3d::{collider_tree::ColliderTrees, prelude::{SpatialQuery, SpatialQueryFilter}};
    use bevy::ecs::system::RunSystemOnce;
    use feathrise_scene_content::SceneInstancePath;
    use feathrise_scene_core::residency::{InstanceResourceTable, ResidentSceneEntity, ResidentSceneToken};
    use super::super::{components::PlacementInstanceGate, resources::PlacementActiveBatch};

    for (lod, shapes) in [(None, 1), (Some(MeshLodMode::Override(default())), 2)] {
        let mut app = source_collision_app();
        app.init_resource::<InstanceAssetJournalEpoch>();
        let world = app.world_mut();
        let map = SceneId("SCENE-source-collision-outer".into());
        let resident = ResidentSceneEntity(ResidentSceneToken(37));
        let batch = world.spawn_empty().id();
        let instance = world.spawn((GlobalTransform::IDENTITY, SubsceneInstance {
            path: SceneInstancePath::root(map.clone()), source_scene_id: map.clone(), generation: 1, batch,
        })).id();
        let record: ScenePlacementRecord = serde_json::from_value(serde_json::json!({
            "id":"nested-source", "asset_path":"models/shared.glb",
            "asset_target":{"kind":"scene0","collision":false},
            "translation":[8,0,0], "yaw_degrees":0, "scale":[1,1,1]
        })).unwrap();
        let root = world.spawn((
            ScenePlacementInstance::new(&map, &record), Transform::from_xyz(8., 0., 0.),
            TerrainPatchCollisionRequest { generation: 1 }, SceneInstanceOwner(instance), resident,
            SourceSceneCollision { partitioned: true, generation: 1, ..default() },
        )).id();
        let mesh = world.resource_mut::<Assets<Mesh>>().add(floor(0.));
        for x in [0., 6.] {
            let node = world.spawn((Transform::from_xyz(x, 0., 0.), ChildOf(root),
                SourceMeshProperties { collision: Some(true), collision_lod: lod, ..default() })).id();
            world.spawn((Mesh3d(mesh.clone()), Transform::IDENTITY, ChildOf(node)));
        }
        let mut states = InstanceResourceTable::<ScenePlacementRuntimeState>::default();
        states.values.insert(instance, ScenePlacementRuntimeState {
            active: Some(PlacementActiveBatch { root, roots: vec![root], primitive_collision_roots: vec![root],
                terrain_roots: Vec::new(), gate: PlacementInstanceGate::new(true), building_preset_revision: 0,
                marker_cleanup_cursor: 0 }), ..default()
        });
        world.insert_resource(states);
        let proxy_count = |app: &App| app.world().resource::<ColliderTrees>().static_tree.proxies.len();
        let hit = |app: &mut App| app.world_mut().run_system_once(|query: SpatialQuery| {
            query.cast_ray(Vec3::new(8., 5., 0.), Dir3::NEG_Y, 10., true, &SpatialQueryFilter::default())
        }).unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while app.world().get::<TerrainColliderProxy>(root).is_none() {
            app.update();
            assert_eq!(proxy_count(&app), 0, "内层准备过程中不能登记真实代理");
            assert!(Instant::now() < deadline, "子场景碰撞未准备完毕");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let proxy = app.world().get::<TerrainColliderProxy>(root).unwrap().0;
        assert_eq!(app.world().get::<SceneInstanceOwner>(proxy), Some(&SceneInstanceOwner(instance)));
        assert_eq!(app.world().get::<ResidentSceneEntity>(proxy), Some(&resident));
        assert!(app.world().get::<SourceGlbPhysicsRoot>(proxy).is_some());
        assert!(hit(&mut app).is_none(), "内层已采用也不能提前通过原生射线");

        ScenePlacementInstance::set_editor_collision_participation(app.world_mut(), root, false);
        app.world_mut().entity_mut(batch).insert(SubsceneActive);
        set_subscene_placement_collision_activity(app.world_mut(), batch, instance);
        assert_eq!(proxy_count(&app), 0, "外层放行要继续尊重作者遮蔽");
        ScenePlacementInstance::set_editor_collision_participation(app.world_mut(), root, true);
        app.update();
        assert_eq!(proxy_count(&app), shapes);
        assert!((hit(&mut app).unwrap().distance - 5.).abs() < 1e-4);

        app.world_mut().write_message(AssetEvent::Modified { id: mesh.id() });
        app.update();
        assert!(app.world().get::<SourceSceneCollision>(root).unwrap().superseded);
        assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 1);
        assert_eq!(app.world().get::<TerrainPatchCollisionRequest>(root).unwrap().generation, 1);
        assert_eq!(proxy_count(&app), shapes, "热更应保留本批当前已采用碰撞");
        app.world_mut().entity_mut(batch).remove::<SubsceneActive>();
        set_subscene_placement_collision_activity(app.world_mut(), batch, instance);
        ScenePlacementInstance::set_editor_collision_participation(app.world_mut(), root, true);
        assert_eq!(proxy_count(&app), 0, "撤回外层后编辑器打开碰撞不能复活旧代理");
        assert!(hit(&mut app).is_none());
    }
}

#[test]
fn source_glb_node_collision_inherits_per_reference_and_can_publish_zero_shapes() {
    use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};
    use bevy::ecs::system::RunSystemOnce;
    use feathrise_scene_content::ScenePlacementAssetTarget;

    let mut app = source_collision_app();
    let (original, displayed) = {
        let mut meshes = app.world_mut().resource_mut::<Assets<Mesh>>();
        (meshes.add(floor(0.0)), meshes.add(floor(3.0)))
    };
    let mut record: ScenePlacementRecord = serde_json::from_value(serde_json::json!({
        "id":"source-mesh", "asset_path":"models/shared.glb", "translation":[0,0,0],
        "yaw_degrees":0, "scale":[1,1,1]
    })).unwrap();
    let map = SceneId("SCENE-source-mesh-collision".into());
    let mut spawn = |body_x: f32, whole_collision| {
        record.asset_target = ScenePlacementAssetTarget::Scene { collision: whole_collision };
        let pose = Transform::from_xyz(body_x, 0.0, 0.0);
        let world = app.world_mut();
        let root = world.spawn((
            ScenePlacementInstance::new(&map, &record), pose, GlobalTransform::from(pose),
            TerrainPatchCollisionRequest { generation: 1 },
            SourceSceneCollision { partitioned: true, ..SourceSceneCollision::default() },
        )).id();
        let mut nodes = Vec::new();
        for (x, collision, lod) in [(0.0, None, None), (6.0, Some(false), None), (12.0, Some(true), Some(MeshLodMode::Disabled))] {
            let pose = Transform::from_xyz(x, 0.0, 0.0);
            let node = world.spawn((
                SourceMeshProperties { collision, collision_lod: lod, ..default() },
                ChildOf(root), pose, GlobalTransform::from_translation(Vec3::new(body_x + x, 0.0, 0.0)),
            )).id();
            world.spawn((Mesh3d(displayed.clone()), StaticMeshLodOriginal { mesh: original.clone() }, ChildOf(node), Transform::IDENTITY));
            nodes.push(node);
        }
        (root, nodes)
    };
    let (off, off_nodes) = spawn(0.0, false);
    let (on, _) = spawn(30.0, true);
    let scoped_root = app.world_mut().spawn(Transform::from_xyz(200.0, 0.0, 0.0)).id();
    app.world_mut().entity_mut(on).insert(SceneInstanceOwner(scoped_root));
    let wait_for_generation = |app: &mut App, root: Entity, generation| {
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while !app.world().get::<PartitionColliderReady>(root).is_some_and(|ready| ready.generation == generation)
            && !app.world().get::<TerrainPatchColliderReady>(root).is_some_and(|ready| ready.generation == generation)
        {
            app.update();
            assert!(Instant::now() < deadline, "分组碰撞未完成");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        app.update(); // 物理步读取同代已采用的代理和姿态。
    };
    let ground_hit = |app: &mut App, x: f32| {
        app.world_mut().run_system_once(move |query: SpatialQuery, active: feathrise_grounding::GroundSurfaceFilter| {
            query.cast_ray_predicate(Vec3::new(x, 10.0, 0.0), Dir3::NEG_Y, 20.0, true,
                &SpatialQueryFilter::default(), &|entity| active.is_active(entity)).map(|hit| {
                    assert!((hit.distance - 10.0).abs() < 1e-4); // 显示派生不能影响真实物理位置。
                    hit.entity
                })
        }).unwrap()
    };
    wait_for_generation(&mut app, off, 1);
    wait_for_generation(&mut app, on, 1);
    let body = app.world().get::<TerrainColliderProxy>(on).unwrap().0;
    assert_eq!(app.world().get::<SceneInstanceOwner>(body).unwrap().0, scoped_root);
    assert!(app.world().get::<ChildOf>(body).is_none());
    for (x, expected) in [(0.0, false), (6.0, false), (12.0, true), (30.0, true), (36.0, false), (42.0, true)] {
        assert_eq!(ground_hit(&mut app, x).is_some(), expected, "x={x}");
    }
    let inherited = ground_hit(&mut app, 30.0).unwrap();
    let explicit = ground_hit(&mut app, 42.0).unwrap();
    assert_ne!(inherited, explicit);
    assert_eq!(app.world().get::<PlacementColliderOwner>(explicit).unwrap().lod, Some(MeshLodMode::Disabled));
    let old = ground_hit(&mut app, 12.0).unwrap();
    app.world_mut().get_mut::<SourceMeshProperties>(off_nodes[2]).unwrap().collision = Some(false);
    app.world_mut().get_mut::<TerrainPatchCollisionRequest>(off).unwrap().generation += 1;
    app.update();
    assert_eq!(ground_hit(&mut app, 12.0), Some(old)); // 旧完整物理保留到新结果发布。
    wait_for_generation(&mut app, off, 2);
    assert!(app.world().get::<TerrainColliderProxy>(off).is_none());
    assert!(ground_hit(&mut app, 12.0).is_none());
    assert_eq!(ground_hit(&mut app, 42.0), Some(explicit));
}

#[test]
fn source_collision_batch_uses_only_the_proxies_its_lod_policy_requires() {
    use avian3d::{collider_tree::ColliderTrees, prelude::{ContactGraph, GravityScale, SpatialQuery, SpatialQueryFilter}};
    use bevy::ecs::system::RunSystemOnce;

    const NODES: usize = 96; // 多于两次现有代理登记预算，分别覆盖共享、不宜合并和独立档位。
    let cases = [
        (None, 6.0, 1),
        (Some(MeshLodMode::Disabled), 6.0, 1),
        (Some(MeshLodMode::Disabled), 4.0, NODES), // 边界接触时保留各自法线。
        (Some(MeshLodMode::Override(default())), 6.0, NODES),
    ];
    for (lod, spacing, expected_proxies) in cases {
        let mut app = source_collision_app();
        let floor = app.world_mut().resource_mut::<Assets<Mesh>>().add(floor(0.0));
        let record: ScenePlacementRecord = serde_json::from_value(serde_json::json!({
            "id":"many-source-meshes", "asset_path":"models/shared.glb",
            "asset_target":{"kind":"scene0","collision":false},
            "translation":[0,0,0], "yaw_degrees":0, "scale":[1,1,1]
        })).unwrap();
        let world = app.world_mut();
        let root = world.spawn((
            ScenePlacementInstance::new(&SceneId("SCENE-source-collision-batch".into()), &record),
            Transform::IDENTITY, DeferredTerrainPreviewCollider,
            TerrainPatchCollisionRequest { generation: 1 },
            SourceSceneCollision { partitioned: true, ..default() },
        )).id();
        for i in 0..NODES {
            let node = world.spawn((
                SourceMeshProperties { collision: Some(true), collision_lod: lod, ..default() },
                Transform::from_xyz(i as f32 * spacing, 0.0, 0.0), ChildOf(root),
            )).id();
            world.spawn((Mesh3d(floor.clone()), ChildOf(node), Transform::IDENTITY));
        }
        let xs = [0.0, (NODES - 1) as f32 * spacing];
        let probes = xs.map(|x| world.spawn((
            RigidBody::Dynamic, GravityScale(0.0), Collider::cuboid(0.4, 0.4, 0.4),
            Transform::from_xyz(x, 0.18, 0.0),
        )).id());
        let contacts = |app: &App| {
            let graph = app.world().resource::<ContactGraph>();
            probes.map(|probe| graph.contact_pairs_with(probe).filter(|pair| pair.is_touching())
                .fold((0, 0), |(touching, supporting), pair| (touching + 1, supporting + usize::from(pair.generates_constraints()))))
        };
        let ray_hits = |app: &mut App| app.world_mut().run_system_once(move |query: SpatialQuery| {
            xs.map(|x| query.cast_ray(Vec3::new(x, 5.0, 1.0), Dir3::NEG_Y, 10.0, true, &SpatialQueryFilter::default()).map(|hit| hit.entity))
        }).unwrap();
        let static_proxies = |app: &App| app.world().resource::<ColliderTrees>().static_tree.proxies.len();
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.update();
            assert_eq!(contacts(&app), [(0, 0); 2], "准备中的批次不能产生接触");
            assert_eq!(static_proxies(&app), 0, "候选还未进入原生空间查询树");
            if app.world().get::<TerrainPatchColliderReady>(root).is_some()
                || app.world().get::<PartitionColliderReady>(root).is_some()
            { break; }
            assert!(Instant::now() < deadline, "源 GLB 碰撞未完成");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(ray_hits(&mut app), [None, None]);
        let started = Instant::now();
        app.world_mut().entity_mut(root).insert(InstallTerrainPatchCollider);
        app.world_mut().flush();
        let install = started.elapsed();
        app.update();
        let frame = started.elapsed();
        let registered = static_proxies(&app);
        eprintln!("源碰撞 {NODES} 节点，档位 {lod:?}，间距 {spacing}，树代理 {registered}，采用 {install:?}，含物理帧 {frame:?}");
        assert_eq!(registered, expected_proxies);
        assert_eq!(contacts(&app), [(1, 1); 2], "采用后的同一物理帧必须包含首末支撑且无叠加");
        let [first, last] = ray_hits(&mut app).map(|hit| hit.expect("首末原形均可命中"));
        assert_eq!(first == last, expected_proxies == 1);
        assert_eq!(app.world().get::<PlacementColliderOwner>(first).unwrap().lod, lod);
        if matches!(lod, Some(MeshLodMode::Override(_))) {
            let old_body = app.world().get::<TerrainColliderProxy>(root).unwrap().0;
            app.world_mut().get_mut::<TerrainPatchCollisionRequest>(root).unwrap().generation += 1;
            let deadline = Instant::now() + std::time::Duration::from_secs(5);
            while app.world().get::<PartitionColliderReady>(root).is_none_or(|ready| ready.generation != 2) {
                app.update();
                assert_eq!(static_proxies(&app), NODES);
                assert_eq!(ray_hits(&mut app), [Some(first), Some(last)]); // 热更准备中仍只能看见旧一代。
                assert!(Instant::now() < deadline, "替代碰撞未完成");
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let started = Instant::now();
            app.world_mut().entity_mut(root).insert(InstallTerrainPatchCollider);
            app.world_mut().flush();
            let install = started.elapsed();
            assert_eq!(static_proxies(&app), NODES, "同一采用命令必须撤回旧树代理，避免双份碰撞");
            assert_eq!(app.world().get::<Children>(old_body).unwrap().len(), NODES);
            app.update();
            eprintln!("源碰撞 {NODES} 独立节点热替换，采用 {install:?}，含物理帧 {:?}", started.elapsed());
            let remaining = app.world().get::<Children>(old_body).unwrap().len();
            assert!(remaining > 0 && remaining < NODES, "已经离开物理树的旧实体应分帧回收");
            assert_eq!(static_proxies(&app), NODES);
            assert!(ray_hits(&mut app).into_iter().zip([first, last]).all(|(hit, old)| hit.is_some_and(|hit| hit != old)));
        }
    }
}

#[test]
fn dense_scene0_cooks_and_installs_exact_collision_across_frames() {
    // 527076点、1051250三角，加上多节点和多个子网格，覆盖原先拦截完整GLB的各项限制。
    const CELLS: u32 = 725;
    let mut mesh = floor(-3.0);
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        (0..=CELLS)
            .flat_map(|z| (0..=CELLS).map(move |x| [x as f32, -3.0, z as f32]))
            .collect::<Vec<_>>(),
    );
    let mut indices = Vec::new();
    for z in 0..CELLS {
        for x in 0..CELLS {
            let a = z * (CELLS + 1) + x;
            let b = a + CELLS + 1;
            indices.extend([a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    mesh.insert_indices(Indices::U32(indices));
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<Assets<Mesh>>()
        .add_message::<AssetEvent<Mesh>>()
        .init_resource::<ScenePlacementRuntimeState>()
        .init_resource::<ScenePlacementRuntimeStatus>()
        .add_systems(Update, drive_terrain_collision);
    super::super::terrain_topology_test_support::install_proxy_observers(&mut app);
    let record: ScenePlacementRecord = serde_json::from_value(serde_json::json!({
        "id":"dense-model","asset_path":"models/dense.glb",
        "asset_target":{"kind":"scene0","collision":true},
        "translation":[0,0,0],"yaw_degrees":0,"scale":[1,1,1]
    })).unwrap();
    let root = app.world_mut().spawn((
        ScenePlacementInstance::new(&SceneId("SCENE-dense-model".into()), &record),
        terrain_placement_collision(&record).unwrap(),
        Transform::IDENTITY,
        GlobalTransform::IDENTITY,
        DeferredTerrainPreviewCollider,
    )).id();
    let (main, extra) = {
        let mut meshes = app.world_mut().resource_mut::<Assets<Mesh>>();
        (meshes.add(mesh), meshes.add(floor(-6.0)))
    };
    app.world_mut().spawn((Mesh3d(main), GlobalTransform::IDENTITY, ChildOf(root)));
    for offset in 0..MAX_MESHES {
        let x = -5.0 * (offset + 1) as f32;
        app.world_mut().spawn((Mesh3d(extra.clone()),
            GlobalTransform::from(Transform::from_xyz(x, 0.0, -6.0)), ChildOf(root)));
    }
    for _ in 0..1024 {
        app.world_mut().spawn(ChildOf(root));
    }

    // 先分帧读取所有子节点，再进入第一个密集网格的复制。
    for _ in 0..32 {
        app.update();
        let runtime = app.world().resource::<TerrainCollisionRuntime>();
        if runtime.capture.as_ref().is_some_and(|capture| capture.current.is_some()) {
            break;
        }
    }
    let runtime = app.world().resource::<TerrainCollisionRuntime>();
    let current = runtime.capture.as_ref().unwrap().current.as_ref().unwrap();
    assert_eq!(current.mesh.vertices.len(), COPY_BYTES / 12);
    assert!(current.mesh.indices.is_empty());
    assert!(runtime.started.is_none()); // 已持续推进的普通GLB不受旧地形的累计超时限制。
    wait_ready_within(&mut app, root, 30);
    assert!(app.world().get::<TerrainColliderProxy>(root).is_none());
    app.world_mut().entity_mut(root).insert(InstallTerrainPatchCollider);
    app.update();
    let shape = installed(app.world(), root).shape();
    let main_ray = avian3d::parry::query::Ray::new(Vec3::new(500.25, 10.0, 501.5), Vec3::NEG_Y);
    let extra_ray = avian3d::parry::query::Ray::new(Vec3::new(-5.0, 10.0, -6.0), Vec3::NEG_Y);
    assert_eq!(shape.cast_local_ray(&main_ray, 30.0, false), Some(13.0));
    assert_eq!(shape.cast_local_ray(&extra_ray, 30.0, false), Some(16.0));
}

#[test]
fn volume_stream_chunked_upload_captures_more_than_legacy_mesh_vertex_and_triangle_totals() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin))
        .init_resource::<Assets<Mesh>>()
        .add_message::<AssetEvent<Mesh>>()
        .init_resource::<ScenePlacementRuntimeState>()
        .init_resource::<ScenePlacementRuntimeStatus>()
        .add_systems(Update, drive_terrain_collision);
    super::super::terrain_topology_test_support::install_proxy_observers(&mut app);
    super::super::terrain_mesh_upload::install(&mut app);
    let record: ScenePlacementRecord = serde_json::from_value(serde_json::json!({
        "id":"large-cave","asset_path":"scenes/test/volume_sculpt/v-test.glb",
        "translation":[7,5,9],"yaw_degrees":0,"scale":[1,1,1],
        "terrain_patch":{"sculpted_volume":true,"streamed_volume":true,"volume_cell_size":0.25,"cutouts":[{"min":[0,0],"max":[256,4]}]}
    })).unwrap();
    let mut chunks = Vec::new();
    for part in 0..64 {
        let mut mesh = floor(-3.);
        let positions: Vec<_> = (0..=64)
            .flat_map(|z| {
                (0..=64).map(move |x| [part as f32 * 4. + x as f32 / 16., -3., z as f32 / 16.])
            })
            .collect();
        let mut indices = Vec::new();
        for z in 0..64u32 {
            for x in 0..64u32 {
                let a = z * 65 + x;
                indices.extend([a, a + 65, a + 1, a + 1, a + 65, a + 66]);
            }
        }
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_indices(Indices::U32(indices));
        chunks.push(mesh);
    }
    assert!(chunks.len() > MAX_MESHES);
    assert!(chunks.iter().map(Mesh::count_vertices).sum::<usize>() > MAX_VERTICES);
    assert!(
        chunks
            .iter()
            .map(|m| m.indices().unwrap().len() / 3)
            .sum::<usize>()
            > MAX_TRIANGLES
    );
    let root = app
        .world_mut()
        .spawn((
            ScenePlacementInstance::new(&SceneId("MAP-large-cave".into()), &record),
            terrain_placement_collision(&record).unwrap(),
            Transform::from_translation(Vec3::from_array(record.translation)),
            Visibility::Hidden,
            DeferredTerrainPreviewCollider,
            super::super::TerrainMeshUpload::new(chunks, Handle::default()),
        ))
        .id();
    for frame in 0..64 {
        app.update();
        assert_eq!(app.world().resource::<Assets<Mesh>>().len(), frame + 1);
        assert!(app.world().get::<TerrainPatchColliderReady>(root).is_none());
        assert_eq!(
            app.world().get::<Visibility>(root),
            Some(&Visibility::Hidden)
        );
    }
    app.update();
    if let Some((entity, generation, _)) = app.world().resource::<TerrainCollisionRuntime>().started
    {
        app.world_mut()
            .resource_mut::<TerrainCollisionRuntime>()
            .started = Some((
            entity,
            generation,
            Instant::now() - std::time::Duration::from_secs(31),
        ));
    }
    wait_ready(&mut app, root);
    let ready = app.world().get::<TerrainPatchColliderReady>(root).unwrap();
    assert_eq!(ready.meshes.len(), 64);
    for x in [1., 253.] {
        let ray = avian3d::parry::query::Ray::new(Vec3::new(x, 10., 1.), Vec3::NEG_Y);
        assert_eq!(
            ready.collider.shape().cast_local_ray(&ray, 30., false),
            Some(13.)
        );
    }
    assert!(
        app.world().get::<TerrainColliderProxy>(root).is_none(),
        "隐藏候选不能提前安装真实物理"
    );
    assert!(
        app.world()
            .resource::<TerrainCollisionRuntime>()
            .failed
            .is_empty()
    );
}

#[test]
fn volume_stream_invalid_geometry_reports_terminal_failure_without_installing_partial_shape() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<Assets<Mesh>>()
        .add_message::<AssetEvent<Mesh>>()
        .init_resource::<ScenePlacementRuntimeState>()
        .init_resource::<ScenePlacementRuntimeStatus>()
        .add_systems(Update, drive_terrain_collision);
    super::super::terrain_topology_test_support::install_proxy_observers(&mut app);
    let record: ScenePlacementRecord = serde_json::from_value(serde_json::json!({
        "id":"broken-cave","asset_path":"scenes/test/volume_sculpt/v-test.glb",
        "translation":[0,0,0],"yaw_degrees":0,"scale":[1,1,1],
        "terrain_patch":{"sculpted_volume":true,"streamed_volume":true,"volume_cell_size":0.25,"cutouts":[{"min":[0,0],"max":[4,4]}]}
    })).unwrap();
    let root = app
        .world_mut()
        .spawn((
            ScenePlacementInstance::new(&SceneId("MAP-test".into()), &record),
            terrain_placement_collision(&record).unwrap(),
            GlobalTransform::IDENTITY,
            Visibility::Hidden,
            DeferredTerrainPreviewCollider,
        ))
        .id();
    let mut mesh = floor(-3.);
    mesh.insert_indices(Indices::U32(vec![0, 1, 9]));
    let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
    app.world_mut()
        .spawn((Mesh3d(mesh), GlobalTransform::IDENTITY, ChildOf(root)));
    for _ in 0..1000 {
        app.update();
        if app
            .world()
            .get::<TerrainPatchColliderFailure>(root)
            .is_some()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        app.world()
            .get::<TerrainPatchColliderFailure>(root)
            .unwrap()
            .detail()
            .contains("越界索引")
    );
    assert!(app.world().get::<TerrainPatchColliderReady>(root).is_none());
    assert!(app.world().get::<TerrainColliderProxy>(root).is_none());
    assert_eq!(
        app.world().get::<Visibility>(root),
        Some(&Visibility::Hidden)
    );
}

#[test]
fn garbage_keeps_independent_scaled_bvhs_even_when_raw_shape_is_shared() {
    let raw = Collider::try_trimesh_with_config(
        vec![Vec3::ZERO, Vec3::X, Vec3::Z],
        vec![[0, 2, 1]],
        TrimeshFlags::FIX_INTERNAL_EDGES,
    )
    .unwrap();
    let mut scaled = raw.clone();
    scaled.set_scale(Vec3::new(2.0, 3.0, 4.0), 10);
    let mut garbage = TerrainCollisionGarbage::default();
    garbage.retain_shape(&raw);
    garbage.retain_shape(&scaled);
    garbage.retain_shape(&scaled);
    assert_eq!(garbage.shapes.len(), 2); // 原始查询与缩放物理必须各留一份，重复保留仍去重。
}

#[test]
fn cooked_scaled_collider_installs_without_rebuilding_scaled_bvh() {
    let scale = Vec3::new(2.0, 3.0, 4.0);
    let cooked = cook(Capture {
        root: Entity::PLACEHOLDER,
        generation: 7,
        inverse_root: Mat4::from_scale(scale).inverse(),
        scale,
        stack: VecDeque::new(),
        current: None,
        meshes: vec![CapturedMesh {
            vertices: vec![
                Vec3::new(-2.0, -1.0, -2.0),
                Vec3::new(-2.0, -1.0, 2.0),
                Vec3::new(2.0, -1.0, 2.0),
                Vec3::new(2.0, -1.0, -2.0),
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            matrix: Mat4::IDENTITY,
            group: None,
        }],
        sources: HashSet::new(),
        mesh_entities: Vec::new(),
        nodes: 1,
        vertices: 4,
        triangles: 2,
        scene_collision: false,
        root_collision: false,
        partitioned: false,
        excluded_mesh: false,
        streamed: false,
        child_cursor: None,
    });
    assert_eq!(cooked.scale, scale);
    let CookedShape::Single { collider: query, physical: mut collider, .. } = cooked.shape.unwrap() else {
        panic!("单体形状")
    };
    assert_eq!(collider.scale(), Vec3::ONE);
    let prepared = collider.shape_scaled().clone();
    collider.set_scale(Vec3::ONE, 10); // 独立物理实体应用单位GlobalTransform；必须直接命中准备缓存。
    assert!(std::ptr::eq(
        prepared.as_ref(),
        collider.shape_scaled().as_ref()
    ));
    let ray = avian3d::parry::query::Ray::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y);
    assert_eq!(query.shape().cast_local_ray(&ray, 30.0, false), Some(11.0));
    assert_eq!(
        collider.shape_scaled().cast_local_ray(&ray, 30.0, false),
        Some(13.0)
    );
}

#[test]
fn physical_proxy_keeps_unit_scale_and_owned_pose_with_real_physics_hooks() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        avian3d::prelude::PhysicsPlugins::default(),
    ))
    .init_resource::<Assets<Mesh>>()
    .add_message::<AssetEvent<Mesh>>()
    .add_systems(Update, sync_preview_physics_pose);
    super::super::terrain_topology_test_support::install_proxy_observers(&mut app);
    app.finish();
    app.cleanup();
    let pose = Transform::from_xyz(7.0, 5.0, 9.0).with_rotation(Quat::from_rotation_y(0.7));
    let global = GlobalTransform::from(Transform {
        scale: Vec3::new(2.0, 3.0, 4.0),
        ..pose
    });
    let mesh = avian3d::parry::shape::TriMesh::new(
        vec![
            Vec3::new(-4.0, 0.0, -8.0),
            Vec3::new(-4.0, 0.0, 8.0),
            Vec3::new(4.0, 0.0, 8.0),
            Vec3::new(4.0, 0.0, -8.0),
        ],
        vec![[0, 1, 2], [0, 2, 3]],
    )
    .unwrap();
    let physical = Collider::from(avian3d::parry::shape::SharedShape::new(
        static_shape::StaticTerrainShape::new(mesh),
    ));
    let prepared = physical.shape_scaled().clone();
    let record: ScenePlacementRecord = serde_json::from_str(
        r#"{"id":"cave-preview","asset_path":"scenes/cave.glb","translation":[7,5,9],"yaw_degrees":0,"scale":[2,3,4],"terrain_patch":{}}"#,
    )
    .unwrap();
    let root = app
        .world_mut()
        .spawn((
            ScenePlacementInstance::new(&SceneId("MAP-test-cave".into()), &record),
            global,
            TerrainPatchCollisionRequest { generation: 1 },
            TerrainPatchColliderReady {
                collider: physical.clone(),
                physical,
                lod: None,
                scale: global.scale(),
                generation: 1,
                meshes: std::sync::Arc::default(),
                surface_ready: true,
            },
        ))
        .id();
    app.world_mut().flush();
    app.update();
    let proxy = app.world().get::<TerrainColliderProxy>(root).unwrap().0;
    let collider = app.world().get::<Collider>(proxy).unwrap();
    assert_eq!(collider.scale(), Vec3::ONE);
    assert!(std::ptr::eq(
        prepared.as_ref(),
        collider.shape_scaled().as_ref()
    ));
    assert_eq!(
        app.world()
            .get::<avian3d::prelude::Position>(proxy)
            .unwrap()
            .0,
        pose.translation
    );
    assert!(
        app.world()
            .get::<avian3d::prelude::Rotation>(proxy)
            .unwrap()
            .0
            .abs_diff_eq(pose.rotation, 0.00001)
    );
    assert_eq!(
        app.world().get::<Transform>(proxy).unwrap().scale,
        Vec3::ONE
    );
    let moved_pose =
        Transform::from_xyz(-11.0, 3.0, 4.0).with_rotation(Quat::from_rotation_y(-0.7));
    app.world_mut()
        .entity_mut(root)
        .insert(GlobalTransform::from(Transform {
            scale: Vec3::new(2.0, 3.0, 4.0),
            ..moved_pose
        }));
    app.update();
    assert_eq!(
        app.world().get::<TerrainColliderProxy>(root).unwrap().0,
        proxy
    );
    assert_eq!(
        app.world()
            .get::<avian3d::prelude::Position>(proxy)
            .unwrap()
            .0,
        moved_pose.translation
    );
    assert!(
        app.world()
            .get::<avian3d::prelude::Rotation>(proxy)
            .unwrap()
            .0
            .abs_diff_eq(moved_pose.rotation, 0.00001)
    );
    assert!(std::ptr::eq(
        prepared.as_ref(),
        installed(app.world(), root).shape_scaled().as_ref()
    ));
    // 请求换代期间不得用旧Ready授信新位姿，也不能误删上一完整支撑。
    app.world_mut()
        .get_mut::<TerrainPatchCollisionRequest>(root)
        .unwrap()
        .generation += 1;
    app.world_mut()
        .entity_mut(root)
        .insert(GlobalTransform::from(Transform {
            translation: Vec3::splat(50.0),
            scale: Vec3::new(2.0, 3.0, 4.0),
            ..moved_pose
        }));
    app.update();
    assert_eq!(
        app.world()
            .get::<avian3d::prelude::Position>(proxy)
            .unwrap()
            .0,
        moved_pose.translation
    );
    app.world_mut().despawn(root);
    app.world_mut().flush();
    assert!(app.world().get_entity(proxy).is_err()); // 所有权根删除时独立物理不能遗留。
}

#[test]
fn same_handle_hot_reload_rejects_old_readiness_and_keeps_previous_collider_until_cooked() {
    check_same_handle_hot_reload(false, false);
}

#[test]
fn formal_hot_reload_keeps_old_physics_after_cooking_until_joint_install() {
    check_same_handle_hot_reload(true, false);
}

#[test]
fn scene0_collision_hot_reload_updates_the_active_static_proxy() {
    check_same_handle_hot_reload(true, true);
}

#[test]
fn scene0_collision_registers_beyond_old_limit_and_rejects_stale_pending_cook() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<Assets<Mesh>>()
        .add_message::<AssetEvent<Mesh>>()
        .init_resource::<WorldAssetLifecycleIndex>()
        .init_resource::<ScenePlacementRuntimeState>()
        .init_resource::<ScenePlacementRuntimeStatus>()
        .add_systems(Update, drive_terrain_collision);
    super::super::terrain_topology_test_support::install_proxy_observers(&mut app);

    let scene = SceneId("SCENE-many-colliders".into());
    let mut record: ScenePlacementRecord = serde_json::from_value(serde_json::json!({
        "id":"shared","asset_path":"models/shared.glb",
        "asset_target":{"kind":"scene0","collision":true},
        "translation":[0,0,0],"yaw_degrees":0,"scale":[1,1,1]
    })).unwrap();
    let (shared, edited) = {
        let mut meshes = app.world_mut().resource_mut::<Assets<Mesh>>();
        (meshes.add(floor(-3.0)), meshes.add(floor(-2.0)))
    };
    let old_roots: Vec<_> = (0..128).map(|index| {
        record.id = format!("shared-{index}");
        let root = app.world_mut().spawn((
            ScenePlacementInstance::new(&scene, &record),
            terrain_placement_collision(&record).unwrap(),
            GlobalTransform::IDENTITY,
        )).id();
        app.world_mut().spawn((Mesh3d(shared.clone()), GlobalTransform::IDENTITY, ChildOf(root)));
        root
    }).collect();
    for &root in &old_roots {
        wait_ready_within(&mut app, root, 15);
        app.world_mut().entity_mut(root).insert(FormalTerrainPatchCollider);
    }

    let old_root = old_roots[0];
    let old_generation = app.world().get::<TerrainPatchCollisionRequest>(old_root).unwrap().generation;
    for _ in 0..(ROOTS_PER_FRAME + 1) {
        app.world_mut().write_message(AssetEvent::<Mesh>::Added { id: edited.id() });
    }
    app.update();
    assert_eq!(app.world().get::<TerrainPatchCollisionRequest>(old_root).unwrap().generation, old_generation);

    // 保留旧活动根，隐藏候选同时登记。候选烘焙期间换代，旧结果不能取得Ready或进入物理世界。
    let container = app.world_mut().spawn_empty().id();
    let mut state = super::super::terrain_topology_test_support::closing_candidate(scene.clone(), container);
    let staging = state.staging.as_mut().unwrap();
    record.id = "new-candidate".to_owned();
    let world = Handle::<bevy::world_serialization::WorldAsset>::default();
    let world_id = world.id();
    let candidate = app.world_mut().spawn((
        ScenePlacementInstance::new_pending(&scene, &record, staging.gate.clone()),
        PlacementPendingRevision { generation: staging.generation, revision: staging.revision },
        terrain_placement_collision(&record).unwrap(),
        GlobalTransform::IDENTITY,
        WorldAssetRoot(world),
        TerrainWorldReady,
    )).id();
    app.world_mut().spawn((Mesh3d(edited.clone()), GlobalTransform::IDENTITY, ChildOf(candidate)));
    let world_epoch = app.world_mut().resource_mut::<WorldAssetLifecycleIndex>().mark_world_ready(candidate, world_id);
    staging.primitive_collision_roots.push(candidate);
    staging.roots.push(candidate);
    staging.total_instances = 1;
    app.insert_resource(state);

    for _ in 0..2 {
        app.update();
        if app.world().resource::<TerrainCollisionRuntime>().task.is_some() {
            break;
        }
    }
    let runtime = app.world().resource::<TerrainCollisionRuntime>();
    assert_eq!(runtime.registered.len(), old_roots.len() + 1);
    assert!(runtime.task.is_some());
    app.world_mut().resource_mut::<Assets<Mesh>>().insert(edited.id(), floor(-6.0)).unwrap();
    app.world_mut().write_message(AssetEvent::<Mesh>::Modified { id: edited.id() });
    app.update();
    assert!(!app.world().resource::<ScenePlacementRuntimeState>().collision_mesh_events_caught_up);
    assert!(app.world().get::<TerrainPatchColliderReady>(candidate).is_none());
    assert!(app.world().get::<TerrainColliderProxy>(candidate).is_none());

    wait_ready_within(&mut app, candidate, 15);
    assert_eq!(app.world().get::<PlacementWorldInstanceReady>(candidate).unwrap().0, world_epoch);
    let ray = avian3d::parry::query::Ray::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y);
    let cooked = app.world().get::<TerrainPatchColliderReady>(candidate).unwrap();
    assert_eq!(cooked.collider.shape().cast_local_ray(&ray, 30.0, false), Some(16.0));
    assert!(app.world().get::<TerrainColliderProxy>(candidate).is_none());
    assert_eq!(installed(app.world(), old_root).shape().cast_local_ray(&ray, 30.0, false), Some(13.0));
}

fn check_same_handle_hot_reload(formal: bool, scene_collision: bool) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<Assets<Mesh>>()
        .add_message::<AssetEvent<Mesh>>()
        .init_resource::<TerrainCollisionRuntime>()
        .init_resource::<ScenePlacementRuntimeState>()
        .init_resource::<ScenePlacementRuntimeStatus>()
        .init_resource::<TerrainCollisionGarbage>()
        .add_observer(retire_ready_shape)
        .add_observer(retire_installed_shape)
        .add_observer(retire_physics_proxy)
        .add_observer(register_request)
        .add_observer(unregister_request)
        .add_observer(install_ready_collider)
        .add_systems(Update, drive_terrain_collision);
    let mut record: ScenePlacementRecord = serde_json::from_str(
        r#"{"id":"cave","asset_path":"scenes/cave.glb","translation":[0,0,0],"yaw_degrees":0,"scale":[1,1,1],"terrain_patch":{}}"#,
    ).unwrap();
    if scene_collision {
        record.terrain_patch = None;
        assert!(terrain_placement_collision(&record).is_none()); // 旧完整 GLB 保持无隐式碰撞。
        record.asset_target = feathrise_scene_content::ScenePlacementAssetTarget::Scene { collision: true };
    }
    let root = app
        .world_mut()
        .spawn((
            ScenePlacementInstance::new(&SceneId("MAP-test-cave".into()), &record),
            terrain_placement_collision(&record).unwrap(),
            Transform::default(),
            GlobalTransform::default(),
        ))
        .id();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(floor(-3.0));
    let source = app
        .world_mut()
        .spawn((
            Mesh3d(handle.clone()),
            Transform::default(),
            GlobalTransform::default(),
            ChildOf(root),
        ))
        .id();
    // 同一GLB的第二个primitive有独立节点位姿，合并后索引偏移与局部变换都须正确。
    app.world_mut().spawn((
        Mesh3d(handle.clone()),
        Transform::from_xyz(10.0, 5.0, 0.0),
        GlobalTransform::from(Transform::from_xyz(10.0, 5.0, 0.0)),
        ChildOf(root),
    ));
    wait_ready(&mut app, root);
    if formal {
        app.world_mut()
            .entity_mut(root)
            .insert(FormalTerrainPatchCollider);
    }
    assert_eq!(
        app.world()
            .get::<TerrainPatchColliderReady>(root)
            .unwrap()
            .meshes
            .len(),
        2
    );
    assert_eq!(
        app.world()
            .get::<TerrainPatchColliderReady>(root)
            .unwrap()
            .meshes[0]
            .0,
        source
    );
    let ray = avian3d::parry::query::Ray::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y);
    let second_ray = avian3d::parry::query::Ray::new(Vec3::new(10.0, 10.0, 0.0), Vec3::NEG_Y);
    assert_eq!(
        installed(app.world(), root)
            .shape()
            .cast_local_ray(&second_ray, 30.0, false),
        Some(8.0)
    );
    assert_eq!(
        installed(app.world(), root)
            .shape()
            .cast_local_ray(&ray, 30.0, false),
        Some(13.0)
    );
    app.world_mut()
        .resource_mut::<Assets<Mesh>>()
        .insert(handle.id(), floor(-6.0))
        .unwrap();
    app.world_mut()
        .write_message(AssetEvent::<Mesh>::Modified { id: handle.id() });
    app.update();
    let ready = app.world().get::<TerrainPatchColliderReady>(root).unwrap();
    assert!(
        !ready.matches(
            app.world()
                .get::<TerrainPatchCollisionRequest>(root)
                .unwrap()
        )
    );
    assert_eq!(
        installed(app.world(), root)
            .shape()
            .cast_local_ray(&ray, 30.0, false),
        Some(13.0)
    );
    wait_ready(&mut app, root);
    if formal && !scene_collision {
        // 地形模型烘焙完成只产生Ready；旧物理一直保留到整批拓扑采用。
        assert_eq!(
            installed(app.world(), root)
                .shape()
                .cast_local_ray(&ray, 30.0, false),
            Some(13.0)
        );
        assert_eq!(
            app.world()
                .get::<TerrainPatchColliderReady>(root)
                .unwrap()
                .collider
                .shape()
                .cast_local_ray(&ray, 30.0, false),
            Some(16.0)
        );
        app.world_mut()
            .entity_mut(root)
            .insert(InstallTerrainPatchCollider);
        app.update();
    }
    assert_eq!(
        installed(app.world(), root)
            .shape()
            .cast_local_ray(&second_ray, 30.0, false),
        Some(11.0)
    );
    assert_eq!(
        installed(app.world(), root)
            .shape()
            .cast_local_ray(&ray, 30.0, false),
        Some(16.0)
    );
    app.world_mut().despawn(root);
    app.update();
    assert!(
        app.world()
            .resource::<TerrainCollisionRuntime>()
            .registered
            .is_empty()
    );
}
