//! 体积候选只在高度面让位及真实形状同代后发布；取消时按当前Ready还原正式集合。
use super::*;
use resources::{VolumeFormalBase, VolumePreviewCandidate};

pub(super) fn collect_preview(
    topology: &mut TerrainTopologyRuntime,
    request: Option<&TerrainVolumePreviewRequest>,
    rays: &StaticTerrainRaytracingSources,
    roots: &Query<(
        &TerrainPatchColliderReady,
        &crate::TerrainPatchCollisionRequest,
        &GlobalTransform,
        &crate::ScenePlacementInstance,
    )>,
    mut garbage: Option<&mut crate::shared::placement_data::TerrainCollisionGarbage>,
    source_inverse: Mat4,
) -> Option<Arc<[SceneTerrainCutout]>> {
    let target = request.and_then(|r| r.target.as_ref()).filter(|r| {
        topology.scene_id.as_ref() == Some(&r.scene_id) && topology.volume.ignored_root != Some(r.entity)
    });
    if let Some(target) = target {
        let bounds = Rect::from_corners(Vec2::splat(-1.0e8), Vec2::splat(1.0e8));
        let target_holes = target.record.terrain_cutouts(bounds).ok();
        let valid = roots
            .get(target.entity)
            .is_ok_and(|(ready, collision, global, instance)| {
                ready.matches(collision)
                    && ready.scale == global.scale()
                    && instance.is_active()
                    && instance.matches_record(&target.scene_id, &target.record, &[])
                    && global.to_matrix().is_finite()
                    && global.to_matrix().inverse().is_finite()
                    && target
                        .record
                        .terrain_patch
                        .as_ref()
                        .is_some_and(|p| p.sculpted_volume)
                    && target.world_cutout.validate().is_ok()
                    && target.record.terrain_cutout(bounds).ok().flatten()
                        == Some(target.world_cutout)
                    && target_holes.as_ref().is_some_and(|v| !v.is_empty())
            });
        if !valid {
            clear_pending(topology, garbage.as_deref_mut());
            return Some(if topology.volume.base.is_some() {
                topology.volume.active_holes.clone()
            } else {
                topology.applied.clone()
            });
        }
        if let Some(pending) = &topology.volume.pending
            && !topology.volume.force_refresh
            && pending.root == Some(target.entity)
            && pending.removal == target.removal
            && candidate_valid(pending, roots, source_inverse)
        {
            return Some(pending.holes.clone());
        }
        if topology.volume.active_root == Some(target.entity)
            && !topology.volume.force_refresh
            && topology.volume.pending.is_none()
            && topology.volume.active_removal == target.removal
            && proof_valid(
                topology.volume.active_proof.as_ref(),
                Some(target.entity),
                roots,
                source_inverse,
            )
            && collection_valid(
                &topology.active_roots,
                &topology.surfaces,
                &rays.batches,
                roots,
                source_inverse,
            )
        {
            return Some(topology.volume.active_holes.clone());
        }
        if topology.volume.base.is_none() {
            topology.volume.active_holes = topology.applied.clone();
            topology.volume.base = Some(VolumeFormalBase {
                holes: topology.applied.clone(),
                surfaces: topology.surfaces.clone(),
                roots: topology.active_roots.clone(),
            });
        }
        let base = topology.volume.base.as_ref().unwrap();
        let mut surfaces = Vec::new();
        let mut ids = Vec::new();
        let mut batches = Vec::new();
        let mut hidden = Vec::new();
        let mut replaced_holes = Vec::new();
        for &root in &base.roots {
            let Ok((ready, collision, global, instance)) = roots.get(root) else {
                clear_pending(topology, garbage.as_deref_mut());
                return Some(topology.volume.active_holes.clone());
            };
            if instance.id() == target.record.id {
                if let Some(patch) = instance.terrain_patch() {
                    let offset = (source_inverse * global.to_matrix()).transform_point3(Vec3::ZERO).xz();
                    replaced_holes.extend(patch.cutout.iter().chain(&patch.cutouts).map(|h| {
                        SceneTerrainCutout {
                            min: (Vec2::from_array(h.min) + offset).to_array(),
                            max: (Vec2::from_array(h.max) + offset).to_array(),
                        }
                    }));
                }
                hidden.push(root);
                continue;
            }
            if !ready.matches(collision) || ready.scale != global.scale() || !instance.is_active() {
                clear_pending(topology, garbage.as_deref_mut());
                return Some(topology.volume.active_holes.clone());
            }
            let Ok(query) = TerrainMeshQuery::new(ready.collider.clone(), source_inverse * global.to_matrix())
            else {
                clear_pending(topology, garbage.as_deref_mut());
                return Some(topology.volume.active_holes.clone());
            };
            surfaces.push(query);
            ids.push(root);
            batches.push(ready.meshes.clone());
        }
        if ids.len() >= feathrise_scene_content::MAX_TERRAIN_CUTOUTS {
            return Some(topology.volume.active_holes.clone());
        }
        let mut holes: Vec<_> = base
            .holes
            .iter()
            .copied()
            .filter(|h| !replaced_holes.contains(h))
            .collect();
        if holes.len() >= feathrise_scene_content::MAX_TERRAIN_CUTOUTS {
            return Some(topology.volume.active_holes.clone());
        }
        let (ready, _, global, _) = roots.get(target.entity).unwrap();
        let proof = TerrainMeshQuery::new(ready.collider.clone(), source_inverse * global.to_matrix()).unwrap();
        if !target.removal {
            holes.extend(target_holes.unwrap());
            if holes.len() > feathrise_scene_content::MAX_TERRAIN_CUTOUTS {
                clear_pending(topology, garbage.as_deref_mut());
                return Some(topology.volume.active_holes.clone());
            }
            surfaces.push(proof.clone());
            ids.push(target.entity);
            batches.push(ready.meshes.clone());
        }
        let holes: Arc<[_]> = holes.into();
        clear_pending(topology, garbage.as_deref_mut());
        topology.volume.pending = Some(VolumePreviewCandidate {
            proof: Some(proof),
            root: Some(target.entity),
            holes: holes.clone(),
            surfaces: surfaces.into(),
            roots: ids,
            batches,
            hidden_formal: hidden,
            removal: target.removal,
            owned: !target.removal
                || (topology.volume.active_root == Some(target.entity)
                    && topology.volume.active_owned),
        });
        topology.volume.force_refresh = false;
        return Some(holes);
    }
    let base = topology.volume.base.as_ref()?;
    if let Some(pending) = &topology.volume.pending
        && pending.root.is_none()
        && candidate_valid(pending, roots, source_inverse)
    {
        return Some(pending.holes.clone());
    }
    // 取消时重读正式Ready，不能恢复已经热更失效的旧BVH或旧Mesh批次。
    let mut surfaces = Vec::new();
    let mut batches = Vec::new();
    for &root in &base.roots {
        let Ok((ready, collision, global, instance)) = roots.get(root) else {
            clear_pending(topology, garbage.as_deref_mut());
            return Some(topology.volume.active_holes.clone());
        };
        if !ready.matches(collision) || ready.scale != global.scale() || !instance.is_active() {
            clear_pending(topology, garbage.as_deref_mut());
            return Some(topology.volume.active_holes.clone());
        }
        let Ok(query) = TerrainMeshQuery::new(ready.collider.clone(), source_inverse * global.to_matrix()) else {
            clear_pending(topology, garbage.as_deref_mut());
            return Some(topology.volume.active_holes.clone());
        };
        surfaces.push(query);
        batches.push(ready.meshes.clone());
    }
    let holes = base.holes.clone();
    let ids = base.roots.clone();
    clear_pending(topology, garbage);
    topology.volume.pending = Some(VolumePreviewCandidate {
        proof: None,
        root: None,
        holes: holes.clone(),
        surfaces: surfaces.into(),
        roots: ids,
        batches,
        hidden_formal: Vec::new(),
        removal: false,
        owned: false,
    });
    Some(holes)
}

