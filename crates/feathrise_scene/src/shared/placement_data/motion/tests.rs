use super::*;
use crate::shared::placement_data::{
    builtin, components::PlacementInstanceGate, resources::{PlacementActiveBatch, ScenePlacementRuntimeState},
};
use avian3d::{PhysicsPlugins, prelude::PhysicsSystems};
use bevy::{
    asset::{AssetPlugin, io::{AssetSourceBuilder, AssetSourceId, memory::{Dir, MemoryAssetReader}}},
    gltf::GltfPlugin, mesh::MeshPlugin, time::TimeUpdateStrategy, transform::TransformPlugin,
    world_serialization::WorldAsset,
};
use feathrise_scene_content::{SceneContentPlugin, SceneId, SceneInstancePath, SceneMotionAsset, SceneMotionClips, ScenePlacementAssetTarget};
use feathrise_scene_core::residency::InstanceResourceTable;
use serde_json::{Value, json};
use std::{path::Path, time::{Duration, Instant}};

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
    use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};
    use bevy::ecs::system::RunSystemOnce;

    let quarter = std::f32::consts::FRAC_1_SQRT_2;
    let binary: Vec<_> = [0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, quarter, 0.0, quarter,
        2.0, 1.0, 3.0, 4.0, 2.0, 1.5]
        .into_iter().flat_map(f32::to_le_bytes).collect();
    let document = json!({
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
    });
    let (mut app, motion) = motion_app(document, binary);
    let track = app.world().resource::<Assets<SceneMotionAsset>>().get(&motion)
        .unwrap().tracks.as_ref().unwrap()["旋转盒"].clone();
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

#[derive(Resource, Default)]
struct FinishedClips(Vec<SceneMotionFinished>);

#[test]
fn observed_motion_region_opens_once_and_reverses_after_last_observer_leaves() {
    let (mut app, clips) = manual_motion_app();
    app.init_resource::<SceneMotionTriggerTrace>();
    let scene = SceneId::parse("SCENE-motion").unwrap();
    let [door, _] = use_motion_clips(app.world_mut(), &scene, clips, None);
    let observers = [0.0, 8.0].map(|x| app.world_mut().spawn((crate::SceneRuntimeObserver,
        Transform::from_xyz(x,1.0,0.0))).id());
    let run = |app: &mut App, count| { for _ in 0..count { app.update(); } };
    run(&mut app,85);
    let feedback = |app: &App| {
        let hit = app.world().resource::<SceneMotionTriggerTrace>().last.as_ref().unwrap();
        (hit.clip_name.clone(), hit.entering, hit.action)
    };
    let entered = feedback(&app);
    assert!(!entered.0.is_empty() && entered.1 && entered.2 == SceneMotionAction::Play);
    assert!((app.world().get::<Position>(door).unwrap().x - 4.0).abs() < 0.001);
    app.world_mut().get_mut::<Transform>(observers[1]).unwrap().translation.x = 0.5;
    run(&mut app,3);
    app.world_mut().get_mut::<Transform>(observers[0]).unwrap().translation.x = -6.0;
    run(&mut app,10);
    assert_eq!(feedback(&app), entered);
    assert!((app.world().get::<Position>(door).unwrap().x - 4.0).abs() < 0.001);
    app.world_mut().get_mut::<Transform>(observers[1]).unwrap().translation.x = 7.0;
    run(&mut app,85);
    assert!(app.world().get::<Position>(door).unwrap().x.abs() < 0.001);
    assert_eq!(feedback(&app), (entered.0, false, SceneMotionAction::Reverse));
    let events = &app.world().resource::<FinishedClips>().0;
    assert_eq!(events.iter().map(|e| e.direction).collect::<Vec<_>>(),
        [SceneMotionDirection::Forward, SceneMotionDirection::Backward]);
}

