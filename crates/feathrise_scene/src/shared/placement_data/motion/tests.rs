use super::*;
use crate::shared::placement_data::{builtin, components::PlacementInstanceGate};
use avian3d::PhysicsPlugins;
use bevy::{asset::AssetPlugin, mesh::MeshPlugin, time::TimeUpdateStrategy, transform::TransformPlugin};
use feathrise_scene_content::{SceneId, ScenePlacementAssetTarget};
use std::time::Duration;

#[test]
fn scene_motion_collision_carries_a_rider_and_retires() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), TransformPlugin, MeshPlugin, PhysicsPlugins::default()))
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(1.0 / 64.0)))
        .add_systems(FixedUpdate, drive);
    app.finish();

    let mut record: ScenePlacementRecord = serde_json::from_str(r#"{
        "id":"test-lift", "translation":[0,1,0], "yaw_degrees":0, "scale":[2,1,3]
    }"#).unwrap();
    record.asset_target = ScenePlacementAssetTarget::DEFAULT_CUBOID;
    let track: SceneMotionTrack = serde_json::from_str(r#"{
        "end_offset":[0,2,0], "travel_seconds":1.5, "hold_seconds":0.5,
        "collider":{"center":[0,0.25,0],"half_extents":[1.5,0.2,1.5]}
    }"#).unwrap();
    let gate = PlacementInstanceGate::new(false);
    let scene = SceneId::parse("SCENE-motion").unwrap();
    let parent = app.world_mut().spawn((Transform::from_xyz(3.0, 2.0, -1.0), InheritedVisibility::VISIBLE)).id();
    let transform = Transform::from_xyz(0.0, 1.0, 0.0).with_scale(Vec3::new(2.0, 1.0, 3.0));
    let platform = app.world_mut().spawn((
        transform, ChildOf(parent), ScenePlacementInstance::new_pending(&scene, &record, gate.clone()),
    )).id();
    let mut commands = app.world_mut().commands();
    spawn(&mut commands.entity(platform), transform, &record, Arc::new(track));
    assert!(builtin::spawn_builtin_placement_visual(&mut commands, platform, record.asset_target));
    app.world_mut().flush();
    for _ in 0..6 { app.update(); }
    let child = app.world().get::<MotionPlatform>(platform).unwrap().collider.unwrap();
    assert_eq!(app.world().get::<ColliderOf>(child).unwrap().body, platform);
    assert!(builtin::builtin_placement_visual_ready(app.world(), platform));
    let collider_count = app.world().get::<Children>(platform).unwrap().into_iter()
        .filter(|&&entity| app.world().get::<Collider>(entity).is_some()).count();
    assert_eq!(collider_count, 1, "内置可视网格不得再生成静态碰撞");
    assert!(app.world().get::<RigidBodyDisabled>(platform).is_some());
    assert_eq!(app.world().get::<Position>(platform).unwrap().0.y, 3.0);

    gate.activate();
    let rider = app.world_mut().spawn((
        Transform::from_xyz(3.0, 3.75, -1.0), RigidBody::Dynamic, Collider::sphere(0.25),
    )).id();
    for _ in 0..64 { app.update(); }
    let before_edit = app.world().get::<Position>(platform).unwrap().0;
    app.world_mut().insert_resource(EditorInputState { scene_editor_open: true, ..default() });
    for _ in 0..20 { app.update(); }
    assert_eq!(app.world().get::<Position>(platform).unwrap().0, before_edit);
    app.world_mut().resource_mut::<EditorInputState>().scene_editor_open = false;
    for _ in 64..154 { app.update(); } // 等上行减速后的乘客在高端重新落稳。
    let platform_y = app.world().get::<Position>(platform).unwrap().0.y;
    let rider_y = app.world().get::<Position>(rider).unwrap().0.y;
    assert!((platform_y - 5.0).abs() < 0.02, "上端停留时平台高度为 {platform_y}");
    assert!((rider_y - (platform_y + 0.7)).abs() < 0.06, "乘客随真实碰撞上升至 {rider_y}");

    for _ in 0..104 { app.update(); }
    let platform_y = app.world().get::<Position>(platform).unwrap().0.y;
    let rider_y = app.world().get::<Position>(rider).unwrap().0.y;
    assert!((platform_y - 3.0).abs() < 0.02, "往返后平台回到地面 {platform_y}");
    assert!((rider_y - (platform_y + 0.7)).abs() < 0.06, "乘客跟随回程到 {rider_y}");

    gate.retire();
    for _ in 0..32 { app.update(); }
    assert!(app.world().get::<RigidBodyDisabled>(platform).is_some());
    assert!(app.world().get::<TnuaNotPlatform>(platform).is_some());
    assert!(app.world().get::<Position>(rider).unwrap().0.y < 3.0, "退役平台不能继续托住乘客");
}

