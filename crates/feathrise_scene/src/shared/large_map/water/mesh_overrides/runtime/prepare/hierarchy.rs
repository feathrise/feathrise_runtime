//! 作者覆盖快照、World后代DFS和相关primitive路径的固定预算增量处理。

use bevy::prelude::*; // Children、Name、Entity及只读ECS查询。
use feathrise_map_content::{
    MAX_MAP_PLACEMENT_WATER_NAME_CHARS, MAX_MAP_PLACEMENT_WATER_NODE_PATH_CHARS,
    MAX_MAP_PLACEMENT_WATER_NODE_PATH_DEPTH, MapPlacementWaterSurface,
}; // 运行时快照与编辑器/JSON使用同一作者硬边界。

use super::super::super::{
    DynamicWaterMeshOverridePrimitive, ImportedPrimitiveData, PrimitiveCandidate,
}; // 复用根级场景、primitive和恢复所有权上限。
use super::work::{
    HierarchyFrame, MeshWaterRootWork, MeshWaterWorkStage, PendingCandidatePath, SurfaceIdentity,
}; // 跨帧游标和身份索引由单根工作集持有。

pub(super) const MESH_WATER_SURFACE_SNAPSHOT_BUDGET_PER_FRAME: usize = 2; // 作者路径复制与公共校验每帧最多两项。
pub(super) const MESH_WATER_HIERARCHY_OPS_PER_FRAME: usize = 128; // DFS节点、路径段或父链矩阵操作都不超过该预算。

/// 每帧验证并复制固定数量覆盖，同时建立精确身份HashMap和Mesh名快筛集合。
pub(super) fn advance_surface_snapshot(
    work: &mut MeshWaterRootWork,
    current: &[MapPlacementWaterSurface],
) -> Result<(), String> {
    if current.len() != work.expected_surface_count {
        return Err("整网格水体作者数组在增量快照期间发生长度变化".to_owned());
    }
    let end = work
        .snapshot_cursor
        .saturating_add(MESH_WATER_SURFACE_SNAPSHOT_BUDGET_PER_FRAME)
        .min(current.len());
    for surface in &current[work.snapshot_cursor..end] {
        surface.validate()?; // 单项名称、数值和路径边界在复制前失败关闭。
        let identity = SurfaceIdentity::from(surface); // 每帧只拥有固定数量合法身份。
        if work.identities.contains_key(&identity) {
            return Err(format!(
                "GLB节点路径{}下的网格{}被重复赋予水体覆盖",
                surface.node_path.join("/"),
                surface.mesh_name
            ));
        }
        let index = work.surfaces.len(); // 身份索引稳定指向拥有作者快照。
        work.mesh_names.insert(surface.mesh_name.clone()); // 后代先按名称O(1)拒绝无关primitive。
        work.identities.insert(identity, index); // 路径完成后不再逐项比较全部surface。
        work.surfaces.push(surface.clone()); // 最终提交不借用可能继续热改的根组件。
        work.matches.push(Vec::new()); // 每个作者项拥有独立多primitive桶。
    }
    work.snapshot_cursor = end; // 精确保存已验证前缀。
    if end == current.len() {
        work.stage = if current.is_empty() {
            MeshWaterWorkStage::Materials // 清空覆盖无需遍历World，材质阶段会直接进入Commit。
        } else {
            MeshWaterWorkStage::Traverse // 非空覆盖开始固定预算DFS。
        };
    }
    Ok(())
}

