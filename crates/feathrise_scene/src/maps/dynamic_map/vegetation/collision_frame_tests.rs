//! 近场代理使用世界米制距离，跟随源网格平移旋转缩放，玩家静止时姿态变化也须核对。

use super::*;
use bevy::ecs::system::RunSystemOnce;
use feathrise_map_content::{ResolvedVegetationPresetCatalog, VegetationPresetCatalogAsset};
use std::sync::Arc;
use super::super::resources::{DynamicMeshChunkKey, DynamicMeshLayerBuild, DynamicMeshResidentChunk,
    DynamicVegetationGroundSource};

#[derive(Resource)]
struct Input { frame: MapSourceFrame, player: Vec3 }

fn sync(mut commands: Commands, input: Res<Input>, presets: Res<VegetationPresetRegistry>,
    mut state: ResMut<DynamicVegetationState>) {
    let source = state.mesh_source.clone().unwrap();
    update_dynamic_mesh_collision_focus(&mut commands, input.player, input.frame,
        &source, &presets, &mut state, false).unwrap();
}

fn run(app: &mut App) {
    app.world_mut().run_system_once(sync).unwrap();
    app.update(); // 走真实层级GlobalTransform传播，代理仍位于源网格子树。
}

fn proxy(app: &App) -> Option<Entity> {
    app.world().resource::<DynamicVegetationState>().resident_mesh_chunks.values().next().unwrap()
        .collision_proxies.get(&7).copied()
}

