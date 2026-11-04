//! 三维地形摆放与 Clipmap 洞口的联合发布；候选失败或迟到不改变旧洞口。

use super::resources::DynamicMapRuntimeState;
use crate::shared::placement_data::MapPlacementRuntimeState;
use crate::shared::placement_data::TerrainPatchColliderReady;
use bevy::prelude::*;
use feathrise_map_content::{ActiveDynamicMap, MapId, MapTerrainCutout};
use std::sync::Arc;

/// 原始GlobalTransform与上下文同属上一传播代次；整体拖动不能变成内部拓扑变化。
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct TerrainTopologyFrame<'w, 's> {
    frame: feathrise_scene_core::residency::InstanceFrame<'w>,
    globals: Query<'w, 's, &'static GlobalTransform>,
}
impl TerrainTopologyFrame<'_, '_> {
    fn inverse(&self) -> Mat4 {
        self.frame.instance().and_then(|entity| self.globals.get(entity).ok()).map_or(
            self.frame.frame().world_to_source, |global| global.to_matrix().inverse())
    }
}

mod query;
mod resources;
mod volume;
mod volume_surface;
pub(crate) use query::TerrainMeshQuery;
pub(crate) use resources::TerrainTopologyRuntime;
pub(crate) use resources::VolumeSurfaceState;
pub use resources::{
    StaticTerrainRaytracingSources, TerrainTopologySystems, TerrainVolumePreviewRequest,
    TerrainVolumePreviewTarget,
};
pub(crate) use volume_surface::sync_volume_surface;

/// 离场必须立即归还体积预览、查询及光追来源；不能等待下一次进入动态地图才轮询清理。
pub(crate) fn clear_terrain_topology(
    mut commands: Commands,
    mut topology: feathrise_scene_core::residency::InstanceResMut<TerrainTopologyRuntime>,
    mut ray_sources: feathrise_scene_core::residency::InstanceResMut<StaticTerrainRaytracingSources>,
    mut garbage: Option<ResMut<crate::shared::placement_data::TerrainCollisionGarbage>>,
    mut preview: Option<crate::maps::dynamic_map::author_sources::TerrainAuthorResMut<TerrainVolumePreviewRequest>>,
    mut volume_surface: Option<feathrise_scene_core::residency::InstanceResMut<VolumeSurfaceState>>,
) {
    if let Some(state) = volume_surface.as_deref_mut() {
        *state = VolumeSurfaceState::default(); // 离场归还本图材质强句柄，不能带入下一地图。
    }
    volume::reset_preview(&mut commands, &mut topology, garbage.as_deref_mut());
    if let Some(garbage) = garbage.as_deref_mut() {
        for surface in topology.surfaces.iter() {
            garbage.retain_shape(&surface.shape);
        }
    }
    topology.map = None;
    topology.root = None;
    topology.desired = Arc::default();
    topology.applied = Arc::default();
    topology.surfaces = Arc::default();
    topology.active_roots.clear();
    topology.retire_candidate(garbage.as_deref_mut());
    topology.applied_candidate = None;
    topology.revision = topology.revision.wrapping_add(1);
    ray_sources.batches.clear();
    ray_sources.revision = ray_sources.revision.wrapping_add(1);
    if let Some(preview) = preview.as_deref_mut() {
        preview.target = None;
        preview.adopted = None;
        preview.adopted_removal = false;
    }
}