#[test]
fn scene_motion_commands_control_independent_targets_and_finish_once() {
    let (mut app, clips) = manual_motion_app();
    let scene = SceneId::parse("SCENE-motion").unwrap();
    let instance = SceneInstancePath::root(scene.clone());
    let [door, lift] = use_motion_clips(app.world_mut(), &scene, clips, None);
    let step = |app: &mut App, frames| {
        for _ in 0..frames { app.update(); }
    };
    let position = |app: &App, entity| app.world().get::<Position>(entity).unwrap().0;
    let command = |app: &mut App, id: &str, action| {
        app.world_mut().write_message(SceneMotionCommand {
            instance: instance.clone(), clip_id: id.into(), action,
        });
    };

    step(&mut app, 4);
    assert_eq!(position(&app, door).x, 0.0);
    assert_eq!(position(&app, lift).y, 1.0);
    command(&mut app, "door", SceneMotionAction::Play);
    step(&mut app, 30);
    assert!(position(&app, door).x > 1.0);
    assert_eq!(position(&app, lift).y, 1.0);
    command(&mut app, "lift", SceneMotionAction::Play);
    command(&mut app, "door", SceneMotionAction::Pause);
    step(&mut app, 4);
    let paused = position(&app, door);
    step(&mut app, 80);
    assert_eq!(position(&app, door), paused);
    assert!((position(&app, lift).y - 3.0).abs() < 0.001);

    command(&mut app, "door", SceneMotionAction::Reverse);
    command(&mut app, "lift", SceneMotionAction::Stop);
    step(&mut app, 70);
    assert!(position(&app, door).x.abs() < 0.001);
    assert!((position(&app, lift).y - 1.0).abs() < 0.001);
    assert!(app.world().get::<MotionPlatform>(lift).unwrap().collider_ready(app.world(), lift));
    assert!(app.world().get::<RigidBodyDisabled>(lift).is_none());
    assert_eq!(app.world().resource::<FinishedClips>().0, [
        SceneMotionFinished { instance: instance.clone(), clip_id: "lift".into(), direction: SceneMotionDirection::Forward },
        SceneMotionFinished { instance: instance.clone(), clip_id: "door".into(), direction: SceneMotionDirection::Backward },
    ]);

    command(&mut app, "door", SceneMotionAction::Play);
    command(&mut app, "door-alternate", SceneMotionAction::Play);
    step(&mut app, 18);
    assert!(position(&app, door).x < 0.0, "同一对象由后发的明确播放命令接管");
    command(&mut app, "door", SceneMotionAction::Play);
    step(&mut app, 8);
    assert!(position(&app, door).x > 0.0, "原片段可再次接管，沿其自己的时钟继续");
}

#[cfg(feature = "runtime_test_support")]
#[test]
fn world_observer_triggers_only_the_translated_instance_containing_it() {
    use feathrise_scene_content::{SceneInstanceId, SceneInstancePlacement, SceneInstanceTransform};
    let (mut app, clips) = manual_motion_app();
    let owner = SceneId::parse("SCENE-main").unwrap();
    let source = SceneId::parse("SCENE-doors").unwrap();
    crate::runtime_test_support::install_active_terrain_instances(app.world_mut(), &owner, &[]);
    for (name, x) in [("left",-20.0),("right",20.0)] {
        let placement = SceneInstancePlacement { id: SceneInstanceId::parse(name).unwrap(), source_scene_id: source.clone(),
            label: String::new(), transform: SceneInstanceTransform { translation: [x,0.0,0.0], yaw_degrees: 90.0, ..default() } };
        let instance = crate::runtime_test_support::install_active_subscene_instance(app.world_mut(), &owner, &placement);
        use_motion_clips(app.world_mut(), &source, clips.clone(), Some(instance));
    }
    app.world_mut().spawn((crate::SceneRuntimeObserver,Transform::from_xyz(20.0,1.0,0.0)));
    for _ in 0..85 { app.update(); }
    let events = &app.world().resource::<FinishedClips>().0;
    assert_eq!(events.len(),1);
    assert_eq!(events[0].instance,SceneInstancePath::root(owner).child(SceneInstanceId::parse("right").unwrap()));
}