#[test]
fn mesh_collision_transformed_grid_uses_world_meters_and_rechecks_when_only_grid_pose_moves() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin));
    let catalog: VegetationPresetCatalogAsset = serde_json::from_str(
        crate::test_fixtures::VEGETATION_CATALOG_JSON).unwrap();
    let presets = VegetationPresetRegistry { catalog: Handle::default(), revision: 1,
        resolved: ResolvedVegetationPresetCatalog::build(&catalog).unwrap(), last_error: None };
    let preset = presets.resolve("hn01_tree").unwrap();
    let layer = preset.create_map_layer("maps/test");
    let physics = resolve_mesh_preset_physics(&presets, "hn01_tree").unwrap();
    let enter = physics.collision.enter_distance().unwrap();
    let exit = physics.collision.exit_distance().unwrap();
    let embed = physics.ground_embed_depth;
    let transform = Transform::from_xyz(1_000.0, -8.0, -300.0)
        .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2))
        .with_scale(Vec3::new(2.0, 3.0, 0.25));
    let frame = MapSourceFrame::from_transform(transform);
    let owner = app.world_mut().spawn(transform).id();
    let root = app.world_mut().spawn((Transform::IDENTITY, ChildOf(owner))).id();
    let instance = MapVegetationInstanceTransform { instance_id: 7,
        translation: [12.0, 2.0, 2.0], yaw_radians: 0.0, scale: 1.0 };
    let origin = Vec3::from_array(instance.translation);
    let source = Arc::new(DynamicVegetationBuild {
        generation: 1, revision: 1,
        mesh_layers: vec![DynamicMeshLayerBuild { layer_index: 0, layer, density: Arc::from([255; 4]) }],
        blade_layers: vec![], volume_mesh_layers: Arc::from([]),
        ground: DynamicVegetationGroundSource::Heightfield { heights: Arc::from([0; 4]),
            resolution: [2; 2], base_height: 0.0, height_scale: 1.0 },
        world_min: [0.0; 2], world_max: [128.0, 64.0],
    });
    let key = DynamicMeshChunkKey { layer_index: 0, x: 0, z: 0 };
    let mut state = DynamicVegetationState { mesh_source: Some(source), ..default() };
    state.resident_mesh_chunks.insert(key, DynamicMeshResidentChunk {
        root, clearance_revision: 0, ground_revision: None, instance_count: 1, render_node_count: 1,
        transforms: Arc::from([instance]), collision_proxies: default(),
        source_root_bounds: Some([origin, origin]),
    });
    app.insert_resource(presets).insert_resource(state).insert_resource(Input {
        frame, player: frame.to_world(origin + Vec3::X * ((enter + 16.0) / 2.0)),
    });
    run(&mut app);
    assert!(proxy(&app).is_none(), "源局部距离虽小，世界距离超出预设时不能创建代理");
    app.world_mut().resource_mut::<Input>().player = frame.to_world(origin + Vec3::X * ((enter - 16.0) / 2.0));
    run(&mut app);
    let first = proxy(&app).expect("巨大平移与旋转后仍须按本源树木创建近场代理");
    let expected = frame.to_world(origin + Vec3::Y * (4.0 - embed));
    assert!(app.world().get::<GlobalTransform>(first).unwrap().translation().distance(expected) < 0.001);
    assert!(app.world().get::<Collider>(first).is_some());
    run(&mut app);
    assert_eq!(proxy(&app), Some(first), "相同玩家小格与相同姿态不重建代理");
    let player = app.world().resource::<Input>().player;
    let mut moved = transform;
    moved.translation.x += 500.0;
    *app.world_mut().get_mut::<Transform>(owner).unwrap() = moved;
    app.world_mut().resource_mut::<Input>().frame = MapSourceFrame::from_transform(moved);
    run(&mut app);
    assert_eq!(app.world().resource::<Input>().player, player);
    assert!(proxy(&app).is_none(), "玩家未跨小格，网格单独移远也必须回收已离开近场的代理");
    *app.world_mut().get_mut::<Transform>(owner).unwrap() = transform;
    app.world_mut().resource_mut::<Input>().frame = frame;
    run(&mut app);
    assert!(proxy(&app).is_some(), "网格移回时沿同一姿态门恢复代理");
    app.world_mut().resource_mut::<Input>().player = frame.to_world(origin + Vec3::X * ((exit + 16.0) / 2.0));
    run(&mut app);
    assert!(proxy(&app).is_none());
    // Z轴压缩后，玩家可在源域外很远但世界仍在进入距离；块粗筛不能把合法树木漏掉。
    app.world_mut().resource_mut::<Input>().player = frame.to_world(origin + Vec3::Z * ((enter - 8.0) / 0.25));
    run(&mut app);
    assert!(proxy(&app).is_some(), "非均匀缩放须同时修正块粗筛与逐株精确距离");
    *app.world_mut().get_mut::<Transform>(owner).unwrap() = Transform::IDENTITY;
    *app.world_mut().resource_mut::<Input>() = Input { frame: MapSourceFrame::default(), player: origin };
    run(&mut app);
    assert!(proxy(&app).is_some(), "旧单地形的单位源框架继续工作");

    // 同一源XZ上放置相差798m的两个真实作者根：倾斜后它们在世界XZ分离，
    // 既能检查块三维包围盒包含Y，也能检查逐株筛选没有退回源XZ距离。
    app.world_mut().resource_mut::<Input>().player = origin + Vec3::X * (exit + 64.0);
    run(&mut app);
    assert!(proxy(&app).is_none());
    let high_origin = Vec3::new(origin.x, 800.0, origin.z);
    {
        let mut state = app.world_mut().resource_mut::<DynamicVegetationState>();
        let chunk = state.resident_mesh_chunks.get_mut(&key).unwrap();
        chunk.transforms = Arc::from([
            MapVegetationInstanceTransform { translation: high_origin.to_array(), ..instance },
            MapVegetationInstanceTransform { instance_id: 8, ..instance },
        ]);
        chunk.source_root_bounds = Some([origin, high_origin]);
        chunk.instance_count = 2;
    }
    for rotation in [
        Quat::from_rotation_x(0.42),
        Quat::from_rotation_z(-0.37),
        Quat::from_euler(EulerRot::YXZ, 0.5, 0.4, -0.25),
        Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
        Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
    ] {
        let tilted = transform.with_rotation(rotation);
        let tilted_frame = MapSourceFrame::from_transform(tilted);
        let world_root = tilted_frame.to_world(high_origin);
        // 玩家高度与树根相差800m，但近场合同只比较世界XZ；逆变换后再丢Y会漏建。
        let near_player = world_root + Vec3::X * (enter - 16.0) + Vec3::Y * 800.0;
        *app.world_mut().get_mut::<Transform>(owner).unwrap() = tilted;
        *app.world_mut().resource_mut::<Input>() = Input { frame: tilted_frame, player: near_player };
        run(&mut app);
        let tilted_proxy = proxy(&app).expect("X/Z及组合倾斜后，高作者根仍应在世界近场创建碰撞");
        let global = app.world().get::<GlobalTransform>(tilted_proxy).unwrap();
        let expected = tilted_frame.to_world(high_origin + Vec3::Y * (4.0 - embed));
        assert!(global.translation().distance(expected) < 0.001);
        assert!((Vec3::from(global.affine().matrix3.y_axis) - tilted_frame.source_to_world.y_axis.truncate())
            .length() < 0.001, "真实碰撞代理须继承完整源姿态");
        assert_eq!(app.world().resource::<DynamicVegetationState>().resident_mesh_chunks[&key]
            .collision_proxies.len(), 1, "相同源XZ的低层树不能冒充世界近场根");
        app.world_mut().resource_mut::<Input>().player.y -= 2_000.0;
        run(&mut app);
        assert_eq!(proxy(&app), Some(tilted_proxy), "仅玩家高度变化不改变世界XZ近场");

        app.world_mut().resource_mut::<Input>().player = world_root + Vec3::X * (exit + 32.0);
        run(&mut app);
        assert!(proxy(&app).is_none(), "倾斜后也必须按世界米制回收远离的根");
        app.world_mut().resource_mut::<Input>().player = near_player;
        run(&mut app);
        assert!(proxy(&app).is_some());
        // 玩家静止，只把网格恢复直立；高作者根的世界XZ已移远，姿态身份必须触发核对。
        let upright = tilted.with_rotation(Quat::IDENTITY);
        *app.world_mut().get_mut::<Transform>(owner).unwrap() = upright;
        app.world_mut().resource_mut::<Input>().frame = MapSourceFrame::from_transform(upright);
        run(&mut app);
        assert_eq!(app.world().resource::<Input>().player, near_player);
        assert!(proxy(&app).is_none(), "纯X/Z旋转变化也须回收离开世界近场的代理");
    }
}
