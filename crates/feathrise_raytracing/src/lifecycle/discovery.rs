//! 只在动态地形就绪后捕获明确目标，并整体核对资源预算。

use bevy::{
    gltf::{GltfMaterialName, GltfMeshName}, // 使用Blender原始双重身份，不依赖最终实体拼接Name。
    mesh::{PrimitiveTopology, VertexAttributeValues}, // 发现阶段冻结格式与数量。
    prelude::*,                             // Observer、Children、Mesh3d与资源系统参数。
};
use feathrise_environment::terrain_surface::TerrainSurfaceCarrier; // 通过显式地表承接标记选择扩展材质路径。
use feathrise_scene::{
    DYNAMIC_TERRAIN_MESH_NAME, DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME, DynamicSceneTerrainPrimitive,
}; // 复用场景crate的动态地形身份与完整就绪证据，禁止在光追crate复制字符串或加载状态机。
mod static_terrain;

use crate::{
    constants::{
        DYNAMIC_EXPECTED_INSTANCE_COUNT, DYNAMIC_EXPECTED_UNIQUE_MATERIAL_COUNT,
        DYNAMIC_MAX_TERRAIN_TRIANGLE_COUNT, DYNAMIC_MAX_TERRAIN_VERTEX_COUNT,
        MAX_CPU_WORKING_SET_BYTES,
        MAX_DERIVED_MESH_BYTES, MAX_INSTANCE_COUNT, MAX_TRIANGLE_COUNT, MAX_UNIQUE_MATERIAL_COUNT,
        MAX_UPLOAD_BYTES_PER_FRAME, MAX_VERTEX_COUNT,
    },
    resources::{
        MeshRevisionRegistry, RaytracingRuntime, RollbackDisposition,
    },
    types::{
        RaytracingBudgetSnapshot, RaytracingPrototypeBlocker, RaytracingPrototypeStage,
        RaytracingPrototypeTarget, SourceCandidate,
    },
};

/// 按当前场景目标在明确层级内一次发现完整候选集合，并在任何不一致时整体失败。
pub(crate) fn discover_candidates(
    sources: Query<(
        &GltfMeshName,
        &GltfMaterialName,
        &Mesh3d,
        Has<TerrainSurfaceCarrier>,
        Has<DynamicSceneTerrainPrimitive>,
        Has<feathrise_scene::ClipmapRaytracingAnchor>,
    )>, // 候选必须同时具备双身份、可见源Mesh和明确内容角色。
    meshes: Res<Assets<Mesh>>,  // 冻结当前已加载源统计与格式。
    mut revisions: ResMut<MeshRevisionRegistry>, // 为每个源AssetId取得当前修订。
    mut runtime: ResMut<RaytracingRuntime>, // 写入固定顺序候选和预算快照。
    terrain_sources: Option<Res<feathrise_scene::StaticTerrainRaytracingSources>>,
    static_sources: Query<static_terrain::StaticSourceData<'_>>,
) {
    if runtime.stage != RaytracingPrototypeStage::DiscoveringCandidates
        || runtime.pending_rollback.is_some()
    {
        return; // 事件与协调系统只让每个场景代次进入一次发现。
    }
    let Some(root) = runtime.root else {
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::WaitingForSupportedScene),
            RollbackDisposition::RasterReady,
        );
        return;
    };

    if runtime.target == RaytracingPrototypeTarget::DynamicSceneTerrain {
        discover_dynamic_terrain(
            root,
            &sources,
            &meshes,
            &mut revisions,
            &mut runtime,
            terrain_sources.as_deref(),
            &static_sources,
        );
        return;
    }
    runtime.request_rollback(
        Some(RaytracingPrototypeBlocker::WaitingForSupportedScene),
        RollbackDisposition::RasterReady,
    );
}

