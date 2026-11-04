//! 父图与同源两个子实例分别读写、驻留恢复与上下文退役的真实ECS回归。

use super::*;
use crate::residency::{take_resident_resources, restore_resident_resources};
use bevy::prelude::*;

#[derive(Resource, Default)]
struct State { population: Vec<u32>, ticket: u64 }

fn advance(mut state: InstanceResMut<State>) {
    state.ticket += 1;
    let ticket = state.ticket as u32;
    state.population.push(ticket);
}

#[test]
fn two_instances_keep_separate_population_and_root_never_changes() {
    let mut app = App::new();
    app.init_instance_resource::<State>().add_systems(Update, advance);
    let a = app.world_mut().spawn_empty().id();
    let b = app.world_mut().spawn_empty().id();
    initialize_instance_context(app.world_mut(), a);
    initialize_instance_context(app.world_mut(), b);
    app.world_mut().resource_mut::<MapRuntimeScope>().instance = Some(a);
    app.update();
    app.update();
    app.world_mut().resource_mut::<MapRuntimeScope>().instance = Some(b);
    app.update();
    app.world_mut().resource_mut::<MapRuntimeScope>().instance = None;
    assert_eq!(app.world().resource::<State>().ticket, 0);
    let instances = app.world().resource::<InstanceResourceTable<State>>();
    assert_eq!(instances.values[&a].population, [1, 2]);
    assert_eq!(instances.values[&b].population, [1]);
    app.update();
    assert_eq!(app.world().resource::<State>().ticket, 1);
}

#[test]
fn residency_moves_child_states_without_resetting_tickets_or_population() {
    let mut app = App::new();
    app.init_instance_resource::<State>();
    let a = app.world_mut().spawn_empty().id();
    initialize_instance_context(app.world_mut(), a);
    app.world_mut().resource_mut::<InstanceResourceTable<State>>().values.get_mut(&a).unwrap().ticket = 71;
    let mut snapshot = take_resident_resources(app.world_mut());
    assert!(app.world().resource::<InstanceResourceTable<State>>().values.is_empty());
    restore_resident_resources(app.world_mut(), &mut snapshot);
    assert_eq!(app.world().resource::<InstanceResourceTable<State>>().values[&a].ticket, 71);
    let retired = retire_instance_context(app.world_mut(), a);
    assert_eq!(retired.len(), 2); // 独立状态与脱离显示树的预热根注册表一同退役。
    assert!(app.world().resource::<InstanceResourceTable<State>>().values.is_empty());
}

#[test]
fn source_frame_keeps_parent_world_and_child_queries_consistent() {
    let transform = Transform::from_xyz(100.0, 12.0, 20.0).with_rotation(Quat::from_rotation_y(90.0_f32.to_radians()));
    let frame = MapSourceFrame::from_transform(transform);
    let source = Vec3::new(2.0, 3.0, 4.0);
    let world = frame.to_world(source);
    assert!(world.abs_diff_eq(Vec3::new(104.0, 15.0, 18.0), 0.00001));
    assert!(frame.to_local(world).abs_diff_eq(source, 0.00001));
}

#[derive(Component)]
struct Count(u32);
fn increment_owned(mut values: InstanceQuery<&mut Count>) {
    for mut value in &mut values { value.0 += 1; }
}
#[test]
fn indexed_mutable_queries_follow_owner_changes_and_skip_other_instances() {
    let mut app = App::new();
    app.init_instance_resource::<State>().add_systems(Update, increment_owned);
    let a = app.world_mut().spawn_empty().id(); let b = app.world_mut().spawn_empty().id();
    let first = app.world_mut().spawn((Count(0), MapInstanceOwner(a))).id();
    let second = app.world_mut().spawn((Count(0), MapInstanceOwner(b))).id();
    let root = app.world_mut().spawn(Count(0)).id();
    app.world_mut().resource_mut::<MapRuntimeScope>().instance = Some(a);
    app.update();
    assert_eq!(app.world().get::<Count>(first).unwrap().0, 1);
    assert_eq!(app.world().get::<Count>(second).unwrap().0, 0);
    assert_eq!(app.world().get::<Count>(root).unwrap().0, 0);
    app.world_mut().entity_mut(first).insert(MapInstanceOwner(b));
    app.update();
    assert_eq!(app.world().get::<Count>(first).unwrap().0, 1);
    app.world_mut().resource_mut::<MapRuntimeScope>().instance = Some(b);
    app.update();
    assert_eq!(app.world().get::<Count>(first).unwrap().0, 2);
    assert_eq!(app.world().get::<Count>(second).unwrap().0, 1);
    app.world_mut().entity_mut(first).remove::<MapInstanceOwner>();
    app.world_mut().resource_mut::<MapRuntimeScope>().instance = None;
    app.update();
    assert_eq!(app.world().get::<Count>(first).unwrap().0, 3);
    assert_eq!(app.world().get::<Count>(root).unwrap().0, 1);
}

#[test]
fn a_completed_gltf_hierarchy_attached_late_inherits_source_with_bounded_fanout() {
    let mut app = App::new();
    app.init_instance_resource::<State>().add_systems(Update, increment_owned);
    let source = app.world_mut().spawn(MapInstanceOwner(Entity::PLACEHOLDER)).id();
    app.world_mut().entity_mut(source).insert(MapInstanceOwner(source));
    let scene = app.world_mut().spawn(Transform::IDENTITY).id();
    let branch = app.world_mut().spawn((Transform::IDENTITY, ChildOf(scene), Count(0))).id();
    let leaf = app.world_mut().spawn((Transform::IDENTITY, ChildOf(branch), Count(0))).id();
    let many: Vec<_> = (0..600).map(|_| app.world_mut().spawn((Transform::IDENTITY, ChildOf(scene), Count(0))).id()).collect();
    assert!(app.world().get::<MapInstanceOwner>(leaf).is_none());
    app.world_mut().entity_mut(scene).insert(ChildOf(source)); // WorldAssetSpawner在复制完整层级之后才建立这一关系。
    app.world_mut().resource_mut::<MapRuntimeScope>().instance = Some(source);
    app.update();
    assert!(many.iter().filter(|&&entity| app.world().get::<MapInstanceOwner>(entity).is_some()).count() < 600);
    for _ in 0..8 { app.update(); }
    for entity in [branch, leaf].into_iter().chain(many) {
        assert_eq!(app.world().get::<MapInstanceOwner>(entity), Some(&MapInstanceOwner(source)));
        assert!(app.world().get::<Count>(entity).unwrap().0 > 0, "正式索引查询应识别GLB后代");
    }
    app.world_mut().resource_mut::<MapRuntimeScope>().instance = None;
    let before = app.world().get::<Count>(leaf).unwrap().0;
    app.update();
    assert_eq!(app.world().get::<Count>(leaf).unwrap().0, before, "父图不能反向读写子图成员");
}
