//! 完整候选的静态碰撞共用批次刚体；切换一个活动门即可同时切换整套地形和建筑。

use avian3d::prelude::*;
use bevy::prelude::*;
use feathrise_scene_core::residency::EntityMapScope;

pub(super) fn consolidate_static_bodies(add: On<Add<RigidBody>>,
    scope: EntityMapScope, bodies: Query<&RigidBody>, parents: Query<&ChildOf>, mut commands: Commands) {
    if scope.instance(add.entity).is_some() && bodies.get(add.entity) == Ok(&RigidBody::Static) {
        // 源地图域仍按原组合生成；子实例叶子的Collider由最近的批次静态刚体承接。
        commands.entity(add.entity).remove::<RigidBody>();
        // Avian移除嵌套刚体只清理旧ColliderOf，重新插入原层级关系才会寻找剩余最近刚体。
        if let Ok(parent) = parents.get(add.entity) { commands.entity(add.entity).insert(ChildOf(parent.parent())); }
    }
}

pub(super) fn repair_collider_body(insert: On<Insert<ColliderOf>>, scope: EntityMapScope,
    bodies: Query<(&RigidBody, Has<super::components::SubMapBatch>)>,
    parents: Query<&ChildOf>, colliders: Query<&ColliderOf>, mut commands: Commands) {
    if scope.instance(insert.entity).is_none()
        || bodies.get(insert.entity).is_ok_and(|(body, _)| *body != RigidBody::Static) { return; }
    // Avian的添加观察者可能迟于移除命令写回旧叶刚体地址；以最终ColliderOf事件校正，不依赖观察者执行顺序。
    let body = parents.iter_ancestors(insert.entity).find(|&ancestor|
        bodies.get(ancestor).is_ok_and(|(body, batch)| *body != RigidBody::Static || batch));
    // 批次必须按显式标记识别；实例调度的scope后备值会把无Owner的批次也报告为当前实例。
    if let Some(body) = body && colliders.get(insert.entity).is_ok_and(|current| current.body != body) {
        commands.entity(insert.entity).insert(ColliderOf { body });
    }
}

pub(super) fn consolidate_late_owned_bodies(insert: On<Insert<feathrise_scene_core::residency::MapInstanceOwner>>,
    bodies: Query<&RigidBody>, parents: Query<&ChildOf>, mut commands: Commands) {
    if bodies.get(insert.entity) == Ok(&RigidBody::Static) {
        // 异步模型可能先生成刚体、后建立所属层级；补齐Owner时仍必须转交完整批次。
        commands.entity(insert.entity).remove::<RigidBody>();
        if let Ok(parent) = parents.get(insert.entity) { commands.entity(insert.entity).insert(ChildOf(parent.parent())); }
    }
}