fn proof_valid(
    proof: Option<&TerrainMeshQuery>,
    root: Option<Entity>,
    roots: &Query<(
        &TerrainPatchColliderReady,
        &crate::TerrainPatchCollisionRequest,
        &GlobalTransform,
        &crate::ScenePlacementInstance,
    )>,
    source_inverse: Mat4,
) -> bool {
    match (proof, root) {
        (Some(proof), Some(root)) => {
            roots
                .get(root)
                .is_ok_and(|(ready, request, global, instance)| {
                    ready.matches(request)
                        && ready.scale == global.scale()
                        && instance.is_active()
                        && proof.matches(&ready.collider, source_inverse * global.to_matrix())
                })
        }
        (None, None) => true,
        _ => false,
    }
}
fn collection_valid(
    ids: &[Entity],
    surfaces: &[TerrainMeshQuery],
    batches: &[Arc<[(Entity, Handle<Mesh>)]>],
    roots: &Query<(
        &TerrainPatchColliderReady,
        &crate::TerrainPatchCollisionRequest,
        &GlobalTransform,
        &crate::ScenePlacementInstance,
    )>,
    source_inverse: Mat4,
) -> bool {
    ids.len() == surfaces.len()
        && ids.len() == batches.len()
        && ids.iter().enumerate().all(|(i, &root)| {
            roots
                .get(root)
                .is_ok_and(|(ready, request, global, instance)| {
                    ready.matches(request)
                        && ready.scale == global.scale()
                        && instance.is_active()
                        && surfaces[i].matches(&ready.collider, source_inverse * global.to_matrix())
                        && Arc::ptr_eq(&batches[i], &ready.meshes)
                })
        })
}
fn candidate_valid(
    candidate: &VolumePreviewCandidate,
    roots: &Query<(
        &TerrainPatchColliderReady,
        &crate::TerrainPatchCollisionRequest,
        &GlobalTransform,
        &crate::ScenePlacementInstance,
    )>,
    source_inverse: Mat4,
) -> bool {
    proof_valid(candidate.proof.as_ref(), candidate.root, roots, source_inverse)
        && collection_valid(
            &candidate.roots,
            &candidate.surfaces,
            &candidate.batches,
            roots,
            source_inverse,
        )
}