/// 每帧共享固定层级操作预算推进DFS或当前相关primitive的父链路径。
pub(super) fn advance_candidate_traversal(
    work: &mut MeshWaterRootWork,
    children: &Query<&Children>,
    world_roots: &Query<(), With<bevy::world_serialization::WorldAssetRoot>>,
    primitives: &Query<ImportedPrimitiveData>,
    nodes: &Query<(&Name, &ChildOf)>,
    overridden: &Query<&DynamicWaterMeshOverridePrimitive>,
) -> Result<(), String> {
    let mut budget = MESH_WATER_HIERARCHY_OPS_PER_FRAME;
    while budget > 0 {
        budget -= 1; // 路径一段、DFS一个子节点或一个完成帧弹出都计一次操作。
        if let Some(mut pending) = work.traversal.pending_path.take() {
            if pending.current == work.root {
                pending.candidate.node_path = pending.reversed;
                pending.candidate.node_path.reverse(); // 固定最多64段，转成浏览器同序作者路径。
                let identity = SurfaceIdentity {
                    node_path: pending.candidate.node_path.clone(),
                    mesh_name: pending.candidate.mesh_name.clone(),
                }; // 临时精确键只用于一次O(1)HashMap查询。
                if let Some(&surface_index) = work.identities.get(&identity) {
                    let bucket = &mut work.matches[surface_index];
                    bucket.push(pending.candidate); // 全量成功前只写工作内存，不改ECS。
                    work.matched_primitives += 1; // O(1)累计避免后代×surface桶扫描。
                }
                continue; // 未命中完整路径的相关Mesh名候选按原语义忽略。
            }
            let (name, parent) = nodes
                .get(pending.current)
                .map_err(|_| format!("网格对象节点{:?}缺少Name或ChildOf", pending.current))?;
            let parent_entity = parent.parent();
            if parent_entity == work.root {
                pending.current = work.root; // WorldAsset顶层scene root不进入作者路径。
                work.traversal.pending_path = Some(pending);
                continue;
            }
            if pending.reversed.len() >= MAX_MAP_PLACEMENT_WATER_NODE_PATH_DEPTH {
                return Err("实例层级超过整网格水体节点路径上限".to_owned());
            }
            let segment_chars = name
                .as_str()
                .chars()
                .take(MAX_MAP_PLACEMENT_WATER_NAME_CHARS + 1)
                .count();
            if segment_chars > MAX_MAP_PLACEMENT_WATER_NAME_CHARS {
                return Err(format!(
                    "网格节点{:?}名称超过整网格水体单段字符上限",
                    pending.current
                ));
            }
            pending.path_chars = pending.path_chars.saturating_add(segment_chars);
            if pending.path_chars > MAX_MAP_PLACEMENT_WATER_NODE_PATH_CHARS {
                return Err("网格节点路径超过整网格水体总字符上限".to_owned());
            }
            pending.reversed.push(name.as_str().to_owned()); // 每次只复制一个有界名称段。
            pending.current = parent_entity;
            work.traversal.pending_path = Some(pending); // 精确留到下一层路径操作。
            continue;
        }

        let Some(frame) = work.traversal.frames.last_mut() else {
            work.stage = MeshWaterWorkStage::Finalize; // DFS完整结束才开始逐surface匹配收口。
            return Ok(());
        };
        let Ok(items) = children.get(frame.parent) else {
            work.traversal.frames.pop(); // 叶节点完成也计本次层级操作。
            continue;
        };
        let Some(entity) = items.get(frame.next_child).copied() else {
            work.traversal.frames.pop(); // 当前父节点全部子项已完成。
            continue;
        };
        frame.next_child += 1; // 下一帧不会重复读取当前子项。
        work.traversal.descendants_seen += 1;
        if !world_roots.contains(entity) {
            work.traversal.frames.push(HierarchyFrame {
                parent: entity, // 嵌套World根不穿透，其余节点下一操作再按下标读取Children。
                next_child: 0,
            });
        }
        let Ok((
            _,
            mesh_name,
            mesh,
            material,
            transform,
            parent,
            has_no_frustum_culling,
            has_camera_raycast_ignore,
            has_not_shadow_caster,
            has_not_shadow_receiver,
        )) = primitives.get(entity)
        else {
            continue; // 非渲染节点只参与层级DFS。
        };
        if !work.mesh_names.contains(mesh_name.0.as_str()) {
            continue; // 无关Mesh名不构造路径，稳定消除后代×surfaces比较。
        }
        let previous = overridden.get(entity).ok();
        if let Some(previous) = previous
            && previous.owner_root != work.root
        {
            return Err(format!(
                "网格{entity:?}已由其他placement根{:?}接管",
                previous.owner_root
            ));
        }
        let object_node = parent.parent();
        work.traversal.pending_path = Some(PendingCandidatePath {
            candidate: PrimitiveCandidate {
                entity,
                object_node,
                node_path: Vec::new(),
                mesh_name: mesh_name.0.clone(),
                mesh: mesh.map(|value| value.0.clone()),
                original_material: previous
                    .map(|value| value.original_material.clone())
                    .or_else(|| material.map(|value| value.0.clone())),
                original_transform: previous
                    .map(|value| value.original_transform)
                    .or(transform.copied()),
                had_no_frustum_culling: previous
                    .map_or(has_no_frustum_culling, |value| value.had_no_frustum_culling),
                had_camera_raycast_ignore: previous.map_or(has_camera_raycast_ignore, |value| {
                    value.had_camera_raycast_ignore
                }),
                had_not_shadow_caster: previous
                    .map_or(has_not_shadow_caster, |value| value.had_not_shadow_caster),
                had_not_shadow_receiver: previous.map_or(has_not_shadow_receiver, |value| {
                    value.had_not_shadow_receiver
                }),
            },
            current: object_node,
            reversed: Vec::new(),
            path_chars: 0,
        }); // 下一预算操作才读取第一段路径，避免一次命中隐藏完整父链扫描。
    }
    Ok(())
}
