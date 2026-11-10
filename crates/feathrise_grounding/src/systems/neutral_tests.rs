//! 无角色/游戏状态的真实Avian回归：应用就绪门、来源代次、覆盖与禁用刚体层级。

use super::*;
use avian3d::prelude::*;
use feathrise_scene_core::residency::{InstanceResourceTable, SceneInstanceOwner};

fn physics_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::asset::AssetPlugin::default(),
        TransformPlugin,
        bevy::mesh::MeshPlugin,
        PhysicsPlugins::default(),
        crate::GroundingPlugin,
    ))
    .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / 60.0),
    ));
    app.finish();
    app
}

fn tick(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

fn assert_y(app: &App, entity: Entity, expected: f32) {
    assert!((app.world().get::<Transform>(entity).unwrap().translation.y - expected).abs() < 0.001);
    assert!(!app.world().get::<GroundSnap>(entity).unwrap().is_pending());
}

#[test]
fn shared_plugin_needs_no_game_state_and_obeys_application_collision_readiness() {
    #[derive(Resource, Default)]
    struct CollisionReady(bool);
    let mut app = physics_app();
    app.init_resource::<CollisionReady>().configure_sets(
        Update,
        crate::GroundingSet::Snap.run_if(|ready: Res<CollisionReady>| ready.0),
    );
    app.world_mut().spawn((
        Transform::from_xyz(0.0, -0.5, 0.0),
        RigidBody::Static,
        Collider::cuboid(10.0, 1.0, 10.0),
    ));
    tick(&mut app);
    let actor = app
        .world_mut()
        .spawn((Transform::from_xyz(0.0, 9.0, 0.0), GroundSnap::new(0.5)))
        .id();
    tick(&mut app);
    assert!(app.world().get::<GroundSnap>(actor).unwrap().is_pending());
    assert_eq!(
        app.world().get::<Transform>(actor).unwrap().translation.y,
        9.0
    );
    app.world_mut().resource_mut::<CollisionReady>().0 = true;
    tick(&mut app);
    assert_y(&app, actor, 0.5); // 同一请求在应用真实放行后完成，不需要GameScene或角色资源。
}

#[test]
fn explicit_source_and_published_collision_revision_never_borrow_parent_support() {
    let mut app = physics_app();
    app.init_resource::<InstanceResourceTable<TerrainCollisionRegion>>();
    let source = app.world_mut().spawn(Transform::IDENTITY).id();
    let replaced = app.world_mut().spawn(Transform::IDENTITY).id();
    app.world_mut().spawn((
        Transform::from_xyz(0.0, 5.5, 0.0),
        RigidBody::Static,
        Collider::cuboid(10.0, 1.0, 10.0),
        SceneInstanceOwner(source),
    ));
    app.world_mut().spawn((
        Transform::from_xyz(0.0, -0.5, 0.0),
        RigidBody::Static,
        Collider::cuboid(10.0, 1.0, 10.0),
    ));
    let mut region = super::tests::region(Vec2::ZERO, 10.0);
    region.revision = 1;
    app.world_mut()
        .resource_mut::<InstanceResourceTable<TerrainCollisionRegion>>()
        .values
        .insert(source, region);
    tick(&mut app);
    let actor = app
        .world_mut()
        .spawn((
            Transform::from_xyz(0.0, 9.0, 0.0),
            GroundSnap::new(0.5),
            GroundSnapFilter {
                source: GroundSnapSource::Unavailable,
                ..default()
            },
        ))
        .id();
    tick(&mut app);
    assert!(app.world().get::<GroundSnap>(actor).unwrap().is_pending());
    app.world_mut()
        .get_mut::<GroundSnapFilter>(actor)
        .unwrap()
        .source = GroundSnapSource::Instance(replaced);
    tick(&mut app);
    assert!(app.world().get::<GroundSnap>(actor).unwrap().is_pending()); // 同位置旧实例不能冒充当前来源。
    app.world_mut()
        .get_mut::<GroundSnapFilter>(actor)
        .unwrap()
        .source = GroundSnapSource::Instance(source);
    {
        let mut regions = app
            .world_mut()
            .resource_mut::<InstanceResourceTable<TerrainCollisionRegion>>();
        let region = regions.values.get_mut(&source).unwrap();
        region.revision = 2;
        region.supported_bounds.clear();
    }
    tick(&mut app);
    assert!(app.world().get::<GroundSnap>(actor).unwrap().is_pending());
    assert_eq!(
        app.world().get::<Transform>(actor).unwrap().translation.y,
        9.0
    ); // 不能穿过失去覆盖的最近子地表落到父图。
    {
        let mut regions = app
            .world_mut()
            .resource_mut::<InstanceResourceTable<TerrainCollisionRegion>>();
        let region = regions.values.get_mut(&source).unwrap();
        region.revision = 3;
        region
            .supported_bounds
            .push((Vec2::splat(-10.0), Vec2::splat(10.0)));
    }
    tick(&mut app);
    assert_y(&app, actor, 6.5);
}

#[test]
fn a_disabled_rigid_body_child_collider_cannot_become_the_ground() {
    let mut app = physics_app();
    app.world_mut().spawn((
        Transform::from_xyz(0.0, -0.5, 0.0),
        RigidBody::Static,
        Collider::cuboid(10.0, 1.0, 10.0),
    ));
    let body = app
        .world_mut()
        .spawn((Transform::from_xyz(0.0, 6.0, 0.0), RigidBody::Static))
        .id();
    app.world_mut().spawn((
        Transform::from_xyz(0.0, -0.5, 0.0),
        ChildOf(body),
        Collider::cuboid(10.0, 1.0, 10.0),
    ));
    tick(&mut app); // 先让高层Collider真实进入空间索引，再禁用其父刚体。
    app.world_mut().entity_mut(body).insert(RigidBodyDisabled);
    let actor = app
        .world_mut()
        .spawn((Transform::from_xyz(0.0, 9.0, 0.0), GroundSnap::new(0.5)))
        .id();
    tick(&mut app);
    assert_y(&app, actor, 0.5);
    app.world_mut()
        .entity_mut(body)
        .remove::<RigidBodyDisabled>();
    app.world_mut()
        .get_mut::<GroundSnap>(actor)
        .unwrap()
        .request();
    tick(&mut app);
    assert_y(&app, actor, 6.5); // 恢复后同一Collider重新可见，不永久排除合法地面。
}

#[test]
fn explicit_query_mask_works_without_any_game_entity_classification() {
    let mut app = physics_app();
    let high = LayerMask(1 << 12);
    let low = LayerMask(1 << 13);
    app.world_mut().spawn((
        Transform::from_xyz(0.0, 5.5, 0.0),
        RigidBody::Static,
        Collider::cuboid(10.0, 1.0, 10.0),
        CollisionLayers::new(high, LayerMask::ALL),
    ));
    app.world_mut().spawn((
        Transform::from_xyz(0.0, 0.5, 0.0),
        RigidBody::Static,
        Collider::cuboid(10.0, 1.0, 10.0),
        CollisionLayers::new(low, LayerMask::ALL),
    ));
    tick(&mut app);
    let actor = app
        .world_mut()
        .spawn((
            Transform::from_xyz(0.0, 9.0, 0.0),
            GroundSnap::new(0.5),
            GroundSnapFilter {
                mask: low,
                ..default()
            },
        ))
        .id();
    tick(&mut app);
    assert_y(&app, actor, 1.5);
}
