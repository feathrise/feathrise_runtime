//! 将已采用地表PBR绑定到雕刻体积面；高度位移、裁切与临时光追租约不得借入体积网格。

use super::{resources::VolumeSurfaceState, *};
use crate::{
    DynamicSparseClipmapPrimitive,
    maps::dynamic_map::clipmap::{
        SparseHeightPageRuntime,
        material::{SparseClipmapSurfaceLease, SparseClipmapSurfaceMaterial},
    },
};
use feathrise_environment::terrain_surface::{
    TerrainSurfaceMaterial, TerrainSurfaceMaterialOverride, TerrainVolumeSurface,
};

/// 【功能设计】体积网格包含冻结列的地表，不只是洞壁；白模会抹去整列原有材质。
/// 复制当前可见Clipmap的内层PBR和既有图/表句柄；外部地表沿用XZ涂绘，洞面独立叠加三维作者字段。
/// 根来自联合拓扑固定集合；显示块按游标分帧绑定，稳定帧只校验每根同代身份。
#[allow(clippy::too_many_arguments)]
pub(crate) fn sync_volume_surface(
    mut commands: Commands,
    active: feathrise_scene_core::residency::InstanceRes<ActiveDynamicMap>,
    map: feathrise_scene_core::residency::InstanceRes<DynamicMapRuntimeState>,
    pages: feathrise_scene_core::residency::InstanceRes<SparseHeightPageRuntime>,
    topology: feathrise_scene_core::residency::InstanceRes<TerrainTopologyRuntime>,
    preview: crate::maps::dynamic_map::author_sources::TerrainAuthorRes<TerrainVolumePreviewRequest>,
    mut state: feathrise_scene_core::residency::InstanceResMut<VolumeSurfaceState>,
    mut materials: ResMut<Assets<TerrainSurfaceMaterial>>,
    mut buffers: Option<ResMut<Assets<bevy::render::storage::ShaderBuffer>>>,
    wrapped: Res<Assets<SparseClipmapSurfaceMaterial>>,
    mut events: MessageReader<AssetEvent<TerrainSurfaceMaterial>>,
    displays: Query<
        (
            &MeshMaterial3d<SparseClipmapSurfaceMaterial>,
            &SparseClipmapSurfaceLease,
        ),
        With<DynamicSparseClipmapPrimitive>,
    >,
    mut roots: Query<(
        &mut TerrainPatchColliderReady,
        &crate::TerrainPatchCollisionRequest,
        &crate::MapPlacementInstance,
        Option<&crate::VolumeSurfaceFields>,
        Option<&Transform>,
    )>,
    meshes: Query<(
        &Mesh3d,
        Option<&MeshMaterial3d<TerrainSurfaceMaterial>>,
        Option<&TerrainSurfaceMaterialOverride>,
    )>,
    mut rays: feathrise_scene_core::residency::InstanceResMut<StaticTerrainRaytracingSources>,
) {
    let scope = active.map_id.as_ref().zip(map.root).filter(|(id, _)| {
        map.active_map_id.as_ref() == Some(*id) && map.sparse_height_path.is_some()
    });
    if state.scope.as_ref().map(|(id, root)| (id, *root)) != scope {
        // 同图保存交接更换场景根时，驻留体积可继续使用；CPU缓冲已移交，不能丢掉唯一GPU句柄。
        let fields = if state.scope.as_ref().map(|(id, _)| id) == scope.map(|(id, _)| id) {
            std::mem::take(&mut state.fields)
        } else {
            default()
        };
        *state = VolumeSurfaceState::default();
        state.fields = fields;
        state.scope = scope.map(|(id, root)| (id.clone(), root));
    }
    let source = scope.and_then(|(id, root)| pages.volume_surface_source(id, root));
    let source_modified = events.read().fold(false, |changed, event| {
        changed || matches!(event, AssetEvent::Modified { id } if source.as_ref().is_some_and(|s| s.id() == *id))
    }); // 消费全部消息；新增体积副本自身的事件不能形成更新循环。
    let Some((id, _)) = scope else {
        return;
    };
    if let Some(source) = source
        && let Ok((installed, lease)) = displays.single()
        && lease.matches(source.id(), installed.id())
        && let Some(author) = materials.get(&source)
        && let Some(visible) = wrapped.get(&installed.0)
    {
        let mut candidate = visible.base.clone();
        // 不能复制Clipmap外层：它会位移洞底并再次裁掉洞穴。其Deferred租约同样只属于高度代理。
        candidate.base.opaque_render_method = author.base.opaque_render_method;
        candidate
            .extension
            .set_dynamic_terrain_shadow_terminator_data(false);
        candidate.extension.set_volume_projection(true);
        let unchanged = state.source == Some(source.id())
            && !source_modified
            && state
                .material
                .as_ref()
                .and_then(|h| materials.get(h))
                .is_some_and(|current| current.extension == candidate.extension);
        if !unchanged {
            state.material = Some(materials.add(candidate));
            state.source = Some(source.id()); // 新句柄让旧Solari租约失效，不能继续显示旧涂绘副本。
        }
    } // 地表候选尚未采用或失效时沿用同图上一完整体积材质。
    let Some(material) = state.material.clone() else {
        return;
    };
    let mut selected = Vec::with_capacity(4 * feathrise_map_content::MAX_TERRAIN_CUTOUTS + 2);
    let mut retain = |root| {
        if !selected.contains(&root) {
            selected.push(root);
        }
    };
    for &root in &topology.active_roots {
        retain(root);
    }
    if let Some(candidate) = &topology.candidate {
        for &root in &candidate.4 {
            retain(root);
        }
    }
    if let Some(base) = &topology.volume.base {
        for &root in &base.roots {
            retain(root);
        }
    }
    if let Some(pending) = &topology.volume.pending {
        for &root in &pending.roots {
            retain(root);
        }
    }
    if let Some(target) = preview.target.as_ref().filter(|t| &t.map_id == id) {
        retain(target.entity);
    }
    state.fields.retain(|root, _| selected.contains(root)); // 只保留当前有界采用/候选集合。
    state.bindings.retain(|root, _| selected.contains(root));
    let mut published_changed = false;
    let mut remaining = 32usize;
    for root in selected {
        let Ok((mut ready, request, instance, fields, transform)) = roots.get_mut(root) else {
            continue;
        };
        if instance.map_id() != id
            || !(instance.is_active() || instance.is_pending())
            || !instance.terrain_patch().is_some_and(|p| p.sculpted_volume)
            || !ready.matches_geometry(request)
        {
            continue;
        }
        // 作者块已在计算池打包；主帧只移交一次缓冲所有权，并复用每根材质。
        let mut material = material.clone();
        if let Some(fields) = fields.filter(|f| f.0.cell_size > 0.0) {
            let identity = std::sync::Arc::as_ptr(&fields.0) as usize;
            if state
                .fields
                .get(&root)
                .is_none_or(|(id, _, _, _)| *id != identity)
                && let Some(buffers) = buffers.as_deref_mut()
                && let Ok(mut ready) = fields.1.try_lock()
                && let Some(buffer) = ready.take()
            {
                let buffer = buffers.add(buffer);
                state
                    .fields
                    .insert(root, (identity, buffer, material.clone(), material.id()));
            }
            if let Some((_, buffer, current, parent)) = state.fields.get_mut(&root)
                && let Some(mut candidate) = materials.get(&material).cloned()
            {
                candidate.extension.set_volume_fields(
                    buffer.clone(),
                    transform.map_or(Vec3::ZERO, |t| t.translation),
                    fields.0.cell_size,
                );
                if *parent != material.id()
                    || materials
                        .get(&*current)
                        .is_none_or(|old| old.extension != candidate.extension)
                {
                    *current = materials.add(candidate);
                    *parent = material.id(); // 字段缓冲复用，基础PBR换代只登记一次完整新材质。
                }
                material = current.clone();
            }
            if !state.fields.contains_key(&root) {
                continue; // 作者字段没有移交渲染资产时不得用无字段材质授信候选。
            }
        }
        let binding = state
            .bindings
            .entry(root)
            .or_insert_with(|| (ready.meshes.clone(), material.id(), 0));
        if !std::sync::Arc::ptr_eq(&binding.0, &ready.meshes) || binding.1 != material.id() {
            *binding = (ready.meshes.clone(), material.id(), 0);
            ready.surface_ready = false;
        }
        let end = (binding.2 + remaining).min(ready.meshes.len());
        let mut valid = true;
        for (entity, mesh) in &ready.meshes[binding.2..end] {
            remaining -= 1;
            let Ok((current_mesh, current, material_override)) = meshes.get(*entity) else {
                valid = false;
                break;
            };
            if current_mesh.id() != mesh.id() {
                valid = false;
                break;
            }
            if current.is_some_and(|current| {
                current.id() == material.id()
                    || material_override
                        .is_some_and(|lease| lease.matches(material.id(), current.id()))
            }) {
                continue;
            } // 精确租约仍有效时不抢回可见材质。
            commands
                .entity(*entity)
                .remove::<MeshMaterial3d<StandardMaterial>>()
                .remove::<TerrainSurfaceMaterialOverride>()
                .insert((MeshMaterial3d(material.clone()), TerrainVolumeSurface));
            // 根集合有固定元数据预算；不为每一块遍历所有已发布的网格。
            published_changed |= topology.active_roots.contains(&root);
        }
        if valid {
            binding.2 = end;
            ready.surface_ready = binding.2 == ready.meshes.len();
        }
    }
    if published_changed {
        rays.revision = rays.revision.wrapping_add(1); // 已采用体积材质换代进入同一光追事务，隐藏候选不触发重建。
    }
}

#[cfg(test)]
mod tests;
