//! 后台 Mesh 转换结果的完整身份校验与当前失败事务发布。

use bevy::prelude::{AssetId, Mesh, Mesh3d, Query}; // 读取源实体当前Mesh句柄，不扫描其他实体。

use crate::{
    resources::{MeshRevisionRegistry, RaytracingRuntime, RollbackDisposition}, // 当前会话身份与原子回退入口。
    types::{AdaptationIdentity, RaytracingPrototypeBlocker}, // 后台票据身份与公开失败原因。
};

/// 判断后台身份是否仍精确对应当前场景代次与源实体。
pub(super) fn adaptation_identity_matches(
    runtime: &RaytracingRuntime,      // 当前场景会话和当前候选槽。
    identity: AdaptationIdentity,     // 后台完成值携带的完整启动身份。
    revisions: &MeshRevisionRegistry, // 资产事件维护的当前源修订。
    source_meshes: &Query<&Mesh3d>,   // 按明确源实体读取当前Mesh句柄。
) -> bool {
    adaptation_identity_values_match(
        runtime,                                 // 当前会话权威值。
        identity,                                // 完成值的启动身份。
        revisions.current(identity.source_mesh), // 热更事件推进后的当前修订。
        source_meshes
            .get(identity.source_entity)
            .ok()
            .map(|mesh| mesh.0.id()), // 源实体换句柄或消失都归一为身份不匹配。
    )
}

/// 仅当失败任务仍属于当前完整事务时请求硬失败；迟到失败只回收。
pub(super) fn record_task_failure_if_current(
    runtime: &mut RaytracingRuntime,  // 当前会话及待发布回退事务。
    identity: AdaptationIdentity,     // 失败任务的完整启动身份。
    error: String,                    // 后台转换返回的诊断详情。
    revisions: &MeshRevisionRegistry, // 当前源Mesh修订。
    source_meshes: &Query<&Mesh3d>,   // 当前源实体实际句柄。
) {
    record_task_failure_if_values_current(
        runtime,                                 // 只有身份完全匹配才会被修改。
        identity,                                // 当前或迟到任务的身份。
        error,                                   // 仅当前失败会公开此详情。
        revisions.current(identity.source_mesh), // 当前资产修订。
        source_meshes
            .get(identity.source_entity)
            .ok()
            .map(|mesh| mesh.0.id()), // 当前源实体不存在时视为迟到结果。
    );
}

/// 使用已读取的权威值发布失败，便于锁定“当前失败/迟到失败”边界测试。
pub(super) fn record_task_failure_if_values_current(
    runtime: &mut RaytracingRuntime, // 当前场景会话、候选槽与任务票据权威值。
    identity: AdaptationIdentity,    // 后台失败值携带的完整启动身份。
    error: String,                   // 当前失败时写入F1的详情。
    current_revision: u64,           // MeshRevisionRegistry当前修订。
    current_source_mesh: Option<AssetId<Mesh>>, // 源实体此刻实际绑定的Mesh身份。
) {
    if adaptation_identity_values_match(
        runtime,             // 当前会话权威值。
        identity,            // 后台启动身份。
        current_revision,    // 当前资产修订。
        current_source_mesh, // 当前实体Mesh句柄。
    ) {
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::AsyncTaskFailed { detail: error }), // 保留失败详情。
            RollbackDisposition::Failed, // 当前转换失败不得自动降级为部分光追。
        );
    }
}

/// 对比不依赖ECS借用的身份值，所有字段一致才允许成功采用或发布失败。
pub(super) fn adaptation_identity_values_match(
    runtime: &RaytracingRuntime,  // 当前场景会话、候选槽与任务票据权威值。
    identity: AdaptationIdentity, // 后台完成值携带的完整启动身份。
    current_revision: u64,        // MeshRevisionRegistry当前修订。
    current_source_mesh: Option<AssetId<Mesh>>, // 源实体此刻实际绑定的Mesh身份。
) -> bool {
    let Some(root) = runtime.root else {
        return false; // 已回退并清空根时，任何后台值都属于迟到结果。
    };
    let Some(candidate) = runtime.candidates.get(identity.contract_index) else {
        return false; // 固定槽不存在时不得按其他候选兜底。
    };
    runtime.generation == identity.generation // 场景代次必须一致。
        && root == identity.root // 目标根实体必须一致。
        && candidate.entity == identity.source_entity // 固定primitive实体必须一致。
        && candidate.mesh_id == identity.source_mesh // 源Mesh AssetId必须一致。
        && candidate.source_revision == identity.source_revision // 启动修订必须仍当前。
        && candidate.adaptation_ticket == Some(identity.ticket) // 每槽只接受最新唯一票据。
        && current_revision == identity.source_revision // 资产事件表不得已推进修订。
        && current_source_mesh == Some(identity.source_mesh) // 实体不得换句柄或消失。
}
