//! 真实Avian树验证隐藏门及多层平移旋转后的碰撞位置。
#[path = "collider_hierarchy_tests.rs"]
mod collider_hierarchy_tests;

use super::*;
use avian3d::prelude::*;
use bevy::{asset::AssetPlugin, ecs::system::RunSystemOnce};
use feathrise_scene_core::residency::{initialize_instance_context, SceneInstanceOwner};

fn hits(query: SpatialQuery, active: feathrise_grounding::GroundSurfaceFilter) -> [Option<Entity>; 2] {
    [Vec3::new(10.0, 10.0, -3.0), Vec3::new(20.0, 10.0, -3.0)].map(|origin|
        query.cast_ray_predicate(origin, Dir3::NEG_Y, 20.0, true, &SpatialQueryFilter::default(), &|entity| active.is_active(entity)).map(|hit| hit.entity))
}
#[test]
fn source_advance_and_pose_schedules_initialize_without_ecs_access_conflicts() {
    let mut world = World::new();
    // 独立实例的组合调度也须验证SystemParam；根场景调度通过不能替代这里的B0001/B0002检查。
    schedules::advance(None).initialize(&mut world).expect("子场景完整域调度应可初始化");
    schedules::follow().initialize(&mut world).expect("子场景整体变换调度应可初始化");
}

#[test]
fn retirement_accepts_earlier_scene_exit_and_overlapping_detached_roots_with_fixed_budget() {
    let mut world = World::new();
    let parent = world.spawn_empty().id();
    let child = world.spawn(ChildOf(parent)).id();
    let grandchild = world.spawn(ChildOf(child)).id();
    let live = world.spawn_empty().id();
    world.entity_mut(parent).despawn(); // 模拟DespawnOnExit先于后台退役完成，整棵树已离场。
    let mut cleanup = vec![live, parent, child, grandchild, child];
    driver::cleanup_entities(&mut world, &mut cleanup, 2);
    assert_eq!(cleanup, [live, parent, child]); // 过期项也计入固定预算，不同步清空任意大队列。
    assert!(world.get_entity(live).is_ok());
    driver::cleanup_entities(&mut world, &mut cleanup, 3);
    assert!(cleanup.is_empty());
    assert!(world.get_entity(live).is_err());

    let root = world.spawn_empty().id();
    let leaf = world.spawn(ChildOf(root)).id();
    let mut cleanup = vec![leaf, root, root]; // 根重复登记、脱离根与显示树相交也只能销毁一次。
    driver::cleanup_entities(&mut world, &mut cleanup, 5);
    assert!(cleanup.is_empty());
    assert!(world.get_entity(root).is_err() && world.get_entity(leaf).is_err());
}

#[test]
fn surface_gpu_feedback_is_adopted_only_for_the_current_instance_and_generation() {
    use feathrise_environment::terrain_surface::{TerrainSurfaceInstanceFeedback, TerrainSurfaceRenderStats, TerrainSurfacePhase};
    use feathrise_scene_core::residency::InstanceResourceTable;
    let mut world = World::new();
    let source = world.spawn_empty().id();
    let other = world.spawn_empty().id();
    let current = TerrainSurfaceRenderStats { generation: 3, phase: TerrainSurfacePhase::Warming, ..default() };
    let mut table = InstanceResourceTable::<TerrainSurfaceRenderStats>::default();
    table.values.insert(source, current);
    world.insert_resource(table);
    world.insert_resource(TerrainSurfaceRenderStats { generation: 99, ..default() });
    world.insert_resource(TerrainSurfaceInstanceFeedback(vec![(source, TerrainSurfaceRenderStats { generation: 2, pipeline_ready: 1, ..default() }),
        (other, TerrainSurfaceRenderStats { generation: 3, pipeline_ready: 1, ..default() })]));
    readiness::refresh_surface_gpu_evidence(&mut world, source);
    assert_eq!(*readiness::get::<TerrainSurfaceRenderStats>(&world, source), current);
    world.resource_mut::<TerrainSurfaceInstanceFeedback>().0 = vec![(source, TerrainSurfaceRenderStats { generation: 3, pipeline_ready: 1, carrier_instances: 1, ..default() })];
    readiness::refresh_surface_gpu_evidence(&mut world, source);
    let stats = readiness::get::<TerrainSurfaceRenderStats>(&world, source);
    assert_eq!(stats.pipeline_ready, 1);
    assert_eq!(stats.phase, TerrainSurfacePhase::Warming); // GPU只能发布证据，不能绕过正式采用状态机。
    assert_eq!(world.resource::<TerrainSurfaceRenderStats>().generation, 99);
    assert_eq!(world.resource::<TerrainSurfaceRenderStats>().pipeline_ready, 0); // 父图保留自己的反馈。
}

