//! 同一 GLB 的各个场景引用分别结算子网格落点。后台只处理真实地表快照，采用前核对实例代次。

use super::{
    components::{ScenePlacementInstance, ScenePlacementProgressPulse},
    resources::PlacementSettlementQueue,
    source_mesh_properties::{SourceMeshHeight, SourceMeshProperties, requires_source_grounding},
};
use crate::{
    TerrainObjectQueries, TerrainObjectRaySnapshot, TerrainSurfaceVersion,
    scenes::dynamic_scene::{PlacementGroundSources, PlacementGroundSurface},
    shared::world_asset_lifecycle::{WorldAssetLifecycleIndex, WorldAssetRootEpochInvalidated},
};
use bevy::{
    asset::AssetId,
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
    transform::TransformSystems,
    world_serialization::{
        InstanceId, WorldAsset, WorldAssetRoot, WorldInstance, WorldInstanceReady,
    },
};
use feathrise_scene_content::SceneId;
use feathrise_scene_core::residency::EntitySceneScope;
use std::{collections::BTreeMap, sync::Arc};

mod solve;

const NODES_PER_FRAME: usize = 128;
const SURFACES_PER_FRAME: usize = 16;

#[derive(Component)]
pub(super) struct SourceGroundingRequired;

#[derive(Component, Clone, Copy)]
pub(super) struct SourceGroundingReady(pub(super) u64);

#[derive(Clone, Debug, PartialEq)]
struct Ticket {
    root: Entity,
    instance: InstanceId,
    asset: AssetId<WorldAsset>,
    epoch: u64,
    scope: Option<Entity>,
    owner: SceneId,
}

#[derive(Resource, Default)]
pub(crate) struct SourceGroundingWork {
    requests: BTreeMap<Entity, Ticket>,
    scopes: BTreeMap<Entity, usize>,
    queue: PlacementSettlementQueue,
    active: Option<Job>,
    worker: Option<Task<WorkResult>>,
    propagate: Option<Ticket>,
}

type Surface = Option<(TerrainObjectRaySnapshot, TerrainSurfaceVersion)>;

#[derive(Clone, Copy)]
struct Node {
    entity: Entity,
    parent: Option<usize>,
    local: Transform,
    height: Option<SourceMeshHeight>,
}

struct Cursor {
    entity: Entity,
    parent: Option<usize>,
    child: usize,
}

struct Job {
    ticket: Ticket,
    pose: Mat4,
    sources: PlacementGroundSources,
    next_surface: usize,
    surfaces: Vec<Surface>,
    has_ground: bool,
    nodes: Vec<Node>,
    tree: Vec<Cursor>,
    solution: Option<Vec<(Entity, Vec3)>>,
    verifying: bool,
    next_write: usize,
}

struct WorkResult {
    job: Job,
    solution: Option<Vec<(Entity, Vec3)>>,
}

type GroundingRoot = (
    &'static WorldInstance,
    &'static WorldAssetRoot,
    &'static GlobalTransform,
);

pub(super) fn install(app: &mut App) {
    app.init_resource::<SourceGroundingWork>()
        .add_observer(queue_on_world_ready)
        .add_observer(refresh_world_epoch)
        .add_observer(discard_world_root)
        .add_systems(
            Update,
            drive.after(
                crate::shared::world_asset_lifecycle::WorldAssetLifecycleSystems::InvalidateRoots,
            ),
        )
        .add_systems(PostUpdate, publish.after(TransformSystems::Propagate));
}

/// 空闲的场景准备席位可推进供它使用的独立地形，避免父场景与地形互等。
pub(crate) fn waits_for_source_ground(world: &World, scope: Entity) -> bool {
    world
        .get_resource::<SourceGroundingWork>()
        .is_some_and(|work| work.scopes.contains_key(&scope))
}