pub(crate) fn collect_terrain_topology(
    mut commands: Commands,
    active: feathrise_scene_core::residency::InstanceRes<ActiveDynamicMap>,
    map: feathrise_scene_core::residency::InstanceRes<DynamicMapRuntimeState>,
    placements: Option<feathrise_scene_core::residency::InstanceRes<MapPlacementRuntimeState>>,
    mut topology: feathrise_scene_core::residency::InstanceResMut<TerrainTopologyRuntime>,
    status: Option<feathrise_scene_core::residency::InstanceRes<crate::MapPlacementRuntimeStatus>>,
    mut ray_sources: feathrise_scene_core::residency::InstanceResMut<StaticTerrainRaytracingSources>,
    catalog: Option<crate::maps::dynamic_map::sources::MapRuntimeCatalog>,
    buildings: Option<Res<feathrise_map_content::BuildingPresetRegistry>>,
    colliders: Query<(
        &TerrainPatchColliderReady,
        &crate::TerrainPatchCollisionRequest,
        &GlobalTransform,
        &crate::MapPlacementInstance,
    )>,
    containers: Query<&Visibility>,
    mut garbage: Option<ResMut<crate::shared::placement_data::TerrainCollisionGarbage>>,
    preview: Option<crate::maps::dynamic_map::author_sources::TerrainAuthorRes<TerrainVolumePreviewRequest>>,
    frames: TerrainTopologyFrame,
) {
    if preview.as_ref().is_some_and(|preview| preview.is_container()) { return; } // 空容器不能重置由兼容主对象拥有的共享作者预览。
    let scope = active.map_id.as_ref().zip(map.root).filter(|(id, _)| {
        map.active_map_id.as_ref() == Some(*id) && map.sparse_height_path.is_some()
    });
    let Some((id, root)) = scope else {
        volume::reset_preview(&mut commands, &mut topology, garbage.as_deref_mut());
        if topology.map.is_some()
            && let Some(garbage) = garbage.as_deref_mut()
        {
            for surface in topology.surfaces.iter() {
                garbage.retain_shape(&surface.shape);
            }
        }
        if topology.map.take().is_some() {
            topology.root = None;
            topology.desired = Arc::default();
            topology.applied = Arc::default();
            topology.surfaces = Arc::default();
            topology.active_roots.clear();
            topology.retire_candidate(garbage.as_deref_mut());
            topology.applied_candidate = None;
            topology.revision = topology.revision.wrapping_add(1);
            ray_sources.batches.clear();
            ray_sources.revision = ray_sources.revision.wrapping_add(1);
        }
        return;
    };
    if topology.map.as_ref() != Some(id) || topology.root != Some(root) {
        volume::reset_preview(&mut commands, &mut topology, garbage.as_deref_mut());
        if let Some(garbage) = garbage.as_deref_mut() {
            for surface in topology.surfaces.iter() {
                garbage.retain_shape(&surface.shape);
            }
        }
        topology.map = Some(id.clone());
        topology.root = Some(root);
        topology.desired = Arc::default();
        topology.applied = Arc::default();
        topology.surfaces = Arc::default();
        topology.active_roots.clear();
        topology.retire_candidate(garbage.as_deref_mut());
        topology.applied_candidate = None;
        topology.revision = topology.revision.wrapping_add(1);
        ray_sources.batches.clear();
        ray_sources.revision = ray_sources.revision.wrapping_add(1);
    }
    let desired = placements.as_deref().and_then(|state| {
        let pending = state.pending.as_ref()?;
        let staging = state.staging.as_ref()?;
        (
            pending.map_id == *id
                && state.active_map_id.as_ref() == Some(id)
                && !state.dirty
                && pending.generation == state.generation
                && pending.revision == state.revision
                && catalog.as_deref().is_none_or(|catalog| catalog.revision == state.catalog_revision)
                && buildings.as_deref().is_none_or(|buildings| buildings.revision == pending.building_preset_revision)
                && state.scene_load.as_ref().is_some_and(|tracker| {
                    tracker.generation == pending.generation
                        && tracker.revision == pending.revision
                        && tracker.ready_scene_ids.len() == tracker.scene_ids.len()
                        && tracker.scene_checks.is_empty()
                })
                // 本帧新JSON、目录换代及尚未复查的资产必须先撤销开洞权。
                && status
                    .as_deref()
                    .is_none_or(|status| status.phase != crate::MapPlacementRuntimePhase::Failed)
                && staging.generation == state.generation
                && staging.revision == state.revision
                && staging.next_plan == staging.total_instances
                && staging.ready_roots.len() == staging.total_instances
                && staging.verification_cursor == staging.total_instances
                && staging.verification_epoch == staging.eligibility_epoch
                && staging.blocked_scene_assets.is_empty()
                && state.world_asset_events_caught_up
                && containers.get(staging.root).is_ok()
            // 在撤掉高度承托前证明新可见容器确实仍存在。
        )
        .then_some((pending, staging))
    });
    topology.streamed_formal_ready = false;
    let desired = if let Some((pending, staging)) = desired {
        let key = (
            [pending.generation, pending.revision],
            staging.eligibility_epoch,
        );
        if topology
            .candidate
            .as_ref()
            .is_none_or(|candidate| (candidate.0, candidate.1) != key)
        {
            let mut surfaces = Vec::with_capacity(staging.terrain_roots.len());
            let mut batches = Vec::with_capacity(staging.terrain_roots.len());
            for root in &staging.terrain_roots {
                let Ok((shape, request, transform, _)) = colliders.get(*root) else {
                    topology.desired = topology.applied.clone();
                    topology.retire_candidate(garbage.as_deref_mut());
                    return;
                };
                let matrix = frames.inverse() * transform.to_matrix();
                if !shape.matches(request)
                    || shape.scale != transform.scale()
                    || !matrix.is_finite()
                    || !matrix.inverse().is_finite()
                {
                    topology.desired = topology.applied.clone();
                    topology.retire_candidate(garbage.as_deref_mut());
                    return;
                }
                surfaces.push(
                    TerrainMeshQuery::new(shape.collider.clone(), matrix)
                        .expect("上方已验证有限可逆矩阵"),
                );
                batches.push(shape.meshes.clone());
            }
            topology.retire_candidate(garbage.as_deref_mut());
            topology.candidate = Some((
                key.0,
                key.1,
                Arc::from(surfaces),
                batches,
                staging.terrain_roots.clone(),
            ));
        }
        let streaming = preview
            .as_deref()
            .and_then(|p| p.target.as_ref())
            .is_some_and(|t| {
                t.map_id == *id
                    && t.record
                        .terrain_patch
                        .as_ref()
                        .is_some_and(|p| p.streamed_volume)
            });
        if streaming {
            if topology.volume.formal_key != Some(key) {
                if let Some(base) = topology.volume.base.take()
                    && let Some(garbage) = garbage.as_deref_mut()
                {
                    for surface in base.surfaces.iter() {
                        garbage.retain_shape(&surface.shape);
                    }
                }
                let formal = topology.candidate.as_ref().unwrap();
                topology.volume.base = Some(resources::VolumeFormalBase {
                    holes: pending.terrain_holes.clone(),
                    surfaces: formal.2.clone(),
                    roots: formal.4.clone(),
                });
                topology.volume.formal_key = Some(key);
                topology.volume.force_refresh = true;
            }
            let holes = volume::collect_preview(
                &mut topology,
                preview.as_deref(),
                &ray_sources,
                &colliders,
                garbage.as_deref_mut(),
                frames.inverse(),
            )
            .unwrap_or_else(|| topology.applied.clone());
            topology.streamed_formal_ready = !topology.volume.force_refresh
                && topology
                    .volume
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.holes == holes);
            holes
        } else {
            pending.terrain_holes.clone()
        }
    } else if let Some(holes) = volume::collect_preview(
        &mut topology,
        preview.as_deref(),
        &ray_sources,
        &colliders,
        garbage.as_deref_mut(),
        frames.inverse(),
    ) {
        topology.retire_candidate(garbage.as_deref_mut());
        holes
    } else {
        topology.retire_candidate(garbage.as_deref_mut());
        refresh_active_terrain(
            &mut commands,
            &mut topology,
            &mut ray_sources,
            &colliders,
            garbage.as_deref_mut(),
            frames.inverse(),
        );
        topology.applied.clone()
    }; // 取消/热更撤销候选时保持旧完整洞口。
    if topology.desired != desired {
        topology.desired = desired;
    }
}