#[cfg(feature = "runtime_test_support")]
#[test]
fn scene_motion_commands_address_one_of_two_instances_of_the_same_source() {
    use feathrise_scene_content::{SceneInstanceId, SceneInstancePlacement};

    let (mut app, clips) = manual_motion_app();
    let owner = SceneId::parse("SCENE-main").unwrap();
    let source = SceneId::parse("SCENE-shared").unwrap();
    crate::runtime_test_support::install_active_terrain_instances(app.world_mut(), &owner, &[]);
    let mut install_subscene = |id: &str| {
        let placement = SceneInstancePlacement {
            id: SceneInstanceId::parse(id).unwrap(), source_scene_id: source.clone(),
            label: String::new(), transform: default(),
        };
        let entity = crate::runtime_test_support::install_active_subscene_instance(app.world_mut(), &owner, &placement);
        use_motion_clips(app.world_mut(), &source, clips.clone(), Some(entity))[0]
    };
    let left = install_subscene("left");
    let right = install_subscene("right");
    let instance = SceneInstancePath::root(owner).child(SceneInstanceId::parse("right").unwrap());
    app.world_mut().write_message(SceneMotionCommand {
        instance: instance.clone(), clip_id: "door".into(), action: SceneMotionAction::Play,
    });
    for _ in 0..75 { app.update(); }
    assert_eq!(app.world().get::<Position>(left).unwrap().0.x, 0.0);
    assert!((app.world().get::<Position>(right).unwrap().0.x - 4.0).abs() < 0.001);
    assert_eq!(app.world().resource::<FinishedClips>().0, [
        SceneMotionFinished { instance, clip_id: "door".into(), direction: SceneMotionDirection::Forward },
    ]);
}

fn use_motion_clips(world: &mut World, scene: &SceneId, clips: Arc<SceneMotionClips>, owner: Option<Entity>) -> [Entity; 2] {
    let gate = PlacementInstanceGate::new(true);
    let root = world.spawn((
        Transform::IDENTITY, InheritedVisibility::VISIBLE, MotionController::new(scene.clone(), clips.clone(), gate.clone()),
    )).id();
    if let Some(owner) = owner { world.entity_mut(root).insert((ChildOf(owner), SceneInstanceOwner(owner))); }
    let objects = [("door", 0.0), ("lift", 4.0)].map(|(id, z)| {
        let record: ScenePlacementRecord = serde_json::from_value(json!({
            "id":id, "translation":[0,1,z], "yaw_degrees":0, "scale":[1,1,1],
            "asset_target":ScenePlacementAssetTarget::DEFAULT_CUBOID,
        })).unwrap();
        let origin = Transform::from_xyz(0.0, 1.0, z);
        let entity = world.spawn((origin, ChildOf(root), ScenePlacementInstance::new_pending(scene, &record, gate.clone()))).id();
        if let Some(owner) = owner { world.entity_mut(entity).insert(SceneInstanceOwner(owner)); }
        let motion = PlacementMotion::Glb(clips.placement(id).unwrap().clone());
        spawn(&mut world.commands().entity(entity), origin, &record, motion);
        entity
    });
    let state = ScenePlacementRuntimeState {
        active_scene_id: Some(scene.clone()),
        active: Some(PlacementActiveBatch {
            building_preset_revision: 0, terrain_roots: Vec::new(), primitive_collision_roots: Vec::new(),
            root, roots: objects.to_vec(), gate, marker_cleanup_cursor: 0,
        }),
        ..default()
    };
    if let Some(owner) = owner {
        world.get_resource_or_init::<InstanceResourceTable<ScenePlacementRuntimeState>>().values.insert(owner, state);
    } else {
        world.insert_resource(state);
    }
    world.flush();
    objects
}