fn queue_on_world_ready(
    event: On<WorldInstanceReady>,
    roots: Query<(
        GroundingRoot,
        &ScenePlacementInstance,
        Option<&SourceGroundingReady>,
    )>,
    owners: EntitySceneScope,
    assets: Option<Res<Assets<WorldAsset>>>,
    mut index: ResMut<WorldAssetLifecycleIndex>,
    mut work: ResMut<SourceGroundingWork>,
    mut commands: Commands,
) {
    let Ok(((instance, asset, _), placement, previous)) = roots.get(event.entity) else {
        return;
    };
    if **instance != event.instance_id {
        return;
    }
    let Some(world) = assets.as_ref().and_then(|assets| assets.get(&asset.0)) else {
        return;
    };
    if !requires_source_grounding(world) {
        cancel(&mut work, event.entity);
        commands
            .entity(event.entity)
            .try_remove::<(SourceGroundingRequired, SourceGroundingReady)>();
        return;
    }
    let ticket = Ticket {
        root: event.entity,
        instance: **instance,
        asset: asset.0.id(),
        epoch: index.mark_world_ready(event.entity, asset.0.id()),
        scope: owners.instance(event.entity),
        owner: placement.scene_id().clone(),
    };
    if previous.is_some_and(|ready| ready.0 == ticket.epoch)
        || work.requests.get(&ticket.root) == Some(&ticket)
    {
        return;
    }
    commands
        .entity(ticket.root)
        .try_remove::<SourceGroundingReady>()
        .try_insert(SourceGroundingRequired);
    enqueue(&mut work, ticket);
}

fn refresh_world_epoch(
    event: On<WorldAssetRootEpochInvalidated>,
    roots: Query<
        (
            GroundingRoot,
            &ScenePlacementInstance,
            Option<&SourceGroundingReady>,
        ),
        With<SourceGroundingRequired>,
    >,
    owners: EntitySceneScope,
    mut work: ResMut<SourceGroundingWork>,
    mut commands: Commands,
) {
    if !event.ready_after_event {
        return;
    }
    let Ok(((instance, asset, _), placement, ready)) = roots.get(event.entity) else {
        return;
    };
    if asset.0.id() != event.asset_id
        || work
            .requests
            .get(&event.entity)
            .is_some_and(|ticket| ticket.epoch >= event.epoch)
    {
        return;
    }
    // 被 Bevy 合并的资产通知可能递进 epoch 而不重新实例化，已传播的节点结果可以续签。
    if let Some(ready) = ready {
        if ready.0 < event.epoch {
            commands
                .entity(event.entity)
                .try_insert(SourceGroundingReady(event.epoch));
        }
    } else {
        enqueue(
            &mut work,
            Ticket {
                root: event.entity,
                instance: **instance,
                asset: event.asset_id,
                epoch: event.epoch,
                scope: owners.instance(event.entity),
                owner: placement.scene_id().clone(),
            },
        );
    }
}

fn discard_world_root(
    event: On<Discard<WorldAssetRoot>>,
    marked: Query<(), With<SourceGroundingRequired>>,
    mut work: ResMut<SourceGroundingWork>,
    mut commands: Commands,
) {
    cancel(&mut work, event.entity);
    if marked.contains(event.entity) {
        // 两个不同资产可能恰好具有相同的 epoch，根句柄被替换时必须清旧门。
        commands
            .entity(event.entity)
            .try_remove::<(SourceGroundingRequired, SourceGroundingReady)>();
    }
}

fn enqueue(work: &mut SourceGroundingWork, ticket: Ticket) {
    cancel(work, ticket.root);
    if let Some(scope) = ticket.scope {
        *work.scopes.entry(scope).or_default() += 1;
    }
    work.queue.enqueue(ticket.root);
    work.requests.insert(ticket.root, ticket);
}

fn cancel(work: &mut SourceGroundingWork, root: Entity) {
    work.queue.cancel(root);
    if let Some(scope) = work.requests.remove(&root).and_then(|ticket| ticket.scope) {
        let count = work
            .scopes
            .get_mut(&scope)
            .expect("已登记请求都有场景席位计数");
        *count -= 1;
        if *count == 0 {
            work.scopes.remove(&scope);
        }
    }
}

