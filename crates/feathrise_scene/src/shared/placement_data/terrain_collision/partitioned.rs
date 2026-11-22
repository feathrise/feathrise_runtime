//! 源 GLB 多形状代理相关的分帧登记、整体切换和清理。

use super::{
    components::{
        PartitionColliderBody, PartitionColliderPreparing, PartitionColliderReady,
        PlacementColliderOwner,
    },
    types::PartitionInstall,
    *,
};
use avian3d::prelude::{ColliderOf, Position, RigidBodyDisabled, Rotation};
use bevy_tnua::TnuaNotPlatform;
use feathrise_scene_core::residency::{SceneInstanceDetached, SceneInstanceOwner};

// 同一帧最多登记或回收少量已烘焙形状；不随模型节点数扩大主线程工作批次。
const SHAPES_PER_FRAME: usize = 32;

pub(super) fn prepare(
    install: &mut PartitionInstall,
    commands: &mut Commands,
    global: GlobalTransform,
) -> Option<PartitionColliderReady> {
    if !install.shapes.is_empty() {
        let root = install.root;
        let generation = install.generation;
        let body = *install.body.get_or_insert_with(|| {
            let (_, rotation, translation) = global.to_scale_rotation_translation();
            let pose = Transform::from_translation(translation).with_rotation(rotation);
            let body = commands.spawn_empty().id();
            commands.queue(move |world: &mut World| {
                if world
                    .get::<TerrainPatchCollisionRequest>(root)
                    .is_none_or(|request| request.generation != generation)
                {
                    world.despawn(body);
                    return;
                }
                let owner = world.get::<SceneInstanceOwner>(root).copied();
                let Ok(mut entity) = world.get_entity_mut(body) else {
                    return;
                };
                if let Some(owner) = owner {
                    entity.insert(owner);
                }
                // 已是世界位姿。保留真实场景归属但避免被子场景的自动挂接再次变换。
                entity.insert((
                    Name::new("GLB 网格物理"),
                    PartitionColliderBody,
                    SceneInstanceDetached,
                    RigidBody::Static,
                    RigidBodyDisabled,
                    TnuaNotPlatform,
                    pose,
                    GlobalTransform::from(pose),
                    Position(translation),
                    Rotation(rotation),
                ));
                world
                    .entity_mut(root)
                    .insert(PartitionColliderPreparing(body));
            });
            body
        });
        let placement = install.root;
        for _ in 0..SHAPES_PER_FRAME {
            let Some((collider, lod)) = install.shapes.pop_front() else {
                break;
            };
            // 两个关联都有明确作用：本地恒等位姿随共同刚体，物理归属也是同一禁用门。
            // 先给 Avian 精确单位尺度，再让 Transform 传播恢复世界位姿。
            commands.queue(move |world: &mut World| {
                if world.get::<PartitionColliderBody>(body).is_some() {
                    world.spawn((
                        TerrainPhysicsProxy,
                        PlacementColliderOwner { placement, lod },
                        Transform::IDENTITY,
                        GlobalTransform::IDENTITY,
                        ChildOf(body),
                        ColliderOf { body },
                        collider,
                    ));
                } else {
                    world
                        .resource_mut::<TerrainCollisionGarbage>()
                        .retain_shape(&collider);
                }
            });
        }
    }
    install.shapes.is_empty().then_some(PartitionColliderReady {
        body: install.body,
        generation: install.generation,
        scale: install.scale,
    })
}