pub(super) fn publish_preview(
    commands: &mut Commands,
    topology: &mut TerrainTopologyRuntime,
    rays: &mut StaticTerrainRaytracingSources,
    mut garbage: Option<&mut crate::shared::placement_data::TerrainCollisionGarbage>,
    roots: &Query<(
        &TerrainPatchColliderReady,
        &crate::TerrainPatchCollisionRequest,
        &GlobalTransform,
        &crate::ScenePlacementInstance,
    )>,
    source_inverse: Mat4,
) {
    if topology
        .volume
        .pending
        .as_ref()
        .is_none_or(|p| p.holes != topology.applied || !candidate_valid(p, roots, source_inverse))
    {
        return;
    }
    let pending = topology.volume.pending.take().unwrap();
    retain_surfaces(garbage.as_deref_mut(), &topology.surfaces);
    if let Some(proof) = topology.volume.active_proof.take() {
        retain_surfaces(garbage.as_deref_mut(), &[proof]);
    }
    if let Some(old) = topology
        .volume
        .active_root
        .take()
        .filter(|r| Some(*r) != pending.root)
        && topology.volume.active_owned
    {
        commands.entity(old).try_despawn();
    }
    for &root in &pending.hidden_formal {
        commands.entity(root).insert(Visibility::Hidden);
        crate::shared::placement_data::suspend_terrain_collider(commands, root);
    }
    if pending.removal
        && let Some(root) = pending.root
    {
        commands.entity(root).insert(Visibility::Hidden);
        crate::shared::placement_data::suspend_terrain_collider(commands, root);
    }
    // 另一块正式地形重新进入集合时也重装，不能留下上一个预览隐藏的正式根。
    for &root in &pending.roots {
        commands.entity(root).try_insert((
            Visibility::Inherited,
            crate::shared::placement_data::InstallTerrainPatchCollider,
        ));
    }
    if pending.root.is_none() {
        if let Some(base) = topology.volume.base.take() {
            retain_surfaces(garbage, &base.surfaces);
        }
    }
    topology.surfaces = pending.surfaces;
    topology.active_roots = pending.roots;
    topology.volume.active_root = pending.root;
    topology.volume.active_owned = pending.owned;
    topology.volume.active_removal = pending.removal;
    topology.volume.active_proof = pending.proof;
    topology.volume.active_holes = pending.holes;
    topology.revision = topology.revision.wrapping_add(1);
    rays.batches = pending.batches;
    rays.revision = rays.revision.wrapping_add(1);
}
pub(super) fn finish_formal_publish(
    commands: &mut Commands,
    topology: &mut TerrainTopologyRuntime,
    mut garbage: Option<&mut crate::shared::placement_data::TerrainCollisionGarbage>,
) {
    if let Some(root) = topology.volume.active_root.take() {
        topology.volume.ignored_root = Some(root);
        if topology.volume.active_owned {
            commands.entity(root).try_despawn();
        }
    }
    if let Some(base) = topology.volume.base.take() {
        retain_surfaces(garbage.as_deref_mut(), &base.surfaces);
    }
    if let Some(proof) = topology.volume.active_proof.take() {
        retain_surfaces(garbage.as_deref_mut(), &[proof]);
    }
    if let Some(pending) = topology.volume.pending.take() {
        retain_surfaces(garbage.as_deref_mut(), &pending.surfaces);
        if let Some(proof) = pending.proof {
            retain_surfaces(garbage, &[proof]);
        }
        if let Some(root) = pending.root {
            topology.volume.ignored_root = Some(root);
            if pending.owned {
                commands.entity(root).try_despawn();
            }
        }
    }
    topology.volume.active_owned = false;
    topology.volume.active_removal = false;
    topology.volume.formal_key = None;
    topology.volume.force_refresh = false;
}
pub(super) fn reset_preview(
    commands: &mut Commands,
    topology: &mut TerrainTopologyRuntime,
    garbage: Option<&mut crate::shared::placement_data::TerrainCollisionGarbage>,
) {
    finish_formal_publish(commands, topology, garbage);
    topology.volume.ignored_root = None;
}
fn clear_pending(
    topology: &mut TerrainTopologyRuntime,
    mut garbage: Option<&mut crate::shared::placement_data::TerrainCollisionGarbage>,
) {
    if let Some(old) = topology.volume.pending.take() {
        retain_surfaces(garbage.as_deref_mut(), &old.surfaces);
        if let Some(proof) = old.proof {
            retain_surfaces(garbage, &[proof]);
        }
    }
}
fn retain_surfaces(
    garbage: Option<&mut crate::shared::placement_data::TerrainCollisionGarbage>,
    surfaces: &[TerrainMeshQuery],
) {
    if let Some(garbage) = garbage {
        for surface in surfaces {
            garbage.retain_shape(&surface.shape);
        }
    }
}
