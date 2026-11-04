//! placement整网格水体的single-flight分帧预检协调入口。

use super::*; // 协调入口复用根组件、场景查询和两类只读/可写材质资产。

mod geometry; // 覆盖匹配、高度父链与逐primitive UV几何验证。
mod hierarchy; // 作者快照、后代DFS和相关primitive路径构建。
mod material; // 不可见动态材质的分帧登记与取消回收。
mod work; // 单根游标、请求修订和single-flight资源。

use geometry::{advance_geometry_validation, advance_surface_finalization}; // 每帧只推进当前明确几何阶段。
use hierarchy::{advance_candidate_traversal, advance_surface_snapshot}; // 层级与作者快照均有独立固定预算。
use material::advance_material_preparation; // 全量几何成功后才登记不可见临时材质。
pub(super) use material::discard_temporary_materials; // 失败和换代使用同一固定材质回收预算。
pub(in crate::shared::large_map::water) use work::DynamicWaterMeshOverrideWorkState; // 水插件只初始化资源，不读取内部游标。
pub(super) use work::{MeshWaterRootWork, MeshWaterWorkStage}; // runtime只操作single-flight生命周期，不读取各阶段内部游标。

/// 按当前stage推进一次固定预算；错误只返回诊断，调用方负责进入保旧清理状态。
#[allow(clippy::too_many_arguments)]
pub(super) fn advance_preparation(
    work: &mut MeshWaterRootWork,
    overrides: &DynamicWaterMeshOverrides,
    children: &Query<&Children>,
    world_roots: &Query<(), With<WorldAssetRoot>>,
    primitives: &Query<ImportedPrimitiveData>,
    nodes: &Query<(&Name, &ChildOf)>,
    local_hierarchy: &Query<(&Transform, Option<&ChildOf>)>,
    overridden: &Query<&DynamicWaterMeshOverridePrimitive>,
    meshes: &Assets<Mesh>,
    standard_materials: &Assets<StandardMaterial>,
    dynamic_materials: &mut Assets<DynamicWaterMaterial>,
    elapsed_seconds: f32,
) -> Result<(), String> {
    match work.stage {
        MeshWaterWorkStage::Snapshot => {
            advance_surface_snapshot(work, overrides.surfaces())?; // 每帧最多验证复制两项作者值。
        }
        MeshWaterWorkStage::Traverse => {
            advance_candidate_traversal(
                work,
                children,
                world_roots,
                primitives,
                nodes,
                overridden,
            )?; // DFS和路径段共享固定层级操作预算。
        }
        MeshWaterWorkStage::Finalize => {
            advance_surface_finalization(work, local_hierarchy, overridden)?; // 每次只推进当前覆盖父链。
        }
        MeshWaterWorkStage::Geometry => {
            advance_geometry_validation(work, local_hierarchy, overridden, meshes)?; // 顶点与三角各有独立预算。
        }
        MeshWaterWorkStage::Materials => {
            advance_material_preparation(
                work,
                elapsed_seconds,
                standard_materials,
                dynamic_materials,
            )?; // 每帧最多登记四份且仍不可见。
        }
        MeshWaterWorkStage::Discard | MeshWaterWorkStage::Commit => {}
    }
    Ok(())
}
