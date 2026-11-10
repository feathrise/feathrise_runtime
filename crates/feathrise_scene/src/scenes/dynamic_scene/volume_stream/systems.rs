//! 一份完整窗口与一个最新焦点；候选Ready之后由统一拓扑系统原子采用。
use super::super::{
    resources::DynamicSceneRuntimeState,
    sparse_runtime::DynamicClipmapAuthoringView,
    terrain_topology::{TerrainVolumePreviewRequest, TerrainVolumePreviewTarget},
};
use super::{components::StreamedVolumeResident, jobs, resources::*, types::*};
use crate::{
    DeferredTerrainPreviewCollider, ScenePlacementInstance, ScenePlacementRuntimePhase,
    ScenePlacementRuntimeStatus, terrain_placement_collision,
};
use bevy::ecs::system::SystemParam;
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, IoTaskPool, futures::check_ready},
};
use crate::SceneRuntimeObserver;
use feathrise_scene_content::{ActiveDynamicScene, ScenePlacementsAsset};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[derive(SystemParam)]
pub(super) struct Graphics<'w, 's> {
    asset_root: Option<Res<'w, feathrise_scene_content::RuntimeAssetRoot>>,
    retirement: Option<Res<'w, super::display::FieldGarbage>>,
    clipmap: Option<feathrise_scene_core::residency::InstanceRes<'w, super::super::sparse_runtime::SparseClipmapRuntime>>,
    meshes: Option<Res<'w, Assets<Mesh>>>,
    materials: Option<ResMut<'w, Assets<StandardMaterial>>>,
    residents: Query<'w, 's, &'static StreamedVolumeResident>,
    failures: Query<'w, 's, &'static crate::TerrainPatchColliderFailure>,
    uploads: Query<'w, 's, &'static crate::TerrainMeshUpload>,
}