#[test]
fn cinematic_sampling_uses_only_the_named_placement_track_without_advancing_game_time() {
    let (mut app, clips) = manual_motion_app();
    let scene = SceneId::parse("SCENE-motion").unwrap();
    let [door, lift] = use_motion_clips(app.world_mut(), &scene, clips, None);
    let (origin, half) = crate::sample_placement_clip(app.world(), door, "door", 0.5).unwrap();
    assert_eq!(origin.translation, Vec3::Y);
    assert!((half.translation.x - 2.).abs() < 0.0001);
    let (_, end) = crate::sample_placement_clip(app.world(), door, "door-alternate", 5.).unwrap();
    assert_eq!(end.translation.x, -2.);
    assert!(crate::sample_placement_clip(app.world(), lift, "door", 0.5).is_err());
    assert_eq!(app.world().get::<Transform>(door).unwrap().translation, origin.translation);
    assert!(app.world().resource::<FinishedClips>().0.is_empty());
}

fn manual_motion_app() -> (App, Arc<SceneMotionClips>) {
    let binary: Vec<_> = [0.0, 1.0, 0.0, 1.0, 0.0, 4.0, 1.0, 0.0, 0.0, 1.0, 4.0, 0.0, 3.0, 4.0,
        0.0, 1.0, 0.0, -2.0, 1.0, 0.0]
        .into_iter().flat_map(f32::to_le_bytes).collect();
    let nodes = ["door", "lift"].map(|id| json!({"extras":{"feathrise_id":id}}));
    let mut animations = [("door", 0, 1), ("lift", 1, 2), ("door-alternate", 0, 3)].map(|(id, node, output)| json!({
        "name":id, "extras":{"feathrise_scene_animation":true,"feathrise_clip_id":id},
        "channels":[{"sampler":0,"target":{"node":node,"path":"translation"}}],
        "samplers":[{"input":0,"output":output}],
    }));
    animations[0]["extras"]["feathrise_motion_triggers"] = json!([{
        "center":[0,1,0],"half_extents":[2,1,2],"enter":"play","exit":"reverse"
    }]);
    let document = json!({
        "asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0,1]}],"nodes":nodes,
        "buffers":[{"byteLength":binary.len()}],
        "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":8},
                       {"buffer":0,"byteOffset":8,"byteLength":24},
                       {"buffer":0,"byteOffset":32,"byteLength":24},
                       {"buffer":0,"byteOffset":56,"byteLength":24}],
        "accessors":[{"bufferView":0,"componentType":5126,"count":2,"type":"SCALAR","min":[0],"max":[1]},
                     {"bufferView":1,"componentType":5126,"count":2,"type":"VEC3"},
                     {"bufferView":2,"componentType":5126,"count":2,"type":"VEC3"},
                     {"bufferView":3,"componentType":5126,"count":2,"type":"VEC3"}],
        "animations":animations,
    });
    let (app, motion) = motion_app(document, binary);
    let clips = app.world().resource::<Assets<SceneMotionAsset>>().get(&motion).unwrap().clips.clone();
    (app, clips)
}

fn motion_app(document: Value, binary: Vec<u8>) -> (App, Handle<SceneMotionAsset>) {
    let mut document = serde_json::to_vec(&document).unwrap();
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
    .init_resource::<FinishedClips>()
    .add_message::<SceneMotionCommand>()
    .add_message::<SceneMotionFinished>()
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(1.0 / 64.0)))
    .add_systems(PostUpdate, apply_commands)
    .add_systems(FixedUpdate, (advance_playback, drive).chain().before(PhysicsSystems::Prepare))
    .add_systems(Last, |mut events: MessageReader<SceneMotionFinished>, mut clips: ResMut<FinishedClips>| {
        clips.0.extend(events.read().cloned());
    });
    app.finish();
    app.cleanup();
    let motion = app.world().resource::<AssetServer>().load::<SceneMotionAsset>("scene.glb#SceneMotion");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.update();
        if app.world().resource::<Assets<SceneMotionAsset>>().get(&motion).is_some() { return (app, motion); }
        assert!(Instant::now() < deadline, "真实 GLB 片段未加载");
        std::thread::sleep(Duration::from_millis(1));
    }
}