/// 动态场景先确认唯一高度源，再添加已联合采用的有限三维地形集合。
/// 普通摆放、水和植被不参与本次发现，也不重新遍历它们的层级。
fn discover_dynamic_terrain(
    root: Entity,
    sources: &Query<(
        &GltfMeshName,
        &GltfMaterialName,
        &Mesh3d,
        Has<TerrainSurfaceCarrier>,
        Has<DynamicSceneTerrainPrimitive>,
        Has<feathrise_scene::ClipmapRaytracingAnchor>,
    )>,
    meshes: &Assets<Mesh>,
    revisions: &mut MeshRevisionRegistry,
    runtime: &mut RaytracingRuntime,
    terrain_sources: Option<&feathrise_scene::StaticTerrainRaytracingSources>,
    static_sources: &Query<static_terrain::StaticSourceData<'_>>,
) {
    let Ok((
        mesh_name,
        material_name,
        mesh_handle,
        terrain_surface,
        dynamic_terrain,
        clipmap_anchor,
    )) = sources.get(root)
    else {
        fail_contract(
            runtime,
            "动态场景正式地形实体缺少GLTF双身份或Mesh3d".to_owned(),
        );
        return;
    };
    if !(dynamic_terrain || clipmap_anchor)
        || mesh_name.0 != DYNAMIC_TERRAIN_MESH_NAME
        || material_name.0 != DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME
    {
        fail_contract(
            runtime,
            format!(
                "动态地形身份应为{DYNAMIC_TERRAIN_MESH_NAME} + {DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME}，实际为{} + {}",
                mesh_name.0, material_name.0
            ),
        );
        return;
    }
    let Some(mesh) = meshes.get(mesh_handle.id()) else {
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::MeshUnavailable {
                detail: format!("{} + {}", mesh_name.0, material_name.0),
            }),
            RollbackDisposition::Failed,
        );
        return;
    };
    let (vertex_count, triangle_count) = match validate_source_mesh(mesh, false) {
        Ok(counts) => counts,
        Err(detail) => {
            runtime.request_rollback(
                Some(RaytracingPrototypeBlocker::MeshFormatUnsupported {
                    detail: format!("{} + {}：{detail}", mesh_name.0, material_name.0),
                }),
                RollbackDisposition::Failed,
            );
            return;
        }
    };
    let mesh_id = mesh_handle.id();
    let derived_bytes = vertex_count * 48 + triangle_count * 3 * 4;
    let candidate = SourceCandidate {
        contract_index: 0, // 高度源在候选表居首；实际GPU实例槽位另由完整绑定证明取得。
        mesh_name: mesh_name.0.clone(),
        material_name: material_name.0.clone(),
        requires_source_uv: false, // 动态RGBA地表使用世界XZ，白模与编辑中Mesh都无需UV。
        terrain_surface: terrain_surface || clipmap_anchor, // 隐藏 CPU 源用独立标记，不混入可见地表预热/绘制统计。
        entity: root,
        mesh: mesh_handle.0.clone(),
        mesh_id,
        source_revision: revisions.revision(mesh_id),
        vertex_count,
        triangle_count,
        derived_bytes,
        adaptation_ticket: None,
        derived_mesh: None,
    };
    let budget = RaytracingBudgetSnapshot {
        instances: 1,
        vertices: vertex_count,
        triangles: triangle_count,
        unique_materials: 1,
        derived_bytes,
        cpu_working_set_bytes: runtime.estimated_cpu_working_set(),
    };
    runtime.budget = budget;
    if let Err(detail) = validate_dynamic_budget(budget) {
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::AssetBudgetMismatch { detail }),
            RollbackDisposition::Failed,
        );
        return;
    }
    if derived_bytes > MAX_UPLOAD_BYTES_PER_FRAME {
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::AssetBudgetMismatch {
                detail: format!(
                    "动态地形派生字节{derived_bytes}超过单项上传硬上限{MAX_UPLOAD_BYTES_PER_FRAME}"
                ),
            }),
            RollbackDisposition::Failed,
        );
        return;
    }
    let mut candidates = vec![candidate];
    if let Some(published) = terrain_sources {
        let unique_materials = match static_terrain::append(
            &mut candidates,
            published,
            static_sources,
            meshes,
            revisions,
        ) {
            Ok(count) => count,
            Err(detail) => {
                fail_contract(runtime, detail);
                return;
            }
        };
        runtime.budget = RaytracingBudgetSnapshot {
            instances: candidates.len() as u32,
            vertices: candidates.iter().map(|c| c.vertex_count).sum(),
            triangles: candidates.iter().map(|c| c.triangle_count).sum(),
            unique_materials,
            derived_bytes: candidates.iter().map(|c| c.derived_bytes).sum(),
            cpu_working_set_bytes: budget.cpu_working_set_bytes,
        };
        if let Err(detail) = static_terrain::validate_budget(runtime.budget) {
            runtime.request_rollback(
                Some(RaytracingPrototypeBlocker::AssetBudgetMismatch { detail }),
                RollbackDisposition::Failed,
            );
            return;
        }
    }
    runtime.candidates = candidates;
    runtime.capture_cursor = 0;
    runtime.capture_job = None;
    runtime.blocker = None;
    runtime.transition(RaytracingPrototypeStage::AdaptingMeshes);
}