/// 正式GLB热更只在全部已采用根取得当前碰撞后共同换入查询和光追源。
fn refresh_active_terrain(
    commands: &mut Commands,
    topology: &mut TerrainTopologyRuntime,
    ray_sources: &mut StaticTerrainRaytracingSources,
    colliders: &Query<(
        &TerrainPatchColliderReady,
        &crate::TerrainPatchCollisionRequest,
        &GlobalTransform,
        &crate::MapPlacementInstance,
    )>,
    garbage: Option<&mut crate::shared::placement_data::TerrainCollisionGarbage>,
    source_inverse: Mat4,
) {
    if topology.active_roots.is_empty() {
        return;
    }
    let mut changed = topology.surfaces.len() != topology.active_roots.len()
        || ray_sources.batches.len() != topology.active_roots.len();
    for (index, root) in topology.active_roots.iter().enumerate() {
        let Ok((ready, request, transform, instance)) = colliders.get(*root) else {
            return;
        };
        let matrix = source_inverse * transform.to_matrix();
        if !instance.is_active()
            || instance.map_id() != topology.map.as_ref().expect("活动集合属于当前地图")
            || !instance.is_terrain_patch()
            || !ready.matches(request)
            || ready.scale != transform.scale()
            || !matrix.is_finite()
            || !matrix.inverse().is_finite()
        {
            return;
        } // 同句柄换代期间沿用上一完整查询，不能把新可视源与旧碰撞拼接。
        changed |= topology
            .surfaces
            .get(index)
            .is_none_or(|old| !old.matches(&ready.collider, matrix))
            || ray_sources
                .batches
                .get(index)
                .is_none_or(|old| !Arc::ptr_eq(old, &ready.meshes));
    }
    if !changed {
        return;
    } // 稳定帧只核对32项固定元数据，不分配查询或触发TLAS换代。
    let mut surfaces = Vec::with_capacity(topology.active_roots.len());
    let mut batches = Vec::with_capacity(topology.active_roots.len());
    for root in &topology.active_roots {
        let (ready, _, transform, _) = colliders.get(*root).expect("本系统已完整复验活动根");
        commands
            .entity(*root)
            .insert(crate::shared::placement_data::InstallTerrainPatchCollider); // 同批拾取与光追发布后、下一次物理步前统一替换。
        surfaces.push(
            TerrainMeshQuery::new(ready.collider.clone(), source_inverse * transform.to_matrix())
                .expect("已复验可逆变换"),
        );
        batches.push(ready.meshes.clone());
    }
    if let Some(garbage) = garbage {
        for old in topology.surfaces.iter() {
            garbage.retain_shape(&old.shape);
        }
    }
    topology.surfaces = surfaces.into();
    topology.revision = topology.revision.wrapping_add(1);
    ray_sources.batches = batches;
    ray_sources.revision = ray_sources.revision.wrapping_add(1);
}

