//! 使用正式捕获/后台任务/采用系统复验同句柄Mesh热更与真实洞底。
use super::*;
use feathrise_scene_content::{SceneId, ScenePlacementRecord};

fn installed(world: &World, root: Entity) -> &Collider {
    world
        .get::<Collider>(world.get::<TerrainColliderProxy>(root).unwrap().0)
        .unwrap()
}

fn wait_ready(app: &mut App, root: Entity) {
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
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
    assert_eq!(garbage.0.len(), 2); // 原始查询与缩放物理必须各留一份，重复保留仍去重。
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
        meshes: vec![(
            vec![
                Vec3::new(-2.0, -1.0, -2.0),
                Vec3::new(-2.0, -1.0, 2.0),
                Vec3::new(2.0, -1.0, 2.0),
                Vec3::new(2.0, -1.0, -2.0),
            ],
            vec![0, 1, 2, 0, 2, 3],
        )],
        sources: Vec::new(),
        matrices: vec![Mat4::IDENTITY],
        mesh_entities: Vec::new(),
        nodes: 1,
        vertices: 4,
        triangles: 2,
        streamed: false,
        child_cursor: None,
    });
    assert_eq!(cooked.scale, scale);
    let (query, mut collider) = cooked.shape.unwrap();
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
    check_same_handle_hot_reload(false);
}

#[test]
fn formal_hot_reload_keeps_old_physics_after_cooking_until_joint_install() {
    check_same_handle_hot_reload(true);
}

fn check_same_handle_hot_reload(formal: bool) {
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
    let record:ScenePlacementRecord=serde_json::from_str(r#"{"id":"cave","asset_path":"scenes/cave.glb","translation":[0,0,0],"yaw_degrees":0,"scale":[1,1,1],"terrain_patch":{}}"#).unwrap();
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
    if formal {
        // 正式模型烘焙完成只产生Ready；旧物理一直保留到整批拓扑采用。
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