#[test]
fn hidden_candidate_static_collision_moves_with_nested_instance() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), TransformPlugin,
        bevy::mesh::MeshPlugin, PhysicsPlugins::default()))
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_secs_f64(1.0 / 60.0)))
        .register_instance_resource::<feathrise_scene_core::residency::SceneRuntimeScope>()
        .add_observer(physics::consolidate_static_bodies)
        .add_observer(physics::repair_collider_body)
        .add_observer(physics::consolidate_late_owned_bodies);
    app.finish();
    let batch = app.world_mut().spawn((components::SubsceneBatch, Transform::IDENTITY, RigidBody::Static,
        RigidBodyDisabled, bevy_tnua::TnuaNotPlatform)).id();
    let source = app.world_mut().spawn((Transform::from_xyz(10.0, 0.0, 0.0)
        .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)), ChildOf(batch))).id();
    app.world_mut().entity_mut(source).insert(SceneInstanceOwner(source));
    initialize_instance_context(app.world_mut(), source);
    app.world_mut().resource_mut::<feathrise_scene_core::residency::SceneRuntimeScope>().instance = Some(source);
    let nested = app.world_mut().spawn((Transform::from_xyz(3.0, 0.0, 0.0), ChildOf(source))).id();
    let collider = app.world_mut().spawn((Transform::IDENTITY, Collider::cuboid(2.0, 1.0, 2.0),
        ChildOf(nested), RigidBody::Static)).id();
    app.world_mut().resource_mut::<feathrise_scene_core::residency::SceneRuntimeScope>().instance = None;
    let late = app.world_mut().spawn((Transform::from_xyz(0.0, 0.0, 5.0),
        Collider::cuboid(1.0, 1.0, 1.0), RigidBody::Static)).id();
    assert!(app.world().get::<RigidBody>(late).is_some());
    app.world_mut().entity_mut(late).insert(ChildOf(nested)); // 模拟异步GLB先有刚体、后归属实例。
    for _ in 0..5 { app.update(); }
    assert!(app.world().get::<RigidBody>(collider).is_none());
    assert_eq!(app.world().get::<ColliderOf>(collider).unwrap().body, batch); // Tnua沿此真实刚体地址读取NotPlatform，无须逐叶开关。
    assert!(app.world().get::<RigidBody>(late).is_none());
    assert_eq!(app.world().get::<ColliderOf>(late).unwrap().body, batch);
    assert!(app.world().get::<bevy_tnua::TnuaNotPlatform>(batch).is_some());
    assert_eq!(app.world().get::<SceneInstanceOwner>(collider), Some(&SceneInstanceOwner(source)));
    assert_eq!(app.world_mut().run_system_once(hits).unwrap(), [None, None]);
    app.world_mut().entity_mut(batch).remove::<(RigidBodyDisabled, bevy_tnua::TnuaNotPlatform)>();
    for _ in 0..5 { app.update(); }
    assert_eq!(app.world_mut().run_system_once(hits).unwrap(), [Some(collider), None]);
    app.world_mut().entity_mut(source).get_mut::<Transform>().unwrap().translation.x = 20.0;
    for _ in 0..5 { app.update(); }
    assert_eq!(app.world_mut().run_system_once(hits).unwrap(), [None, Some(collider)]);
    app.world_mut().entity_mut(batch).insert((RigidBodyDisabled, bevy_tnua::TnuaNotPlatform));
    for _ in 0..5 { app.update(); }
    assert_eq!(app.world_mut().run_system_once(hits).unwrap(), [None, None]);
}