pub(crate) fn publish_terrain_topology(
    colliders: Query<(
        &TerrainPatchColliderReady,
        &crate::TerrainPatchCollisionRequest,
        &GlobalTransform,
        &crate::MapPlacementInstance,
    )>,
    mut commands: Commands,
    mut topology: feathrise_scene_core::residency::InstanceResMut<TerrainTopologyRuntime>,
    mut placements: Option<feathrise_scene_core::residency::InstanceResMut<MapPlacementRuntimeState>>,
    mut ray_sources: feathrise_scene_core::residency::InstanceResMut<StaticTerrainRaytracingSources>,
    mut garbage: Option<ResMut<crate::shared::placement_data::TerrainCollisionGarbage>>,
    mut preview: Option<crate::maps::dynamic_map::author_sources::TerrainAuthorResMut<TerrainVolumePreviewRequest>>,
    frames: TerrainTopologyFrame,
) {
    if topology.desired == topology.applied
        && let Some(candidate) = topology.candidate.as_ref()
        && topology.applied_candidate != Some((candidate.0, candidate.1))
        && (preview
            .as_deref()
            .and_then(|p| p.target.as_ref())
            .is_none_or(|t| {
                !t.record
                    .terrain_patch
                    .as_ref()
                    .is_some_and(|p| p.streamed_volume)
            })
            || topology.streamed_formal_ready)
    {
        let (key, epoch, surfaces, batches, roots) = (
            candidate.0,
            candidate.1,
            candidate.2.clone(),
            candidate.3.clone(),
            candidate.4.clone(),
        );
        if let Some(garbage) = garbage.as_deref_mut() {
            for surface in topology.surfaces.iter() {
                garbage.retain_shape(&surface.shape);
            }
        }
        let streaming = preview
            .as_deref()
            .and_then(|p| p.target.as_ref())
            .is_some_and(|t| {
                t.record
                    .terrain_patch
                    .as_ref()
                    .is_some_and(|p| p.streamed_volume)
            });
        if !streaming {
            topology.surfaces = surfaces;
            topology.active_roots = roots;
        }
        topology.applied_candidate = Some((key, epoch));
        topology.revision = topology.revision.wrapping_add(1);
        if !streaming {
            volume::finish_formal_publish(&mut commands, &mut topology, garbage.as_deref_mut());
        }
        if !streaming
            && (ray_sources.batches.len() != batches.len()
                || ray_sources
                    .batches
                    .iter()
                    .zip(&batches)
                    .any(|(a, b)| !Arc::ptr_eq(a, b)))
        {
            ray_sources.batches = batches;
            ray_sources.revision = ray_sources.revision.wrapping_add(1);
        } // 普通摆放保存且没有三维地形时，不重建高度TLAS。
    }
    volume::publish_preview(
        &mut commands,
        &mut topology,
        &mut ray_sources,
        garbage.as_deref_mut(),
        &colliders,
        frames.inverse(),
    );
    if let Some(preview) = preview.as_deref_mut() {
        preview.adopted = topology.volume.active_root;
        preview.adopted_removal = topology.volume.active_removal;
    }
    if let Some(state) = placements.as_deref_mut() {
        state.terrain_holes_applied = if state.active_map_id == topology.map {
            // 正式placement门只核对正式洞口；驻留覆盖已在同一次拓扑发布中单独获得完整证明。
            if preview
                .as_deref()
                .and_then(|p| p.target.as_ref())
                .is_some_and(|t| {
                    t.record
                        .terrain_patch
                        .as_ref()
                        .is_some_and(|p| p.streamed_volume)
                })
            {
                state
                    .pending
                    .as_ref()
                    .filter(|p| {
                        Some([p.generation, p.revision]) == topology.applied_candidate.map(|c| c.0)
                    })
                    .map(|p| p.terrain_holes.clone())
                    .unwrap_or_else(|| state.terrain_holes_applied.clone())
            } else {
                topology.applied.clone()
            }
        } else {
            Arc::default()
        };
        state.terrain_mesh_generation = topology.applied_candidate.map(|candidate| candidate.0);
    }
}

/// 页候选和正式材质重包后同步固定洞口，避免新页把洞口清回封闭高度面。
pub(crate) fn sync_topology_materials(
    topology: feathrise_scene_core::residency::InstanceRes<TerrainTopologyRuntime>,
    mut pages: feathrise_scene_core::residency::InstanceResMut<super::clipmap::SparseHeightPageRuntime>,
    mut white: ResMut<Assets<super::clipmap::ClipmapHeightMaterial>>,
    mut surfaces: ResMut<Assets<super::clipmap::SparseClipmapSurfaceMaterial>>,
) {
    pages.synchronize_holes(
        super::clipmap::material::ClipmapHoleUniform::from_holes(&topology.applied),
        topology.revision,
        &mut white,
        &mut surfaces,
    );
}

#[cfg(test)]
#[path = "terrain_topology_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "terrain_topology/volume_tests.rs"]
mod volume_tests;