fn validate_source_mesh(mesh: &Mesh, requires_source_uv: bool) -> Result<(u64, u64), String> {
    if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
        return Err(format!("拓扑为{:?}", mesh.primitive_topology()));
    }
    let positions = mesh
        .attribute(Mesh::ATTRIBUTE_POSITION)
        .and_then(VertexAttributeValues::as_float3)
        .ok_or_else(|| "缺少Float32x3 Position".to_string())?;
    let normals = mesh
        .attribute(Mesh::ATTRIBUTE_NORMAL)
        .and_then(VertexAttributeValues::as_float3)
        .ok_or_else(|| "缺少Float32x3 Normal".to_string())?;
    if positions.is_empty() || positions.len() != normals.len() {
        return Err(format!(
            "Position={} Normal={} 数量不一致或为空",
            positions.len(),
            normals.len()
        ));
    }
    let uv = mesh.attribute(Mesh::ATTRIBUTE_UV_0);
    if requires_source_uv && uv.is_none() {
        return Err("固定地形缺少原UV0".to_string());
    }
    if let Some(uv) = uv {
        let VertexAttributeValues::Float32x2(uv) = uv else {
            return Err("UV0不是Float32x2".to_string());
        };
        if uv.len() != positions.len() {
            return Err(format!(
                "UV={} Position={} 数量不一致",
                uv.len(),
                positions.len()
            ));
        }
    }
    let indices = mesh
        .indices()
        .ok_or_else(|| "缺少显式U16/U32索引".to_string())?;
    if indices.is_empty() || !indices.len().is_multiple_of(3) {
        return Err(format!("索引数量{}不是非零三角列表", indices.len()));
    }
    Ok((positions.len() as u64, (indices.len() / 3) as u64))
}

/// 动态场景允许小测试图低于正式513²地形，但绝不允许越过正式单地形或全局硬预算。
fn validate_dynamic_budget(budget: RaytracingBudgetSnapshot) -> Result<(), String> {
    if budget.instances != DYNAMIC_EXPECTED_INSTANCE_COUNT
        || budget.unique_materials != DYNAMIC_EXPECTED_UNIQUE_MATERIAL_COUNT
        || budget.vertices > DYNAMIC_MAX_TERRAIN_VERTEX_COUNT
        || budget.triangles > DYNAMIC_MAX_TERRAIN_TRIANGLE_COUNT
    {
        return Err(format!(
            "动态目标应为{DYNAMIC_EXPECTED_INSTANCE_COUNT}实例/至多{DYNAMIC_MAX_TERRAIN_VERTEX_COUNT}顶点/{DYNAMIC_MAX_TERRAIN_TRIANGLE_COUNT}三角形/{DYNAMIC_EXPECTED_UNIQUE_MATERIAL_COUNT}材质，实际为{}实例/{}顶点/{}三角形/{}材质",
            budget.instances, budget.vertices, budget.triangles, budget.unique_materials
        ));
    }
    if budget.instances > MAX_INSTANCE_COUNT
        || budget.vertices > MAX_VERTEX_COUNT
        || budget.triangles > MAX_TRIANGLE_COUNT
        || budget.unique_materials > MAX_UNIQUE_MATERIAL_COUNT
        || budget.derived_bytes > MAX_DERIVED_MESH_BYTES
        || budget.cpu_working_set_bytes > MAX_CPU_WORKING_SET_BYTES
    {
        return Err(format!(
            "动态地形超过硬上限：实例{}/{MAX_INSTANCE_COUNT}，顶点{}/{MAX_VERTEX_COUNT}，三角形{}/{MAX_TRIANGLE_COUNT}，材质{}/{MAX_UNIQUE_MATERIAL_COUNT}，派生字节{}/{MAX_DERIVED_MESH_BYTES}，CPU工作集{}/{MAX_CPU_WORKING_SET_BYTES}",
            budget.instances,
            budget.vertices,
            budget.triangles,
            budget.unique_materials,
            budget.derived_bytes,
            budget.cpu_working_set_bytes
        ));
    }
    Ok(())
}

fn fail_contract(runtime: &mut RaytracingRuntime, detail: String) {
    runtime.request_rollback(
        Some(RaytracingPrototypeBlocker::CandidateContractMismatch { detail }),
        RollbackDisposition::Failed,
    );
}

#[cfg(test)]
mod tests; // 动态单地形预算回归独立存放，保持运行时代码低于500行。
