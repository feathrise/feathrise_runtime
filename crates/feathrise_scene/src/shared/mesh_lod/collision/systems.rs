//! 有界静态碰撞来源登记、派生信用和每物理步安全近区复核。
use super::super::{
    components::MeshLodAuthoringState,
    ownership::owner,
    resources::{CACHE_BYTES, TARGET_BUDGET},
};
use super::{
    build::{cook, source_mesh},
    components::DynamicFocus,
    resources::Runtime,
    types::*,
};
use avian3d::prelude::*;
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, futures::check_ready},
};
use feathrise_map_content::MeshCollisionLodSettings;
use std::sync::Arc;
pub(super) fn body_inserted(
    event: On<Insert<RigidBody>>,
    bodies: Query<&RigidBody>,
    mut commands: Commands,
) {
    if bodies
        .get(event.entity)
        .is_ok_and(|body| *body != RigidBody::Static)
    {
        commands.entity(event.entity).insert(DynamicFocus);
    } else {
        commands.entity(event.entity).remove::<DynamicFocus>();
    }
}

pub(super) fn collider_inserted(event: On<Insert<Collider>>, mut runtime: Option<ResMut<Runtime>>) {
    if let Some(runtime) = runtime.as_mut()
        && runtime.queued.insert(event.entity)
    {
        runtime.pending.push_back(event.entity);
    }
}

pub(super) fn source_inserted<T: Component>(
    event: On<Insert<T>>,
    mut runtime: Option<ResMut<Runtime>>,
) {
    if let Some(runtime) = runtime.as_mut() {
        runtime.discovery.push_back((event.entity, 0));
    }
}

fn release_reference(runtime: &mut Runtime, key: Key) {
    if let Some(count) = runtime.references.get_mut(&key) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            runtime.references.remove(&key);
            runtime.failed.remove(&key);
        }
    }
}

fn static_body(world: &World, entity: Entity) -> bool {
    let body = world
        .get::<ColliderOf>(entity)
        .filter(|owner| world.get::<RigidBody>(owner.body).is_some())
        .map_or(entity, |owner| owner.body);
    world.get::<RigidBody>(body) == Some(&RigidBody::Static)
        && world.get::<RigidBodyDisabled>(body).is_none()
        && world.get::<ColliderDisabled>(entity).is_none()
        && world.get::<Sensor>(entity).is_none()
}

fn global(world: &World) -> Option<MeshCollisionLodSettings> {
    world
        .get_resource::<feathrise_settings::RenderSettings>()
        .map(|settings| settings.mesh_lod.sanitized())
        .filter(|settings| settings.collision_enabled)
        .map(|settings| MeshCollisionLodSettings {
            safe_near_meters: settings.collision_safe_near_meters,
            max_error_meters: settings.collision_max_error_meters,
        })
}

fn foci(world: &mut World) -> Option<Vec<Focus>> {
    let mut bodies = world.query_filtered::<(
        &RigidBody,
        Option<&GlobalTransform>,
        Option<&Position>,
        Option<&Collider>,
        Option<&LinearVelocity>,
    ), (With<DynamicFocus>, Without<RigidBodyDisabled>)>();
    let mut foci = Vec::new();
    for (body, pose, position, collider, velocity) in bodies.iter(world).take(65) {
        if *body == RigidBody::Static {
            continue;
        }
        // 复杂复合/分离碰撞角色无法证明根半径时保原，不猜测安全焦点。
        let collider = collider?;
        let bounds = collider.shape_scaled().compute_local_aabb();
        let radius = bounds.center().length() + bounds.half_extents().length();
        let center = position
            .map(|position| position.0)
            .or_else(|| pose.map(GlobalTransform::translation))?;
        let travel = velocity.map_or(0.0, |velocity| velocity.0.length());
        if !center.is_finite() || !radius.is_finite() || !travel.is_finite() || foci.len() == 64 {
            return None;
        }
        foci.push(Focus {
            center,
            radius,
            travel,
        });
    }
    (!foci.is_empty()).then_some(foci)
}

