//! 真实实例上下文中的Clipmap镜头跟随回归：共享相机、编辑焦点及源局部变换。

use super::*;
use feathrise_scene_core::residency::{
    InstanceResourceTable, SceneInstanceContextAppExt, SceneInstanceOwner, SceneRuntimeScope,
    SceneSourceFrame, initialize_instance_context,
};

fn fixture() -> App {
    let mut app = App::new();
    app.init_instance_resource::<DynamicSceneRuntimeState>()
        .init_instance_resource::<SparseClipmapRuntime>()
        .init_instance_resource::<super::super::clipmap::SparseHeightPageRuntime>();
    app
}

fn source(world: &mut World, instance: Option<Entity>) -> Entity {
    let display = world.spawn((SparseClipmapDisplay, Transform::IDENTITY, Mesh3d::default())).id();
    if let Some(instance) = instance {
        world.entity_mut(display).insert(SceneInstanceOwner(instance));
    }
    let runtime = SparseClipmapRuntime {
        display: Some(display),
        bounds: Some(ClipmapBounds::new(Vec2::ZERO, Vec2::splat(128.0)).unwrap()),
        spec: Some(SparseHeightSpec::new([0.0, 0.0], [128.0, 128.0], 1.0, -64.0, 128.0).unwrap()),
        ..default()
    };
    if let Some(instance) = instance {
        world.resource_mut::<InstanceResourceTable<SparseClipmapRuntime>>().values.insert(instance, runtime);
    } else {
        world.insert_resource(runtime);
    }
    display
}

fn follow(world: &mut World, instance: Option<Entity>, frame: SceneSourceFrame) {
    *world.resource_mut::<SceneRuntimeScope>() = SceneRuntimeScope { instance, frame };
    let mut schedule = Schedule::default();
    schedule.add_systems(follow_sparse_clipmap);
    schedule.run(world); // 运行正式系统，不能只测试预先选好的相机坐标。
}

fn position(world: &World, entity: Entity) -> Vec3 {
    world.get::<Transform>(entity).unwrap().translation
}

#[test]
fn independent_clipmap_lod_follows_shared_camera_in_each_source_frame() {
    let mut app = fixture();
    let world = app.world_mut();
    let camera = world.spawn((Camera3d::default(), GlobalTransform::from_translation(Vec3::new(24.0, 40.0, 36.0)))).id();
    world.resource_mut::<DynamicSceneRuntimeState>().camera = Some(camera);
    // 资产缩略图相机没有父世界所有权，不能被任意Camera查询选为地形观察点。
    world.spawn((Camera3d::default(), GlobalTransform::from_translation(Vec3::splat(120.0))));
    let instances = [world.spawn_empty().id(), world.spawn_empty().id()];
    for instance in instances { initialize_instance_context(world, instance); }
    let displays = instances.map(|instance| source(world, Some(instance)));
    let shifted = SceneSourceFrame::from_transform(Transform::from_xyz(10.0, 0.0, 18.0).with_scale(Vec3::splat(2.0)));
    follow(world, Some(instances[0]), SceneSourceFrame::default());
    follow(world, Some(instances[1]), shifted);
    assert_eq!(position(world, displays[0]), Vec3::new(24.0, 0.0, 36.0));
    assert_eq!(position(world, displays[1]), Vec3::new(7.0, 0.0, 9.0));

    world.entity_mut(camera).insert(GlobalTransform::from_translation(Vec3::new(40.0, 40.0, 50.0)));
    follow(world, Some(instances[0]), SceneSourceFrame::default());
    follow(world, Some(instances[1]), shifted);
    assert_eq!(position(world, displays[0]), Vec3::new(40.0, 0.0, 50.0));
    assert_eq!(position(world, displays[1]), Vec3::new(15.0, 0.0, 16.0));
    for instance in instances {
        assert!(world.resource::<InstanceResourceTable<DynamicSceneRuntimeState>>().values[&instance].camera.is_none());
        let runtime = &world.resource::<InstanceResourceTable<SparseClipmapRuntime>>().values[&instance];
        assert!(runtime.task.is_none() && runtime.pending_cpu.is_none()); // 镜头移动不得重建或上传拓扑。
    }
}

#[test]
fn editor_clipmap_focus_works_without_game_camera_and_keeps_world_height() {
    let mut app = fixture();
    let world = app.world_mut();
    let instance = world.spawn_empty().id();
    initialize_instance_context(world, instance);
    let display = source(world, Some(instance));
    let frame = SceneSourceFrame::from_transform(Transform::from_xyz(30.0, 20.0, 40.0)
        .with_rotation(Quat::from_rotation_x(0.5)).with_scale(Vec3::splat(2.0)));
    let focus = frame.to_world(Vec3::new(8.0, 7.0, 12.0));
    let editor = world.spawn((Camera::default(), DynamicClipmapAuthoringView {
        focus_xz: focus.xz(), focus_y: focus.y,
    })).id();
    follow(world, Some(instance), frame);
    assert_eq!(position(world, display), Vec3::new(8.0, 0.0, 12.0));

    let camera = world.spawn((Camera3d::default(), GlobalTransform::from_translation(frame.to_world(Vec3::new(90.0, 0.0, 70.0))))).id();
    world.resource_mut::<DynamicSceneRuntimeState>().camera = Some(camera);
    follow(world, Some(instance), frame);
    assert_eq!(position(world, display), Vec3::new(8.0, 0.0, 12.0)); // F12焦点优先于远处玩法相机。
    world.get_mut::<Camera>(editor).unwrap().is_active = false;
    follow(world, Some(instance), frame);
    assert_eq!(position(world, display), Vec3::new(90.0, 0.0, 70.0)); // 关闭F12恢复玩法视图。
}

#[test]
fn root_clipmap_lod_keeps_global_center_outside_bounds_and_during_camera_handoff() {
    let mut app = fixture();
    let world = app.world_mut();
    let display = source(world, None);
    let camera = world.spawn((Camera3d::default(), GlobalTransform::from_translation(Vec3::new(-50.0, 10.0, 180.0)))).id();
    world.resource_mut::<DynamicSceneRuntimeState>().camera = Some(camera);
    follow(world, None, SceneSourceFrame::default());
    assert_eq!(position(world, display), Vec3::new(-50.0, 0.0, 180.0));
    world.despawn(camera);
    follow(world, None, SceneSourceFrame::default());
    assert_eq!(position(world, display), Vec3::new(-50.0, 0.0, 180.0)); // 交接空窗不能把细环跳回中心。
}

#[test]
fn camera_candidate_does_not_move_display_before_pages_are_adopted() {
    let mut app = fixture();
    let world = app.world_mut();
    let display = source(world, None);
    world.resource_mut::<SparseClipmapRuntime>().requested_focus = Some(Vec2::new(50.0, 80.0));
    follow(world, None, SceneSourceFrame::default());
    assert_eq!(position(world, display), Vec3::ZERO, "新中心没有完整页组时必须保留原显示位置");
}