#[test]
fn moved_disabled_source_resumes_with_one_contact_island_registration() {
    use avian3d::{collision::contact_types::ContactPairFlags, schedule::PhysicsTime};
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), TransformPlugin,
        bevy::mesh::MeshPlugin, PhysicsPlugins::default()))
        .insert_resource(Gravity(Vec3::ZERO))
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_secs_f64(1.0 / 60.0)));
    app.finish();
    // 候选批次的碰撞先禁用；编辑器暂停中整体抬高，关闭编辑器后再形成真实动态接触。
    let batch = app.world_mut().spawn((Transform::from_xyz(0.0, -2.0, 0.0),
        RigidBody::Static, RigidBodyDisabled)).id();
    let floor = app.world_mut().spawn((Transform::IDENTITY,
        Collider::cuboid(10.0, 1.0, 10.0), ChildOf(batch))).id();
    let actor = app.world_mut().spawn((Transform::from_xyz(0.0, 0.75, 0.0),
        RigidBody::Dynamic, Collider::sphere(0.5))).id();
    for _ in 0..5 { app.update(); }
    assert_eq!(app.world().get::<ColliderOf>(floor).unwrap().body, batch);
    app.world_mut().resource_mut::<Time<Physics>>().pause();
    app.world_mut().entity_mut(batch).get_mut::<Transform>().unwrap().translation.y = 0.0;
    app.world_mut().entity_mut(batch).remove::<RigidBodyDisabled>();
    for _ in 0..3 { app.update(); }
    app.world_mut().resource_mut::<Time<Physics>>().unpause();
    let mut contact_seen = false;
    for _ in 0..12 {
        app.update();
        let graph = app.world().resource::<ContactGraph>();
        for pair in graph.iter_active_touching() {
            if ![pair.collider1, pair.collider2].contains(&floor)
                || ![pair.collider1, pair.collider2].contains(&actor) { continue; }
            contact_seen = true;
            assert!(pair.generates_constraints(), "恢复后必须真正求解来源碰撞");
            let constraints = app.world().resource::<avian3d::dynamics::solver::constraint_graph::ConstraintGraph>();
            assert!(!constraints.contact_handles(pair.contact_id).is_empty());
            // 首次接触已经加入物理岛，开始生成标记不能残留到下一步再次加入。
            assert!(!pair.flags.contains(ContactPairFlags::STARTED_GENERATING_CONSTRAINTS));
        }
    }
    assert!(contact_seen, "回归必须实际形成来源地面与动态角色的接触");
    assert!(app.world().get::<Position>(actor).unwrap().y > 0.9,
        "动态角色应被抬高后的地面推出，而非通过禁用碰撞规避重复登记");
}

#[test]
fn source_revision_rebuilds_contents_but_pose_keeps_instance_addresses() {
    use feathrise_scene_content::*;
    use std::{collections::BTreeMap, sync::Arc};
    let parent = SceneId::parse("MAP-parent").unwrap();
    let child = SceneId::parse("MAP-child").unwrap();
    let reference = SceneInstancePlacement { id: SceneInstanceId::parse("child").unwrap(),
        source_scene_id: child.clone(), label: "子场景".into(), transform: Default::default() };
    let source = |id: SceneId, references: Arc<[SceneInstancePlacement]>| Arc::new(SceneInstanceSource {terrains: None,
        scene_id: id, revision: 1, world_bounds: Rect::from_corners(Vec2::splat(-5.0), Vec2::splat(5.0)), instances: references });
    let inputs = BTreeMap::from([(parent.clone(), source(parent.clone(), Arc::from([reference]))),
        (child.clone(), source(child, Arc::from([])))]);
    let old: Arc<[ExpandedSceneInstance]> = expand_scene_instances(&parent, &inputs).unwrap().into();
    let mut moved = old.to_vec();
    moved[1].transform.translation[0] = 10.0;
    moved[1].local_transform = moved[1].transform;
    let same = planning::compare(Entity::PLACEHOLDER, 2, old.clone(), moved.into());
    assert!(!same.structural);
    assert!(same.compatible(Entity::PLACEHOLDER, 3, 0)); // 下一鼠标帧已经到来，也要显示上一完整姿态。
    assert!(!same.compatible(Entity::PLACEHOLDER, 3, 1)); // 新来源/拓扑世代则必须拒绝。
    assert_eq!(same.poses.into_iter().collect::<Vec<_>>(), vec![1]);
    let mut changed = old.to_vec(); changed[1].source_revision = 2;
    let replacement = planning::compare(Entity::PLACEHOLDER, 3, old.clone(), changed.into());
    assert!(replacement.structural);
    assert!(!replacement.compatible(Entity::PLACEHOLDER, 4, 0));
    let mut root_save = old.to_vec(); root_save[0].source_revision = 2;
    assert!(!planning::compare(Entity::PLACEHOLDER, 4, old, root_save.into()).structural);
}
