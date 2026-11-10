//! 洞内水面单任务构建、单块登记、完整GPU采用和分帧回收。
use super::*;
use crate::shared::large_scene::{WaterGpuUploadQueue, WaterGpuUploadReceipts, WaterGpuUploadTicket};
use std::collections::{BTreeMap, VecDeque};

#[derive(Default)]
struct Batch {
    root: Option<Entity>,
    owners: Vec<(Entity, Handle<ClipmapWaterMaterial>)>,
    pieces: VecDeque<(Entity, Handle<Mesh>)>,
    warmups: Vec<Entity>,
}
struct Pending {
    identity: Identity,
    remaining: VecDeque<(Entity, Mesh)>,
    batch: Batch,
    sources: BTreeMap<Entity, usize>,
    ticket: Option<WaterGpuUploadTicket>,
}
#[derive(Resource, Default)]
struct State {
    active: Option<Identity>,
    building: Option<Identity>,
    failed: Option<Identity>,
    task: Option<Task<Result<Vec<(Entity, Mesh)>, String>>>,
    pending: Option<Pending>,
    installed: Batch,
    retired: VecDeque<Batch>,
    retirement: Option<Task<()>>,
}

pub(crate) fn install(app: &mut App) {
    app.init_instance_resource::<State>()
        .add_systems(
            Update,
            drive
                .after(super::super::runtime::follow_clipmap_water)
                .after(crate::scenes::dynamic_scene::ClipmapGroundPublishSet)
                .run_if(in_state(feathrise_scene_core::state::ScenePhase::Active)),
        )
        .add_systems(OnExit(feathrise_scene_core::state::ScenePhase::Active), reset);
}
fn reset(mut state: InstanceResMut<State>, mut uploads: ResMut<WaterGpuUploadQueue>) {
    if let Some(ticket) = state.pending.as_ref().and_then(|p| p.ticket) {
        uploads.retire(ticket);
    }
    let old = std::mem::take(&mut *state);
    AsyncComputeTaskPool::get()
        .spawn(async move { drop(old) })
        .detach();
}