#[test]
fn scene_motion_standard_glb_rotation_and_scale_update_the_offset_collision() {
    use avian3d::prelude::{PhysicsSystems, SpatialQuery, SpatialQueryFilter};
    use bevy::{
        asset::io::{AssetSourceBuilder, AssetSourceId, memory::{Dir, MemoryAssetReader}},
        ecs::system::RunSystemOnce, gltf::GltfPlugin, world_serialization::WorldAsset,
    };
    use feathrise_scene_content::{SceneContentPlugin, SceneMotionAsset};
    use serde_json::json;
    use std::{path::Path, time::Instant};

    let quarter = std::f32::consts::FRAC_1_SQRT_2;
    let binary: Vec<_> = [0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, quarter, 0.0, quarter,
        2.0, 1.0, 3.0, 4.0, 2.0, 1.5]
        .into_iter().flat_map(f32::to_le_bytes).collect();
    let mut document = serde_json::to_vec(&json!({
        "asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],
        "nodes":[{"extras":{"feathrise_id":"旋转盒",
            "feathrise_motion_collider":{"center":[0.4,0,0],"half_extents":[1,0.2,0.3]}}}],
        "buffers":[{"byteLength":binary.len()}],
        "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":8},
                       {"buffer":0,"byteOffset":8,"byteLength":32},
                       {"buffer":0,"byteOffset":40,"byteLength":24}],
        "accessors":[{"bufferView":0,"componentType":5126,"count":2,"type":"SCALAR","min":[0],"max":[1]},
                     {"bufferView":1,"componentType":5126,"count":2,"type":"VEC4"},
                     {"bufferView":2,"componentType":5126,"count":2,"type":"VEC3"}],
        "animations":[{"extras":{"feathrise_autoplay":true},
                       "channels":[{"sampler":0,"target":{"node":0,"path":"rotation"}},
                                   {"sampler":1,"target":{"node":0,"path":"scale"}}],
                       "samplers":[{"input":0,"output":1},{"input":0,"output":2}]}]
    })).unwrap();
    document.resize(document.len().next_multiple_of(4), b' ');
    let mut glb = b"glTF".to_vec();
    glb.extend(2_u32.to_le_bytes());
    glb.extend(((28 + document.len() + binary.len()) as u32).to_le_bytes());
    glb.extend((document.len() as u32).to_le_bytes());
    glb.extend(b"JSON");
    glb.extend(document);
    glb.extend((binary.len() as u32).to_le_bytes());
    glb.extend(b"BIN\0");
    glb.extend(binary);

    let files = Dir::default();
    files.insert_asset(Path::new("scene.glb"), glb);
    let mut app = App::new();
    app.register_asset_source(AssetSourceId::Default, AssetSourceBuilder::new(move || {
        Box::new(MemoryAssetReader { root: files.clone() })
    }))
    .add_plugins((MinimalPlugins, AssetPlugin::default(), TransformPlugin, MeshPlugin,
        GltfPlugin::default(), SceneContentPlugin, PhysicsPlugins::default()))
    .init_asset::<AnimationClip>()
    .init_asset::<WorldAsset>()
    .init_asset::<StandardMaterial>()
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(1.0 / 64.0)))
    .add_systems(FixedUpdate, drive.before(PhysicsSystems::Prepare));
    app.finish();
    app.cleanup();
    let motion = app.world().resource::<AssetServer>().load::<SceneMotionAsset>("scene.glb#SceneMotion");
    let deadline = Instant::now() + Duration::from_secs(5);
    let track = loop {
        app.update();
        if let Some(asset) = app.world().resource::<Assets<SceneMotionAsset>>().get(&motion) {
            break asset.tracks.as_ref().unwrap()["旋转盒"].clone();
        }
        assert!(Instant::now() < deadline, "真实 GLB 旋转片段未加载");
        std::thread::sleep(Duration::from_millis(1));
    };
    let record: ScenePlacementRecord = serde_json::from_value(json!({
        "id":"旋转盒","translation":[0,1,0],"yaw_degrees":0,"scale":[2,1,3],
        "asset_target":ScenePlacementAssetTarget::DEFAULT_CUBOID,
    })).unwrap();
    let scene = SceneId::parse("SCENE-motion").unwrap();
    let parent = app.world_mut().spawn((Transform::from_xyz(3.0, 2.0, -1.0), InheritedVisibility::VISIBLE)).id();
    let origin = Transform::from_xyz(0.0, 1.0, 0.0).with_scale(Vec3::new(2.0, 1.0, 3.0));
    let platform = app.world_mut().spawn((
        origin, ChildOf(parent), ScenePlacementInstance::new(&scene, &record),
    )).id();
    spawn(&mut app.world_mut().commands().entity(platform), origin, &record, track);
    app.world_mut().flush();
    for _ in 0..90 { app.update(); }

    let pose = app.world().get::<Transform>(platform).unwrap();
    assert!(pose.translation.distance(origin.translation) < 0.001, "偏心盒转动时模型原点偏移：{pose:?}");
    assert!(pose.rotation.dot(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)).abs() > 0.9999);
    assert_eq!(pose.scale, Vec3::new(4.0, 2.0, 1.5), "可视模型采用标准 scale 的最后一键");
    let shape = app.world().get::<MotionPlatform>(platform).unwrap().collider.unwrap();
    assert_eq!(app.world().get::<Collider>(shape).unwrap().scale(), Vec3::new(2.0, 2.0, 0.5));
    let hit = app.world_mut().run_system_once(|query: SpatialQuery| {
        query.cast_ray(Vec3::new(3.0, 4.0, -3.3), Dir3::NEG_Y, 3.0, true, &SpatialQueryFilter::default())
            .map(|hit| hit.entity)
    }).unwrap();
    assert_eq!(hit, Some(shape), "射线能命中新缩放后延伸的碰撞范围");
}