fn ticket_current(
    ticket: &Ticket,
    roots: &Query<GroundingRoot>,
    lifecycle: &WorldAssetLifecycleIndex,
) -> bool {
    lifecycle.request_epoch_is_current(ticket.root, ticket.asset, ticket.epoch)
        && roots.get(ticket.root).is_ok_and(|(instance, asset, _)| {
            **instance == ticket.instance && asset.0.id() == ticket.asset
        })
}

fn drive(
    mut work: ResMut<SourceGroundingWork>,
    roots: Query<GroundingRoot>,
    terrain: TerrainObjectQueries,
    tree: Query<&Children>,
    nodes: Query<(&Transform, Option<&SourceMeshProperties>)>,
    lifecycle: Res<WorldAssetLifecycleIndex>,
    mut commands: Commands,
) {
    if !lifecycle.events_caught_up() {
        return;
    }
    let work = &mut *work;
    if let Some(task) = work.worker.as_mut() {
        let Some(result) = check_ready(task) else {
            return;
        };
        work.worker = None;
        work.active = Some(result.job);
        let job = work.active.as_mut().unwrap();
        job.solution = result.solution;
        job.verifying = true;
        job.next_surface = 0;
    }
    if work.propagate.is_some() {
        return;
    }
    if work.active.is_none() {
        for _ in 0..work.queue.len().min(SURFACES_PER_FRAME) {
            let Some(root) = work.queue.pop_front() else {
                return;
            };
            let Some(ticket) = work.requests.get(&root).cloned() else {
                continue;
            };
            if !ticket_current(&ticket, &roots, &lifecycle) {
                cancel(work, root);
                continue;
            }
            let Some(sources) = terrain.placement_ground_sources(&ticket.owner, ticket.scope)
            else {
                work.queue.enqueue(root);
                continue;
            };
            if matches!(sources, PlacementGroundSources::Empty) {
                work.propagate = Some(ticket);
                return;
            }
            let pose = roots.get(root).unwrap().2.to_matrix();
            work.active = Some(Job {
                ticket,
                pose,
                sources,
                next_surface: 0,
                surfaces: Vec::new(),
                has_ground: false,
                nodes: Vec::new(),
                tree: vec![Cursor {
                    entity: root,
                    parent: None,
                    child: 0,
                }],
                solution: None,
                verifying: false,
                next_write: 0,
            });
            break;
        }
    }
    let Some(job) = work.active.as_mut() else {
        return;
    };
    let ticket = job.ticket.clone();
    if work.requests.get(&ticket.root) != Some(&ticket)
        || !ticket_current(&ticket, &roots, &lifecycle)
    {
        if work.requests.get(&ticket.root) == Some(&ticket) {
            cancel(work, ticket.root);
        }
        work.active = None;
        return;
    }
    let current_pose = roots.get(ticket.root).unwrap().2.to_matrix();
    let matching_sources = terrain
        .placement_ground_sources(&ticket.owner, ticket.scope)
        .is_some_and(|sources| same_sources(&sources, &job.sources));
    if job.next_write == 0 && (job.pose != current_pose || !matching_sources) {
        work.queue.enqueue(ticket.root);
        work.active = None;
        return;
    }
    let surface_step = job.next_surface;
    if !capture_surfaces(job, &terrain) {
        if job.next_surface > surface_step {
            commands.trigger(ScenePlacementProgressPulse {
                entity: ticket.root,
            });
        }
        return;
    }
    if job.verifying {
        if job.solution.is_none() {
            // 权威高度请求由同场景的地形调度消费，只有新页接管才重新计算。
            job.next_surface = 0;
            return;
        }
        let solution = job.solution.as_ref().unwrap();
        let end = (job.next_write + NODES_PER_FRAME).min(solution.len());
        for &(entity, translation) in &solution[job.next_write..end] {
            commands.entity(entity).queue_silenced(
                move |mut entity: bevy::ecs::world::EntityWorldMut| {
                    if let Some(mut transform) = entity.get_mut::<Transform>() {
                        transform.translation = translation;
                    }
                },
            );
        }
        job.next_write = end;
        commands.trigger(ScenePlacementProgressPulse {
            entity: ticket.root,
        });
        if job.next_write == solution.len() {
            work.propagate = Some(ticket.clone());
            work.active = None;
        }
        return;
    }
    if !job.has_ground {
        work.propagate = Some(ticket.clone());
        work.active = None;
        return;
    }
    let captured = &mut job.nodes;
    for _ in 0..NODES_PER_FRAME {
        let Some(cursor) = job.tree.last_mut() else {
            break;
        };
        let Some(entity) = tree
            .get(cursor.entity)
            .ok()
            .and_then(|children| children.get(cursor.child))
            .copied()
        else {
            job.tree.pop();
            continue;
        };
        cursor.child += 1;
        let parent = if let Ok((local, properties)) = nodes.get(entity) {
            let index = captured.len();
            captured.push(Node {
                entity,
                parent: cursor.parent,
                local: *local,
                height: properties.and_then(|properties| properties.height),
            });
            Some(index)
        } else {
            cursor.parent
        };
        job.tree.push(Cursor {
            entity,
            parent,
            child: 0,
        });
    }
    commands.trigger(ScenePlacementProgressPulse {
        entity: ticket.root,
    });
    if job.tree.is_empty() {
        let job = work.active.take().unwrap();
        work.worker = Some(AsyncComputeTaskPool::get().spawn(async move {
            let solution = solve::positions(job.pose, &job.nodes, &job.surfaces);
            WorkResult { job, solution }
        }));
    }
}