fn safe(
    world: &World,
    entity: Entity,
    original: &Collider,
    foci: Option<&[Focus]>,
    policy: MeshCollisionLodSettings,
) -> bool {
    if !static_body(world, entity) || world.resource::<MeshLodAuthoringState>().active {
        return false;
    }
    // 任何宽相接触边都保原，包含睡眠接触，不等待碰撞已发生后才恢复。
    if world
        .get_resource::<ContactGraph>()
        .is_some_and(|graph| graph.contact_edges_with(entity).next().is_some())
    {
        return false;
    }
    let Some(foci) = foci else {
        return false;
    };
    let Some(pose) = world.get::<GlobalTransform>(entity) else {
        return false;
    };
    let bounds = original.shape_scaled().compute_local_aabb();
    let (_, rotation, translation) = pose.to_scale_rotation_translation();
    let center = translation + rotation * bounds.center();
    let radius = bounds.half_extents().length();
    center.is_finite()
        && radius.is_finite()
        && foci.iter().all(|focus| {
            center.distance(focus.center)
                > radius + focus.radius + policy.safe_near_meters + focus.travel
        })
}

/// 预烘焙物理代理自身始终精确单位缩放；不能让旋转分解舍入触发插入hook同步重构。
pub(super) fn adopt_collider(world: &mut World, entity: Entity, collider: Collider) {
    let proxy_pose = world
        .get::<crate::shared::placement_data::TerrainPhysicsProxy>(entity)
        .and_then(|_| world.get::<GlobalTransform>(entity))
        .copied();
    if proxy_pose.is_some() {
        world.entity_mut(entity).insert(GlobalTransform::IDENTITY);
    }
    world.entity_mut(entity).insert(collider);
    if let Some(pose) = proxy_pose {
        world.entity_mut(entity).insert(pose);
    }
    // 独占World操作内立即恢复渲染位姿，既有Position/Rotation承托身份始终不变。
}

fn replace(
    world: &mut World,
    runtime: &mut Runtime,
    entity: Entity,
    target: &mut Target,
    level: usize,
) {
    if level == target.applied {
        return;
    }
    if let Some(cache) = runtime.cache.get_mut(&target.key) {
        if target.applied != 0 {
            cache.pins = cache.pins.saturating_sub(1);
        }
        if level != 0 {
            cache.pins += 1;
        }
        if cache.pins == 0 && target.applied != 0 && level == 0 {
            runtime.credit_revision = runtime.credit_revision.wrapping_add(1);
        }
    }
    let collider = if level == 0 {
        target.original.clone()
    } else {
        runtime.cache[&target.key].levels[level - 1]
            .collider
            .clone()
    };
    adopt_collider(world, entity, collider); // 唯一既有碰撞点原子替换，不增加Collider、刚体或承托实体。
    target.applied = level;
    if level == 0 {
        runtime.coarse.remove(&entity);
    } else {
        runtime.coarse.insert(entity);
    }
}

/// 缩放换代只恢复canonical全拓扑，绝不能把Avian缩放后的派生网格认作新L0。
/// 该兼容分支沿既有Avian缩放路径；频繁运行时大形状缩放需后续独立物理代理。
fn recover_scaled_source(
    world: &mut World,
    runtime: &mut Runtime,
    entity: Entity,
    target: &mut Target,
    current: &Collider,
) -> bool {
    let internally_generated = runtime.cache.get(&target.key).is_some_and(|cache| {
        cache
            .levels
            .iter()
            .any(|level| key(&level.collider).shape == key(current).shape)
    });
    if !internally_generated || current.scale() == target.original.scale() {
        return false;
    }
    let mut full = target.original.clone();
    full.set_scale(current.scale(), 10); // 仅实际缩放换代走baseline恢复，不在普通LOD选择/轮转重算原BVH。
    if target.applied != 0
        && let Some(cache) = runtime.cache.get_mut(&target.key)
    {
        cache.pins = cache.pins.saturating_sub(1);
        if cache.pins == 0 {
            runtime.credit_revision = runtime.credit_revision.wrapping_add(1);
        }
    }
    runtime.coarse.remove(&entity);
    release_reference(runtime, target.key);
    target.original = full.clone();
    target.key = key(&full);
    target.applied = 0;
    target.rescale_guard = true; // 等正式后端重新登记完整新源，期间不再次简化。
    *runtime.references.entry(target.key).or_default() += 1;
    adopt_collider(world, entity, full);
    world
        .resource_mut::<super::super::MeshLodRuntimeStatus>()
        .detail = "碰撞缩放已恢复原始全拓扑；本来源等待完整新原形状".into();
    true
}