pub(super) fn install(
    commands: &mut Commands,
    root: Entity,
    global: GlobalTransform,
    ready: PartitionColliderReady,
) {
    commands.queue(move |world: &mut World| {
        let current = world
            .get::<TerrainPatchCollisionRequest>(root)
            .is_some_and(|request| ready.matches(request))
            && world
                .get::<PartitionColliderReady>(root)
                .is_some_and(|current| {
                    current.body == ready.body && current.generation == ready.generation
                })
            && ready
                .body
                .is_none_or(|body| world.get::<PartitionColliderBody>(body).is_some());
        if !current {
            return;
        }
        let previous = world.get::<TerrainColliderProxy>(root).map(|proxy| proxy.0);
        if previous != ready.body {
            if let Some(previous) = previous {
                sync_proxy_participation(world, previous, false);
            }
            world.entity_mut(root).remove::<TerrainColliderProxy>();
        }
        if let Some(body) = ready.body {
            let (_, rotation, translation) = global.to_scale_rotation_translation();
            let pose = Transform::from_translation(translation).with_rotation(rotation);
            world.entity_mut(body).insert((
                pose,
                GlobalTransform::from(pose),
                Position(translation),
                Rotation(rotation),
            ));
            let enabled = world.get::<AuthorCollisionSuppression>(root).is_none();
            set_enabled(world, body, enabled);
            if previous == Some(body) {
                world.get_mut::<TerrainColliderProxy>(root).unwrap().1 = global.to_matrix();
            } else {
                world
                    .entity_mut(root)
                    .insert(TerrainColliderProxy(body, global.to_matrix()));
            }
        }
    });
}

pub(super) fn set_enabled(world: &mut World, body: Entity, enabled: bool) {
    // 与内置网格候选使用相同的物理和角色承托开关，分组之间不需要重复刚体。
    if world.get::<PartitionColliderBody>(body).is_some()
        && world.get::<RigidBodyDisabled>(body).is_some() == enabled
    {
        super::super::builtin::set_builtin_placement_batch_enabled_world(world, body, enabled);
    }
}

pub(super) fn retire(commands: &mut Commands, body: Entity) {
    commands.queue(move |world: &mut World| retire_body(world, body));
}

fn retire_body(world: &mut World, body: Entity) {
    if world.get::<PartitionColliderBody>(body).is_none() {
        return;
    }
    set_enabled(world, body, false); // 退役帧立即归还承托，之后可慢慢清理形状。
    world.entity_mut(body).remove::<PartitionColliderBody>(); // 去重重复丢弃通知。
    world
        .resource_mut::<TerrainCollisionGarbage>()
        .bodies
        .push_back(body);
}

pub(super) fn cleanup(
    commands: &mut Commands,
    garbage: &mut TerrainCollisionGarbage,
    children: &Query<&Children>,
) {
    let mut remaining = SHAPES_PER_FRAME;
    while remaining != 0 {
        let Some(body) = garbage.bodies.front().copied() else {
            return;
        };
        if let Ok(children) = children.get(body)
            && !children.is_empty()
        {
            for child in children.iter().take(remaining) {
                commands.entity(child).try_despawn();
            }
            return; // 要等删除命令真正落地才可回收父刚体，避免链接组件一次性递归删除。
        }
        garbage.bodies.pop_front();
        commands.entity(body).try_despawn();
        remaining -= 1;
    }
}

pub(in crate::shared::placement_data) fn discard_ready(
    event: On<Discard<PartitionColliderReady>>,
    ready: Query<(&PartitionColliderReady, Option<&TerrainColliderProxy>)>,
    mut commands: Commands,
) {
    if let Ok((ready, active)) = ready.get(event.entity)
        && let Some(body) = ready.body
        && active.is_none_or(|proxy| proxy.0 != body)
    {
        retire(&mut commands, body); // 已装入物理世界的那一代由 TerrainColliderProxy 负责回收。
    }
}

pub(in crate::shared::placement_data) fn discard_preparing(
    event: On<Discard<PartitionColliderPreparing>>,
    roots: Query<(&PartitionColliderPreparing, Option<&PartitionColliderReady>)>,
    mut commands: Commands,
) {
    if let Ok((preparing, ready)) = roots.get(event.entity)
        && ready.is_none_or(|ready| ready.body != Some(preparing.0))
    {
        retire(&mut commands, preparing.0);
    }
}