fn retire_batch(commands: &mut Commands, state: &mut State, batch: Batch) {
    if let Some(root) = batch.root {
        commands.entity(root).insert(Visibility::Hidden);
    }
    for (root, _) in &batch.owners {
        commands
            .entity(*root)
            .remove::<DynamicWaterCpuInteraction>();
    }
    state.retired.push_back(batch);
}
fn discard_pending(commands: &mut Commands, state: &mut State, uploads: &mut WaterGpuUploadQueue) {
    let Some(pending) = state.pending.take() else {
        return;
    };
    if let Some(ticket) = pending.ticket {
        uploads.retire(ticket);
    }
    retire_batch(commands, state, pending.batch);
    // 此任务占用同一域的退役槽，结束前不启动下一份大CPU结果。
    state.retirement = Some(AsyncComputeTaskPool::get().spawn(async move {
        drop(pending.remaining);
        drop(pending.sources);
    }));
}
fn reap(
    commands: &mut Commands,
    state: &mut State,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<ClipmapWaterMaterial>,
) {
    if state.retirement.as_mut().and_then(check_ready).is_some() {
        state.retirement = None;
    }
    let Some(batch) = state.retired.front_mut() else {
        return;
    };
    for _ in 0..32 {
        if let Some((entity, mesh)) = batch.pieces.pop_front() {
            commands.entity(entity).try_despawn();
            meshes.remove(mesh.id());
        } else if let Some(entity) = batch.warmups.pop() {
            commands.entity(entity).try_despawn();
        } else if let Some((entity, material)) = batch.owners.pop() {
            commands.entity(entity).try_despawn();
            materials.remove(material.id());
        } else {
            if let Some(root) = batch.root {
                commands.entity(root).try_despawn();
            }
            state.retired.pop_front();
            break;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn drive(
    mut commands: Commands,
    frame: feathrise_scene_core::residency::InstanceFrame,
    mut state: InstanceResMut<State>,
    ground: InstanceRes<DynamicClipmapGroundSurface>,
    layers: InstanceQuery<
        (
            Entity,
            &ClipmapWaterSurface,
            &MeshMaterial3d<ClipmapWaterMaterial>,
            Option<&DynamicWaterSurface>,
        ),
        Without<VolumeWaterSurface>,
    >,
    mut targets: InstanceQuery<
        (
            &Source,
            &mut ClipmapWaterSurface,
            &mut DynamicWaterSurface,
            &mut Visibility,
        ),
        With<VolumeWaterSurface>,
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ClipmapWaterMaterial>>,
    mut uploads: ResMut<WaterGpuUploadQueue>,
    receipts: Res<WaterGpuUploadReceipts>,
    mut water_status: Option<InstanceResMut<super::super::super::resources::DynamicWaterStatus>>,
    work: Option<Res<crate::scenes::dynamic_scene::voxel_objects::VoxelWorkBudget>>,
) {
    reap(&mut commands, &mut state, &mut meshes, &mut materials);
    let Some(ground) = ground.snapshot.as_ref().filter(|g| g.ready) else {
        for (_, _, _, mut v) in &mut targets {
            *v = Visibility::Hidden;
        }
        discard_pending(&mut commands, &mut state, &mut uploads);
        return;
    };
    let mut identity = Identity {
        ground: ground.content_revision_words(),
        window: if ground.is_density_surface() {
            ground.revision
        } else {
            [0; 3]
        },
        layers: layers
            .iter()
            .filter(|(_, s, _, _)| s.is_published())
            .map(|(e, s, m, _)| (e, m.0.id(), s.mask_identity(), layer_key(s)))
            .collect(),
    };
    identity.layers.sort_by_key(|v| v.0);
    let current = state.active.as_ref() == Some(&identity);
    let retained = state
        .active
        .as_ref()
        .is_some_and(|active| active.ground == identity.ground);
    for (source, mut surface, mut interaction, mut visibility) in &mut targets {
        *visibility = if retained || current && layers.contains(source.0) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if current {
            surface.height_revision = ground.revision;
            interaction.sync_clipmap_footprint(&surface, ground);
        }
    }
    if state
        .pending
        .as_ref()
        .is_some_and(|p| p.identity != identity)
    {
        discard_pending(&mut commands, &mut state, &mut uploads);
    }
    if let Some(result) = state.task.as_mut().and_then(check_ready) {
        let built = state.building.take().expect("构建任务带有完整身份");
        state.task = None;
        if built != identity {
            state.retirement = Some(AsyncComputeTaskPool::get().spawn(async move { drop(result) }));
        } else {
            match result {
                Ok(ready) => {
                    let root = commands
                        .spawn((
                            Name::new("三维静水候选"),
                            Transform::IDENTITY,
                            Visibility::Hidden,
                            DespawnOnExit(feathrise_scene_core::state::ScenePhase::Active),
                        ))
                        .id();
                    state.pending = Some(Pending {
                        identity: built,
                        remaining: ready.into(),
                        batch: Batch {
                            root: Some(root),
                            ..default()
                        },
                        sources: BTreeMap::new(),
                        ticket: None,
                    });
                }
                Err(error) => {
                    warn!("三维静水未采用：{error}");
                    state.failed = Some(built);
                }
            }
        }
    }
    if let Some(pending) = state.pending.as_mut() {
        if !pending.remaining.is_empty()
            && uploads
                .try_reserve_registration(64 * 1024 * 4)
                .unwrap_or(false)
        {
            let (source_entity, mesh) = pending.remaining.pop_front().expect("已检查水面分块队列");
            let Ok((_, source_surface, source_mat, Some(source_interaction))) =
                layers.get(source_entity)
            else {
                state.failed = Some(identity.clone());
                discard_pending(&mut commands, &mut state, &mut uploads);
                return;
            };
            let owner_index = if let Some(&index) = pending.sources.get(&source_entity) {
                index
            } else {
                let Some(source_material) = materials.get(&source_mat.0).cloned() else {
                    state.failed = Some(identity.clone());
                    discard_pending(&mut commands, &mut state, &mut uploads);
                    return;
                };
                let material = materials.add(ClipmapWaterMaterial {
                    base: source_material.base,
                    extension: source_material.extension.for_volume(),
                });
                let surface = source_surface.clone().volume_copy();
                let interaction = source_interaction.derived_for_surface(&surface, ground);
                let owner = commands
                    .spawn((
                        VolumeWaterSurface,
                        Source(source_entity),
                        PaintedWaterSurface {
                            scene_id: surface.scene_id.clone(),
                            layer_id: surface.layer_id.clone(),
                            kind: WaterSurfaceKind::Still,
                            purpose: PaintedWaterPurpose::Runtime,
                        },
                        surface,
                        interaction,
                        MeshMaterial3d(material.clone()),
                        Transform::IDENTITY,
                        Visibility::Inherited,
                        ChildOf(pending.batch.root.unwrap()),
                    ))
                    .id();
                let index = pending.batch.owners.len();
                pending.batch.owners.push((owner, material));
                pending.sources.insert(source_entity, index);
                index
            };
            let (owner, material) = &pending.batch.owners[owner_index];
            let mesh = meshes.add(mesh);
            let piece = commands
                .spawn((
                    VolumeWaterSurface,
                    crate::shared::large_scene::WaterRenderChunk,
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    Transform::IDENTITY,
                    Visibility::Inherited,
                    ChildOf(*owner),
                    bevy::camera::visibility::NoFrustumCulling,
                    bevy::light::NotShadowCaster,
                    feathrise_scene_core::view::CameraRaycastIgnore,
                ))
                .id();
            // 只有每个水层的首块需要管线预热；所有块仍逐一纳入GPU资产回执。
            if pending.batch.warmups.len() < pending.batch.owners.len() {
                let warmup = commands
                    .spawn((
                        // 候选场景根仍隐藏时也需进入RenderApp；只有GPU证据通过后才显示真实块。
                        feathrise_scene_core::residency::SceneInstanceDetached,
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(material.clone()),
                        Transform::from_xyz(0., -100000., 0.).with_scale(Vec3::splat(0.000001)),
                        Visibility::Inherited,
                        bevy::camera::visibility::NoFrustumCulling,
                        bevy::light::NotShadowCaster,
                        feathrise_scene_core::view::CameraRaycastIgnore,
                        DespawnOnExit(feathrise_scene_core::state::ScenePhase::Active),
                    ))
                    .id();
                if let Some(instance) = frame.instance() {
                    commands
                        .entity(warmup)
                        .insert(feathrise_scene_core::residency::SceneInstanceOwner(instance));
                }
                pending.batch.warmups.push(warmup);
            }
            pending.batch.pieces.push_back((piece, mesh));
        }
        if !pending.remaining.is_empty() {
            return;
        }
        if pending.ticket.is_none() && !pending.batch.pieces.is_empty() {
            pending.ticket = Some(
                uploads.submit_assets(
                    pending.batch.pieces.iter().map(|(_, h)| h.id()).collect(),
                    pending
                        .batch
                        .owners
                        .iter()
                        .map(|(_, m)| m.id().untyped())
                        .collect(),
                    pending.batch.warmups.clone(),
                ),
            );
        }
        if pending.ticket.is_some_and(|t| receipts.has_failed(t)) {
            state.failed = Some(identity);
            discard_pending(&mut commands, &mut state, &mut uploads);
            return;
        }
        if pending.ticket.is_some_and(|t| !receipts.is_ready(t)) {
            return;
        }
        let mut pending = state.pending.take().expect("完整洞内水面候选存在");
        if let Some(ticket) = pending.ticket {
            uploads.retire(ticket);
        }
        for entity in pending.batch.warmups.drain(..) {
            commands.entity(entity).try_despawn();
        }
        if let Some(root) = pending.batch.root {
            commands.entity(root).insert(Visibility::Inherited);
        }
        for (owner, _) in &pending.batch.owners {
            commands
                .entity(*owner)
                .insert((Visibility::Inherited, DynamicWaterCpuInteraction));
        }
        let old = std::mem::replace(&mut state.installed, pending.batch);
        retire_batch(&mut commands, &mut state, old);
        state.active = Some(identity.clone());
        if ground.is_density_surface()
            && let Some(status) = water_status.as_mut()
        {
            status.applied_generation = status.desired_generation;
            status.phase = super::super::super::resources::DynamicWaterPhase::Ready;
            status.detail = "原生静水真实水深、分块网格与透明PBR已共同采用".into();
        }
    }
    if state.task.is_none()
        && state.pending.is_none()
        && state.retirement.is_none()
        && state.active.as_ref() != Some(&identity)
        && state.failed.as_ref() != Some(&identity)
    {
        let credit = if ground.is_density_surface() {
            if let Some(work) = work.as_ref() {
                let Some(credit) = work.acquire() else {
                    return;
                };
                Some(credit)
            } else {
                None
            }
        } else {
            None
        };
        let snapshot = ground.clone();
        let input = layers
            .iter()
            .filter(|(_, s, _, _)| s.is_published())
            .map(|(e, s, _, _)| (e, s.clone()))
            .collect();
        state.building = Some(identity);
        state.task = Some(AsyncComputeTaskPool::get().spawn(async move {
            let _credit = credit;
            build(snapshot, input)
        }));
    }
}

pub(in crate::scenes::dynamic_scene::water::clipmap) fn add_instance_systems(schedule: &mut Schedule) {
    schedule.add_systems(
        drive
            .after(super::super::runtime::follow_clipmap_water)
            .after(crate::scenes::dynamic_scene::clipmap::sync_clipmap_ground_surface),
    );
}