fn same_sources(current: &PlacementGroundSources, captured: &PlacementGroundSources) -> bool {
    match (current, captured) {
        (PlacementGroundSources::Objects(left), PlacementGroundSources::Objects(right)) => {
            Arc::ptr_eq(left, right)
        }
        (PlacementGroundSources::Legacy, PlacementGroundSources::Legacy)
        | (PlacementGroundSources::Empty, PlacementGroundSources::Empty) => true,
        _ => false,
    }
}

fn capture_surfaces(job: &mut Job, terrain: &TerrainObjectQueries) -> bool {
    let total = match &job.sources {
        PlacementGroundSources::Objects(objects) => objects.len(),
        _ => 1,
    };
    for _ in 0..SURFACES_PER_FRAME {
        if job.next_surface == total {
            return true;
        }
        let (id, enabled) = match &job.sources {
            PlacementGroundSources::Objects(objects) => {
                let object = &objects[job.next_surface];
                (Some(&object.id), object.enabled)
            }
            _ => (None, true),
        };
        let snapshot = if enabled {
            match terrain.placement_ground_surface(&job.ticket.owner, job.ticket.scope, id) {
                PlacementGroundSurface::Pending => return false,
                PlacementGroundSurface::Empty => None,
                PlacementGroundSurface::Ready(snapshot, version) => Some((snapshot, version)),
            }
        } else {
            None
        };
        if job.verifying {
            let old = &job.surfaces[job.next_surface];
            let changed = match (old.as_ref(), snapshot.as_ref()) {
                (Some((_, old)), Some((_, new))) if job.solution.is_none() => old != new,
                (Some((_, old)), Some((_, new))) => !old.same_content(new),
                (None, None) => false,
                _ => true,
            };
            if changed {
                job.verifying = false;
                job.solution = None;
                job.next_surface = 0;
                job.surfaces.clear();
                job.has_ground = false;
                return false;
            }
        } else {
            job.has_ground |= snapshot.is_some();
            job.surfaces.push(snapshot);
        }
        job.next_surface += 1;
    }
    job.next_surface == total
}

fn publish(
    mut work: ResMut<SourceGroundingWork>,
    roots: Query<GroundingRoot>,
    lifecycle: Res<WorldAssetLifecycleIndex>,
    mut commands: Commands,
) {
    if !lifecycle.events_caught_up() {
        return;
    }
    let Some(ticket) = work.propagate.take() else {
        return;
    };
    if work.requests.get(&ticket.root) != Some(&ticket) {
        return;
    }
    cancel(&mut work, ticket.root);
    if ticket_current(&ticket, &roots, &lifecycle) {
        // 物理捕获与候选采用只能消费包含实际子节点世界坐标的同一代次。
        commands
            .entity(ticket.root)
            .try_insert(SourceGroundingReady(ticket.epoch));
    }
}