pub(super) fn drive(
    mut commands: Commands,
    active: feathrise_scene_core::residency::InstanceRes<ActiveDynamicScene>,
    map: feathrise_scene_core::residency::InstanceRes<DynamicSceneRuntimeState>,
    catalog: Option<crate::scenes::dynamic_scene::sources::SceneRuntimeCatalog>,
    placements: Option<Res<Assets<ScenePlacementsAsset>>>,
    placement_status: Option<feathrise_scene_core::residency::InstanceRes<ScenePlacementRuntimeStatus>>,
    view_inputs: (feathrise_scene_core::residency::InstanceFrame, Query<(&Camera, &DynamicClipmapAuthoringView)>),
    players: Query<&Transform, With<SceneRuntimeObserver>>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    mut state: feathrise_scene_core::residency::InstanceResMut<Runtime>,
    mut control: crate::scenes::dynamic_scene::author_sources::TerrainAuthorResMut<TerrainVolumeStreamControl>,
    mut status: feathrise_scene_core::residency::InstanceResMut<TerrainVolumeStreamStatus>,
    mut request: crate::scenes::dynamic_scene::author_sources::TerrainAuthorResMut<TerrainVolumePreviewRequest>,
    mut graphics: Graphics,
    time: Res<Time<Real>>,
    mut scene_status: feathrise_scene_core::residency::InstanceResMut<super::super::resources::DynamicSceneRuntimeStatus>,
) {
    let (frame, views) = view_inputs;
    if request.is_container() {
        if let Some(result) = state.work.as_mut().and_then(|work| check_ready(&mut work.task)) {
            state.work = None;
            state.release = Some(AsyncComputeTaskPool::get().spawn(async move { drop(result); }));
        }
        if state.release.as_mut().and_then(check_ready).is_some() { state.release = None; }
        return; // 父容器不再生成体积窗口，但遗留在途任务仍归还真实计算槽。
    }
    control.retirement_busy = graphics.retirement.as_ref().is_some_and(|g| g.saturated());
    if control.retirement_busy {
        return;
    }
    if state.release.as_mut().and_then(check_ready).is_some() {
        state.release = None;
    }
    let scope = active
        .scene_id
        .as_ref()
        .zip(map.root)
        .filter(|(id, _)| {
            map.active_scene_id.as_ref() == Some(*id) && map.sparse_height_path.is_some()
        })
        .map(|(id, root)| (id.clone(), root));
    if state.scope != scope {
        if let Some(old) = state.pending.take() {
            commands.entity(old.root).try_despawn();
        }
        state.scope = scope.clone();
        state.records.clear();
        state.revision = None;
        state.next = 0;
        state.scanning = true;
        state.active = None;
        state.failed = None;
        state.observed_root = None;
        status.scene_id = scope.as_ref().map(|(id, _)| id.clone());
        status.ready = false;
    }
    let Some((id, root)) = scope else {
        control.native_busy = state.work.is_some() || state.release.is_some();
        return;
    };
    if let Some(lods) = graphics
        .clipmap
        .as_ref()
        .and_then(|c| c.volume_display_lods(&id))
    {
        if control.display_lods.as_ref() != lods.as_slice() {
            control.display_lods = lods.into();
        }
    }
    if let Some(s) = placement_status
        .as_deref()
        .filter(|s| s.scene_id.as_ref() == Some(&id) && s.phase == ScenePlacementRuntimePhase::Ready)
    {
        if state.revision != Some(s.revision) {
            state.revision = Some(s.revision);
            state.records.clear();
            state.next = 0;
            state.scanning = true;
        }
        if state.scanning {
            if let Some(asset) = catalog
                .as_deref()
                .and_then(|c| c.scenes.get(&id))
                .and_then(|h| placements.as_deref().and_then(|p| p.get(&h.placements)))
            {
                let end = (state.next + 128).min(asset.placements.len());
                for r in &asset.placements[state.next..end] {
                    if r.terrain_patch.as_ref().is_some_and(|p| p.streamed_volume) {
                        if state.records.len() == 32 {
                            status.detail = "流送作者层超过32份场景记录".into();
                            state.scanning = false;
                            return;
                        }
                        state.records.push(r.clone());
                    }
                }
                state.next = end;
                state.scanning = end < asset.placements.len();
            }
        }
    }
    let author_focus = views
        .iter()
        .find(|(c, _)| c.is_active)
        .map(|(_, v)| Vec3::new(v.focus_xz.x, v.focus_y, v.focus_xz.y));
    let world_physics_focus = players.iter().next().map(|p| p.translation);
    control.physics_focus = world_physics_focus;
    let physics_focus = world_physics_focus.map(|point| frame.to_local(point));
    let focus = if let Some(view) = author_focus {
        if control
            .focus_anchor
            .is_some_and(|p| p.distance(view) > 0.25)
        {
            control.focus_override = None;
            control.focus_anchor = None;
        }
        control.focus_override.unwrap_or(view)
    } else {
        players
            .iter()
            .next()
            .map(|p| p.translation)
            .or_else(|| {
                map.camera
                    .and_then(|e| cameras.get(e).ok())
                    .map(|p| p.translation())
            })
            .unwrap_or(Vec3::ZERO)
    };
    let focus = frame.to_local(focus);
    let draft = control
        .draft
        .as_ref()
        .filter(|(m, _)| m == &id)
        .map(|(_, r)| r);
    let record = draft
        .or_else(|| {
            state
                .records
                .iter()
                .filter(|r| control.excluded.as_deref() != Some(r.id.as_str()))
                .min_by(|a, b| distance(a, focus).total_cmp(&distance(b, focus)))
        })
        .cloned();
    let desired = record.map(|record| {
        let extent = record
            .terrain_patch
            .as_ref()
            .and_then(|p| p.volume_cell_size)
            .unwrap_or(0.25)
            * 32.;
        let origin = Vec3::from_array(record.translation);
        Identity {
            scene_id: id.clone(),
            root,
            center: ((focus - origin) / extent).floor().as_ivec3().to_array(),
            physics_center: author_focus
                .and(physics_focus)
                .map(|p| ((p - origin) / extent).floor().as_ivec3().to_array()),
            record,
            lods: control.display_lods.clone(),
        }
    });
    if let Some(work) = state.work.as_mut() {
        let observed = work.progress.load(Ordering::Relaxed);
        if observed != work.observed {
            work.observed = observed;
            work.started = time.elapsed_secs_f64();
        }
        // 有界任务槽控制并发；完成时间可随规模增长，采用只校验版本和编辑所有权。
        if let Some(result) = check_ready(&mut work.task) {
            let work = state.work.take().unwrap();
            if control.editing || desired.as_ref() != Some(&work.identity) {
                state.release = Some(AsyncComputeTaskPool::get().spawn(async move {
                    drop(result);
                }));
            } else {
                match result {
                    Ok(Built {
                        record,
                        meshes,
                        fields,
                    }) if !meshes.is_empty() => {
                        if let Err(error) = fields.validate_storage_limit(control.gpu_storage_limit)
                        {
                            state.failed = Some(work.identity);
                            status.detail = error;
                            if !map.ready_released {
                                scene_status.phase =
                                    super::super::resources::DynamicSceneRuntimePhase::Failed;
                                scene_status.detail = status.detail.clone();
                            }
                            state.release = Some(
                                AsyncComputeTaskPool::get()
                                    .spawn(async move { drop((meshes, fields)) }),
                            );
                            control.native_busy = true;
                            return;
                        }
                        let (Some(_), Some(materials)) = (
                            graphics.meshes.as_deref(),
                            graphics.materials.as_deref_mut(),
                        ) else {
                            state.failed = Some(work.identity);
                            status.detail = "当前宿主未安装三维Mesh/材质存储".into();
                            if !map.ready_released {
                                scene_status.phase =
                                    super::super::resources::DynamicSceneRuntimePhase::Failed;
                                scene_status.detail = status.detail.clone();
                            }
                            state.release = Some(AsyncComputeTaskPool::get().spawn(async move {
                                drop((meshes, fields));
                            }));
                            control.native_busy = true;
                            return;
                        };
                        let material = state
                            .material
                            .get_or_insert_with(|| {
                                materials.add(StandardMaterial {
                                    base_color: Color::WHITE,
                                    perceptual_roughness: 1.,
                                    ..default()
                                })
                            })
                            .clone();
                        let hole = record
                            .terrain_cutout(Rect::from_corners(
                                Vec2::splat(-1.0e8),
                                Vec2::splat(1.0e8),
                            ))
                            .expect("后台驻留范围合法")
                            .expect("非空网格有驻留列");
                        let entity = commands
                            .spawn((
                                StreamedVolumeResident {
                                    center: work.identity.center,
                                    physics_center: work.identity.physics_center,
                                    lod_signature: super::volume_lod_signature(&work.identity.lods),
                                },
                                fields,
                                Name::new("体积地形驻留窗口"),
                                DespawnOnExit(feathrise_scene_core::state::ScenePhase::Active),
                                ScenePlacementInstance::new(&id, &record),
                                Transform::from_translation(Vec3::from_array(record.translation)),
                                // 显示与独立物理根都等联合采用；离场连未授信候选也一并回收。
                                Visibility::Hidden,
                                DeferredTerrainPreviewCollider,
                                crate::TerrainMeshUpload::new(meshes, material),
                            ))
                            .insert(terrain_placement_collision(&record).unwrap())
                            .id();
                        state.pending = Some(Pending {
                            identity: work.identity,
                            root: entity,
                            previous: request.target.clone(),
                            started: time.elapsed_secs_f64(),
                        });
                        request.target = Some(TerrainVolumePreviewTarget {
                            scene_id: id.clone(),
                            entity,
                            record,
                            world_cutout: hole,
                            removal: false,
                        });
                        status.detail = "体积窗口已加载，等待网格与高度碰撞共同采用".into();
                    }
                    Ok(_) => {
                        if let Some(target) = request.target.as_mut().filter(|t| {
                            t.scene_id == id
                                && t.record
                                    .terrain_patch
                                    .as_ref()
                                    .is_some_and(|p| p.streamed_volume)
                        }) {
                            let previous = target.clone();
                            target.removal = true;
                            state.pending = Some(Pending {
                                identity: work.identity,
                                root: target.entity,
                                previous: Some(previous),
                                started: time.elapsed_secs_f64(),
                            });
                        } else {
                            state.active = Some(work.identity);
                            status.ready = true;
                        }
                    }
                    Err(error) => {
                        state.failed = Some(work.identity);
                        status.detail = error.clone();
                        error!("{error}");
                        if !map.ready_released {
                            scene_status.phase =
                                super::super::resources::DynamicSceneRuntimePhase::Failed;
                            scene_status.detail = error;
                        }
                    }
                }
            }
        }
    }
    if let Some(p) = state.pending.as_ref() {
        if let Ok(upload) = graphics.uploads.get(p.root) {
            let (done, total) = upload.progress();
            status.detail = format!("体积网格分块上传：{done}/{total}，完整就绪后采用");
        }
        let removal = request
            .target
            .as_ref()
            .is_some_and(|t| t.entity == p.root && t.removal);
        if request.is_adopted(p.root, removal) {
            let pending = state.pending.take().unwrap();
            state.active = Some(pending.identity);
            status.ready = true;
            status.detail = "体积驻留显示、拾取和物理已共同采用".into();
        } else if control.editing
            || desired.as_ref() != Some(&p.identity)
            || graphics.failures.contains(p.root)
            || removal && time.elapsed_secs_f64() - p.started > 30.
        {
            let pending = state.pending.take().unwrap();
            request.target = pending.previous;
            if let Ok(failure) = graphics.failures.get(pending.root) {
                state.failed = Some(pending.identity);
                status.detail = failure.detail().to_owned();
                if !map.ready_released {
                    scene_status.phase = super::super::resources::DynamicSceneRuntimePhase::Failed;
                    scene_status.detail = status.detail.clone();
                }
            } else if removal && time.elapsed_secs_f64() - pending.started > 30. {
                state.failed = Some(pending.identity);
                status.detail = "体积窗口碰撞采用超时，保留上一完整窗口".into();
                if !map.ready_released {
                    scene_status.phase = super::super::resources::DynamicSceneRuntimePhase::Failed;
                    scene_status.detail = status.detail.clone();
                }
            }
            if !removal {
                commands.entity(pending.root).try_despawn();
            }
        }
    }
    control.native_busy =
        state.work.is_some() || state.pending.is_some() || state.release.is_some();
    if !control.editing && !control.native_busy && request.adopted_root() != state.observed_root {
        state.observed_root = request.adopted_root();
        if let Some(desired) = desired.as_ref()
            && request.target.as_ref().is_some_and(|t| {
                Some(t.entity) == state.observed_root
                    && !t.removal
                    && t.scene_id == desired.scene_id
                    && t.record.id == desired.record.id
                    && t.record.asset_path == desired.record.asset_path
                    && graphics.residents.get(t.entity).is_ok_and(|window| {
                        window.center == desired.center
                            && window.physics_center == desired.physics_center
                            && window.lod_signature == super::volume_lod_signature(&desired.lods)
                    })
            })
        {
            state.active = Some(desired.clone());
            status.ready = true;
        } // 只复用F12已采用且焦点也相同的窗口；等待期间移动或传送仍须请求新窗口。
    }
    if control.editing || control.native_busy || state.scanning && control.draft.is_none() {
        return;
    }
    let Some(desired) = desired else {
        if request.target.as_ref().is_some_and(|t| {
            t.scene_id == id
                && t.record
                    .terrain_patch
                    .as_ref()
                    .is_some_and(|p| p.streamed_volume)
        }) {
            request.target = None;
        }
        state.active = None;
        status.ready = true;
        return;
    };
    if state.active.as_ref() == Some(&desired) || state.failed.as_ref() == Some(&desired) {
        return;
    }
    state.failed = None;
    let progress = Arc::new(AtomicU64::new(0));
    let asset_root = graphics.asset_root.as_deref().cloned().unwrap_or_default(); // 在请求边界冻结路径，后台不再查可变全局来源。
    state.work = Some(Work {
        identity: desired.clone(),
        task: IoTaskPool::get().spawn(jobs::build(asset_root, desired, progress.clone())),
        started: time.elapsed_secs_f64(),
        progress,
        observed: 0,
    });
    control.native_busy = true;
}
fn distance(record: &feathrise_scene_content::ScenePlacementRecord, p: Vec3) -> f32 {
    record
        .terrain_patch
        .as_ref()
        .and_then(|h| h.cutouts.first())
        .map(|h| {
            let local = p.xz() - Vec3::from_array(record.translation).xz();
            local.distance_squared(local.clamp(Vec2::from_array(h.min), Vec2::from_array(h.max)))
        })
        .unwrap_or(f32::INFINITY)
}