/// 所有正在简化的碰撞每个物理步都核查；32是应用信用而非作者总数限制。
pub(in crate::shared::mesh_lod) fn restore_unsafe(world: &mut World) {
    let global = global(world);
    let foci = if global.is_some() && !world.resource::<MeshLodAuthoringState>().active {
        foci(world)
    } else {
        None
    };
    world.resource_scope(|world, mut runtime: Mut<Runtime>| {
        let coarse = runtime.coarse.iter().copied().collect::<Vec<_>>(); // 集合最多32，固定小工作。
        for entity in coarse {
            let Some(mut target) = runtime.targets.remove(&entity) else {
                runtime.coarse.remove(&entity);
                continue;
            };
            if let Some(current) = world.get::<Collider>(entity).cloned()
                && recover_scaled_source(world, &mut runtime, entity, &mut target, &current)
            {
                runtime.targets.insert(entity, target);
                continue;
            }
            let unchanged = world.get::<Collider>(entity).is_some_and(|collider| {
                runtime
                    .cache
                    .get(&target.key)
                    .and_then(|cache| cache.levels.get(target.applied.saturating_sub(1)))
                    .is_some_and(|level| key(&level.collider) == key(collider))
            });
            let policy = owner(world, entity)
                .filter(|owner| owner.active && owner.root == target.owner && !owner.building)
                .and_then(|owner| owner.policy.resolve_collision(global));
            if unchanged
                && !policy.is_some_and(|policy| {
                    safe(world, entity, &target.original, foci.as_deref(), policy)
                        && runtime
                            .cache
                            .get(&target.key)
                            .and_then(|cache| cache.levels.get(target.applied - 1))
                            .is_some_and(|level| level.error <= policy.max_error_meters)
                })
            {
                replace(world, &mut runtime, entity, &mut target, 0);
            }
            if !unchanged {
                runtime.coarse.remove(&entity);
            }
            runtime.targets.insert(entity, target);
        }
    });
}

