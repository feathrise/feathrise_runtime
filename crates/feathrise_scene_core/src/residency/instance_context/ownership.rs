//! 生成命令与异步GLB层级均继承实例归属；不依赖异步完成时的当前编辑地图。

use bevy::{ecs::system::SystemParam, prelude::*};
use super::{MapInstanceOwner, MapRuntimeScope};
use std::collections::VecDeque;

/// GLB先完整实例化内部层级、再挂到地图根；补传归属必须按子节点限额推进。
#[derive(Resource, Default)]
struct PendingInheritance(VecDeque<(Entity, Entity, usize)>);
const INHERITANCE_BUDGET: usize = 256;

fn propagate_inherited_owners(mut pending: ResMut<PendingInheritance>,
    owners: Query<&MapInstanceOwner>, children: Query<&Children>, mut commands: Commands) {
    for _ in 0..INHERITANCE_BUDGET {
        let Some((entity, instance, next)) = pending.0.front_mut() else { break; };
        if !owners.get(*entity).is_ok_and(|owner| owner.0 == *instance) { pending.0.pop_front(); continue; }
        let Some(child) = children.get(*entity).ok().and_then(|children| children.get(*next)).copied() else {
            pending.0.pop_front(); continue;
        };
        *next += 1;
        if !owners.contains(child) { commands.entity(child).try_insert(MapInstanceOwner(*instance)); }
        // 子节点的新Owner观察者继续排入其后代；显式嵌套实例的不同Owner不会被父图覆盖。
    }
}

#[derive(SystemParam)]
pub struct EntityMapScope<'w, 's> {
    owners: Query<'w, 's, &'static MapInstanceOwner>,
    parents: Query<'w, 's, &'static ChildOf>,
    detached: Query<'w, 's, (), With<super::MapInstanceDetached>>,
    scope: Option<Res<'w, MapRuntimeScope>>,
}
impl EntityMapScope<'_, '_> {
    pub fn instance(&self, entity: Entity) -> Option<Entity> {
        let mut cursor = entity;
        for _ in 0..256 { // GLB内部层级与地图引用深度分别受控，异常树不会无界回溯。
            if let Ok(owner) = self.owners.get(cursor) { return Some(owner.0); }
            let Ok(parent) = self.parents.get(cursor) else { break; };
            cursor = parent.parent();
        }
        self.scope.as_deref().and_then(|scope| scope.instance)
    }
}

fn tag_transformed_entity(add: On<Add, Transform>, context: EntityMapScope, mut commands: Commands) {
    tag(add.entity, &context, &mut commands);
}
fn tag_parented_entity(add: On<Insert, ChildOf>, context: EntityMapScope, mut commands: Commands) {
    tag(add.entity, &context, &mut commands);
}

fn track_detached(add: On<Add, super::MapInstanceDetached>, context: EntityMapScope,
    mut roots: ResMut<super::InstanceResourceTable<super::MapDetachedRoots>>) {
    if let Some(instance) = context.instance(add.entity) {
        if let Some(roots) = roots.values.get_mut(&instance) { roots.0.push(add.entity); }
    }
}
fn tag(entity: Entity, context: &EntityMapScope, commands: &mut Commands) {
    if context.owners.contains(entity) { return; }
    let Some(instance) = context.instance(entity) else { return; };
    commands.entity(entity).try_insert(MapInstanceOwner(instance));
    if !context.parents.contains(entity) && !context.detached.contains(entity) && entity != instance {
        commands.entity(entity).try_insert(ChildOf(instance));
    } // 域自己的根挂到子地图根，源图内部Transform保持原样。
}

fn index_owner(insert: On<Insert, MapInstanceOwner>, owners: Query<&MapInstanceOwner>,
    children: Query<&Children>, mut pending: ResMut<PendingInheritance>,
    mut index: ResMut<super::query::OwnershipIndex>) {
    if let Ok(owner) = owners.get(insert.entity) {
        index.insert(insert.entity, owner.0);
        if children.get(insert.entity).is_ok_and(|children| !children.is_empty()) {
            pending.0.push_back((insert.entity, owner.0, 0));
        }
    }
}
fn unindex_owner(remove: On<Remove, MapInstanceOwner>, mut index: ResMut<super::query::OwnershipIndex>) {
    index.remove(remove.entity);
}
#[derive(Resource)]
struct OwnershipInstalled;
pub(super) fn install(app: &mut App) {
    if app.world().contains_resource::<OwnershipInstalled>() { return; }
    app.insert_resource(OwnershipInstalled);
    use crate::residency::SceneResidencyAppExt;
    app.init_resident_resource::<super::query::OwnershipIndex>()
        .init_resident_resource::<PendingInheritance>()
        .add_resident_systems(PreUpdate, propagate_inherited_owners)
        .add_observer(index_owner).add_observer(unindex_owner);
    use super::MapInstanceContextAppExt;
    app.register_instance_resource::<super::MapDetachedRoots>()
        .add_observer(tag_transformed_entity).add_observer(tag_parented_entity).add_observer(track_detached);
}
