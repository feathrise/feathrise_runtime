//! 用真实碰撞树验证源实例范围、世界贴地和局部写回，防止父图托底掩盖缺页。
use super::*;
use avian3d::prelude::*;
use feathrise_scene_core::residency::TerrainCollisionRegion;
use feathrise_scene_core::residency::{InstanceResourceTable, SceneInstanceOwner};

pub(super) fn region(center: Vec2, extent: f32) -> TerrainCollisionRegion {
    TerrainCollisionRegion { enabled: true, center, max_distance_meters: extent,
        supported_bounds: vec![(center - Vec2::splat(extent), center + Vec2::splat(extent))], ..default() }
}

#[test]
fn a_terrain_object_can_support_its_scene_actor_but_not_another_scene_instance() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default(), TransformPlugin,
        bevy::mesh::MeshPlugin, PhysicsPlugins::default()))
        .init_resource::<InstanceResourceTable<TerrainCollisionRegion>>()
        .add_systems(Update, snap_pending_entities_to_ground);
    app.finish();
    let map = app.world_mut().spawn(Transform::IDENTITY).id();
    let other = app.world_mut().spawn(Transform::IDENTITY).id();
    let terrain = app.world_mut().spawn((Transform::from_xyz(0.,6.,0.), ChildOf(map),
        feathrise_scene_core::residency::SceneTerrainOwner { scene_instance: Some(map) })).id();
    app.world_mut().resource_mut::<InstanceResourceTable<TerrainCollisionRegion>>().values.insert(terrain, region(Vec2::ZERO,10.));
    app.world_mut().spawn((Transform::from_xyz(0.,-0.5,0.), ChildOf(terrain), RigidBody::Static,
        Collider::cuboid(10.,1.,10.), SceneInstanceOwner(terrain)));
    for _ in 0..4 { app.update(); }
    let own = app.world_mut().spawn((Transform::from_xyz(0.,9.,0.), GroundSnap::new(0.5), SceneInstanceOwner(map))).id();
    let foreign = app.world_mut().spawn((Transform::from_xyz(1.,9.,0.), GroundSnap::new(0.5), SceneInstanceOwner(other))).id();
    for _ in 0..4 { app.update(); }
    assert!(!app.world().get::<GroundSnap>(own).unwrap().is_pending());
    assert!((app.world().get::<Transform>(own).unwrap().translation.y-6.5).abs()<0.001);
    assert!(app.world().get::<GroundSnap>(foreign).unwrap().is_pending());
}
#[test]
fn child_ground_support_is_independent_and_parented_objects_write_local_height() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default(), TransformPlugin,
        bevy::mesh::MeshPlugin, PhysicsPlugins::default()))
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_secs_f64(1.0 / 60.0)))
        .insert_resource(region(Vec2::ZERO, 2.0))
        .init_resource::<InstanceResourceTable<TerrainCollisionRegion>>()
        .add_systems(Update, snap_pending_entities_to_ground);
    app.finish();
    let frame = Transform::from_xyz(100.0, 8.0, 20.0).with_rotation(Quat::from_rotation_y(90.0_f32.to_radians()));
    let owner = app.world_mut().spawn(frame).id();
    app.world_mut().resource_mut::<InstanceResourceTable<TerrainCollisionRegion>>().values.insert(owner, region(Vec2::ZERO, 10.0));
    app.world_mut().spawn((Transform::from_xyz(1.0, -0.5, 2.0), ChildOf(owner), RigidBody::Static,
        Collider::cuboid(4.0, 1.0, 4.0), SceneInstanceOwner(owner)));
    for _ in 0..4 { app.update(); }
    let world = frame.transform_point(Vec3::new(1.0, 3.0, 2.0));
    let actor = app.world_mut().spawn((Transform::from_translation(world), GroundSnap::new(0.5))).id();
    let portal = app.world_mut().spawn((Transform::from_xyz(1.0, 3.0, 2.0), ChildOf(owner),
        SceneInstanceOwner(owner), GroundSnap::new(0.5))).id();
    for _ in 0..4 { app.update(); }
    assert!(!app.world().get::<GroundSnap>(actor).unwrap().is_pending());
    assert!((app.world().get::<Transform>(actor).unwrap().translation.y - 8.5).abs() < 0.001);
    assert!((app.world().get::<Transform>(portal).unwrap().translation.y - 0.5).abs() < 0.001);
    assert!((app.world().get::<GroundSnap>(portal).unwrap().snapped_y().unwrap() - 8.5).abs() < 0.001);
    app.world_mut().resource_mut::<InstanceResourceTable<TerrainCollisionRegion>>().values.get_mut(&owner).unwrap().supported_bounds.clear();
    *app.world_mut().resource_mut::<TerrainCollisionRegion>() = region(world.xz(), 10.0);
    app.world_mut().spawn((Transform::from_xyz(world.x, -0.5, world.z), RigidBody::Static, Collider::cuboid(4.0, 1.0, 4.0)));
    app.world_mut().entity_mut(actor).get_mut::<GroundSnap>().unwrap().request();
    for _ in 0..4 { app.update(); }
    assert!(app.world().get::<GroundSnap>(actor).unwrap().is_pending());
    assert!((app.world().get::<Transform>(actor).unwrap().translation.y - 8.5).abs() < 0.001); // 不穿过缺覆盖的最近子地表去借用父图。
}