pub(super) fn drive(world: &mut World) {
    let global = global(world);
    let foci = if global.is_some() && !world.resource::<MeshLodAuthoringState>().active {
        foci(world)
    } else {
        None
    };
    world.resource_scope(|world, mut runtime: Mut<Runtime>| {
        if runtime.release.as_mut().and_then(check_ready).is_some() {
            runtime.release = None;
            let credit = std::mem::take(&mut runtime.release_credit);
            runtime.cache_bytes = runtime.cache_bytes.saturating_sub(credit);
            if credit != 0 {
                runtime.credit_revision = runtime.credit_revision.wrapping_add(1);
            }
        }
        if runtime.release.is_none()
            && let Some(retired) = runtime.retired.pop_front()
        {
            runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
                drop(retired);
            }));
        }
        for _ in 0..TARGET_BUDGET {
            let Some((entity, next)) = runtime.discovery.pop_front() else {
                break;
            };
            if next == 0 && runtime.queued.insert(entity) {
                runtime.pending.push_back(entity);
            }
            if let Some(child) = world
                .get::<Children>(entity)
                .and_then(|children| children.get(next))
                .copied()
            {
                runtime.discovery.push_back((entity, next + 1));
                runtime.discovery.push_back((child, 0));
            }
        }
        // 无来源引用的弱票据/派生冷缓存有界退役；原形状不被缓存强留。
        if runtime.release.is_none() {
            for _ in 0..TARGET_BUDGET {
                let Some(key) = runtime.cache_cursor.pop_front() else {
                    break;
                };
                if !runtime.references.contains_key(&key)
                    && runtime.cache.get(&key).is_some_and(|cache| cache.pins == 0)
                {
                    if let Some(cache) = runtime.cache.remove(&key) {
                        runtime.release_credit = cache.bytes;
                        runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
                            drop(cache);
                        }));
                    }
                    break;
                } else if runtime.cache.contains_key(&key) {
                    runtime.cache_cursor.push_back(key);
                }
            }
        }
        for _ in 0..TARGET_BUDGET {
            let Some(entity) = runtime.pending.pop_front() else {
                break;
            };
            runtime.queued.remove(&entity);
            let Some(owner) = owner(world, entity).filter(|owner| !owner.building) else {
                continue;
            };
            let Some(mut original) = world.get::<Collider>(entity).cloned() else {
                continue;
            };
            if let Some(mut target) = runtime.targets.remove(&entity) {
                if recover_scaled_source(world, &mut runtime, entity, &mut target, &original) {
                    runtime.targets.insert(entity, target);
                    continue;
                }
                let internal = key(&original) == target.key
                    || runtime.cache.get(&target.key).is_some_and(|cache| {
                        cache
                            .levels
                            .iter()
                            .any(|level| key(&level.collider) == key(&original))
                    });
                let same_owner = target.owner == owner.root;
                if internal && !same_owner {
                    original = target.original.clone(); // 换合法静态根也保留canonical，不能把当前LOD升级为L0。
                    adopt_collider(world, entity, original.clone());
                }
                runtime.targets.insert(entity, target);
                if internal && same_owner {
                    continue;
                }
            }
            if source_mesh(&original).is_none() {
                if let Some(old) = runtime.targets.remove(&entity) {
                    if old.applied != 0
                        && let Some(cache) = runtime.cache.get_mut(&old.key)
                    {
                        cache.pins = cache.pins.saturating_sub(1);
                    }
                    runtime.coarse.remove(&entity);
                    release_reference(&mut runtime, old.key);
                    runtime.retired.push_back(old);
                }
                continue; // 外部换为凸形/复合等不支持形状后撤销旧登记，不永远反复enqueue。
            }
            if let Some(old) = runtime.targets.remove(&entity) {
                if old.applied != 0
                    && let Some(cache) = runtime.cache.get_mut(&old.key)
                {
                    cache.pins = cache.pins.saturating_sub(1);
                }
                runtime.coarse.remove(&entity);
                release_reference(&mut runtime, old.key);
                runtime.retired.push_back(old);
            } else {
                runtime.cursor.push_back(entity);
            }
            *runtime.references.entry(key(&original)).or_default() += 1;
            runtime.targets.insert(
                entity,
                Target {
                    owner: owner.root,
                    key: key(&original),
                    original,
                    applied: 0,
                    rescale_guard: false,
                },
            );
        }
        if let Some((key, task)) = runtime.task.as_mut()
            && let Some((lease, result)) = check_ready(task)
        {
            runtime.completed = Some((*key, lease, result));
            runtime.task = None;
        }
        if let Some((key, lease, result)) = runtime.completed.take() {
            let needed = runtime.references.get(&key).copied().unwrap_or(0) != 0;
            if !needed {
                if runtime.release.is_none() {
                    runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
                        drop(result);
                    }));
                } else {
                    runtime.completed = Some((key, lease, result));
                }
            } else {
                match result {
                    Ok(built) if runtime.cache_bytes.saturating_add(built.bytes) <= CACHE_BYTES => {
                        runtime.cache_bytes += built.bytes;
                        runtime.cache.insert(
                            key,
                            Cache {
                                levels: built.levels,
                                bytes: built.bytes,
                                pins: 0,
                                _source: built.source,
                            },
                        );
                        runtime.cache_cursor.push_back(key);
                    }
                    Ok(built) => {
                        let mut released = false;
                        for _ in 0..TARGET_BUDGET {
                            let Some(cold) = runtime.cache_cursor.pop_front() else {
                                break;
                            };
                            if runtime
                                .cache
                                .get(&cold)
                                .is_some_and(|cache| cache.pins == 0)
                                && runtime.release.is_none()
                            {
                                if let Some(cache) = runtime.cache.remove(&cold) {
                                    runtime.release_credit = cache.bytes;
                                    runtime.release =
                                        Some(AsyncComputeTaskPool::get().spawn(async move {
                                            drop(cache);
                                        }));
                                    released = true;
                                }
                                break;
                            } else {
                                runtime.cache_cursor.push_back(cold);
                            }
                        }
                        if released || runtime.release.is_some() {
                            runtime.completed = Some((key, lease, Ok(built)));
                        } else {
                            let credit = runtime.credit_revision;
                            runtime.failed.insert(
                                key,
                                Failure {
                                    _source: lease,
                                    retry_credit: Some(credit),
                                },
                            );
                            runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
                                drop(built);
                            }));
                            world
                                .resource_mut::<super::super::MeshLodRuntimeStatus>()
                                .detail = "碰撞LOD缓存信用暂不足，保留原始碰撞".into();
                        }
                    }
                    Err(error) => {
                        runtime.failed.insert(
                            key,
                            Failure {
                                _source: lease,
                                retry_credit: None,
                            },
                        );
                        world
                            .resource_mut::<super::super::MeshLodRuntimeStatus>()
                            .detail = error.clone();
                        warn!("静态网格碰撞LOD保留原始碰撞：{error}");
                    }
                }
            }
        }
        for _ in 0..TARGET_BUDGET.min(runtime.cursor.len()) {
            let entity = runtime.cursor.pop_front().unwrap();
            let Some(mut target) = runtime.targets.remove(&entity) else {
                continue;
            };
            let Some(current) = world.get::<Collider>(entity).cloned() else {
                if target.applied != 0
                    && let Some(cache) = runtime.cache.get_mut(&target.key)
                {
                    cache.pins = cache.pins.saturating_sub(1);
                }
                release_reference(&mut runtime, target.key);
                runtime.coarse.remove(&entity);
                runtime.retired.push_back(target);
                continue;
            };
            if recover_scaled_source(world, &mut runtime, entity, &mut target, &current) {
                runtime.targets.insert(entity, target);
                runtime.cursor.push_back(entity);
                continue;
            }
            let current_is_ours = key(&current) == target.key
                || runtime.cache.get(&target.key).is_some_and(|cache| {
                    cache
                        .levels
                        .iter()
                        .any(|level| key(&level.collider) == key(&current))
                });
            let source_owner = owner(world, entity);
            if source_owner
                .as_ref()
                .is_none_or(|owner| owner.root != target.owner || owner.building)
            {
                if current_is_ours {
                    replace(world, &mut runtime, entity, &mut target, 0);
                } else if target.applied != 0
                    && let Some(cache) = runtime.cache.get_mut(&target.key)
                {
                    cache.pins = cache.pins.saturating_sub(1);
                }
                runtime.coarse.remove(&entity);
                release_reference(&mut runtime, target.key);
                runtime.retired.push_back(target);
                if source_owner.is_some() && runtime.queued.insert(entity) {
                    runtime.pending.push_back(entity);
                }
                continue;
            }
            if !current_is_ours {
                if runtime.queued.insert(entity) {
                    runtime.pending.push_back(entity);
                }
            } else {
                let policy = source_owner
                    .filter(|owner| owner.active && owner.root == target.owner && !owner.building)
                    .and_then(|owner| owner.policy.resolve_collision(global));
                let eligible = !target.rescale_guard
                    && policy.is_some_and(|policy| {
                        safe(world, entity, &target.original, foci.as_deref(), policy)
                    });
                if runtime
                    .failed
                    .get(&target.key)
                    .and_then(|failure| failure.retry_credit)
                    .is_some_and(|credit| credit != runtime.credit_revision)
                {
                    runtime.failed.remove(&target.key);
                }
                if eligible
                    && !runtime.cache.contains_key(&target.key)
                    && !runtime.failed.contains_key(&target.key)
                    && runtime.task.is_none()
                    && runtime.completed.is_none()
                    && runtime.release.is_none()
                {
                    let original = target.original.clone();
                    runtime.task = Some((
                        target.key,
                        AsyncComputeTaskPool::get().spawn(async move {
                            let lease = Arc::downgrade(&original.shape().0);
                            let result = cook(original);
                            (lease, result)
                        }),
                    ));
                }
                let desired = if eligible
                    && (target.applied != 0 || runtime.coarse.len() < TARGET_BUDGET)
                {
                    runtime.cache.get(&target.key).map_or(0, |cache| {
                        cache
                            .levels
                            .iter()
                            .enumerate()
                            .filter(|(_, level)| level.error <= policy.unwrap().max_error_meters)
                            .map(|(index, _)| index + 1)
                            .last()
                            .unwrap_or(0)
                    })
                } else {
                    0
                };
                replace(world, &mut runtime, entity, &mut target, desired);
            }
            runtime.targets.insert(entity, target);
            runtime.cursor.push_back(entity);
        }
    });
}
