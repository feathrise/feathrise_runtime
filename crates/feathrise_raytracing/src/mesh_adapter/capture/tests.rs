//! 分帧源 Mesh 捕获的预算与完整性回归测试。

use bevy::asset::RenderAssetUsages; // 测试Mesh同时保留主世界与渲染世界用法。

use super::super::identity::{
    adaptation_identity_values_match, record_task_failure_if_values_current,
}; // 直接验证后台成功与失败共享的完整身份门。
use super::*; // 直接锁定父模块私有捕获状态机与2MiB硬门。
use crate::types::SourceCandidate; // 构造当前候选槽以验证完整迟到结果身份。

fn test_identity() -> AdaptationIdentity {
    AdaptationIdentity {
        generation: 1,                      // 固定测试场景代次。
        root: Entity::PLACEHOLDER,          // 测试不访问真实ECS根。
        source_entity: Entity::PLACEHOLDER, // 测试不访问真实源实体。
        source_mesh: Default::default(),    // 默认AssetId足以验证纯捕获逻辑。
        source_revision: 1,                 // 固定源修订。
        ticket: 1,                          // 固定任务票据。
        contract_index: 0,                  // 使用首个固定契约槽。
    }
}

fn indexed_mesh(vertex_count: usize) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all()); // 构造最小合法三角列表。
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,            // 捕获第一阶段读取的位置属性。
        vec![[0.0, 0.0, 0.0]; vertex_count], // 按参数生成可跨2MiB边界的数据量。
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_NORMAL,              // 捕获第二阶段读取的法线属性。
        vec![[0.0, 1.0, 0.0]; vertex_count], // 使用有限单位法线。
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,           // 地形契约要求存在原UV0。
        vec![[0.0, 0.0]; vertex_count], // 数量严格匹配顶点。
    );
    mesh.insert_indices(Indices::U32(vec![0, 1, 2])); // 提供一个合法显式三角形。
    mesh
}

/// 构造一项仍属于当前场景代次、源修订和任务票据的固定候选。
fn runtime_with_current_candidate() -> (RaytracingRuntime, AdaptationIdentity, AssetId<Mesh>) {
    let root = Entity::from_raw_u32(1).expect("测试根实体索引合法"); // 固定当前目标根。
    let source_entity = Entity::from_raw_u32(2).expect("测试源实体索引合法"); // 固定源primitive。
    let source_mesh = Handle::<Mesh>::default(); // 默认强句柄足以验证纯身份逻辑。
    let source_mesh_id = source_mesh.id(); // 冻结源Mesh AssetId。
    let identity = AdaptationIdentity {
        generation: 7,               // 当前场景代次。
        root,                        // 当前目标根。
        source_entity,               // 当前源primitive。
        source_mesh: source_mesh_id, // 当前源Mesh AssetId。
        source_revision: 3,          // 当前源修订。
        ticket: 11,                  // 当前槽唯一任务票据。
        contract_index: 0,           // 首个固定契约槽。
    };
    let mut runtime = RaytracingRuntime {
        generation: identity.generation, // 对齐当前场景代次。
        root: Some(root),                // 发布当前目标根。
        ..Default::default()             // 从无会话状态开始构造测试事务。
    };
    runtime.candidates.push(SourceCandidate {
        contract_index: 0,                    // 与完成身份指向同一固定槽。
        mesh_name: "测试网格".to_owned(),     // 身份纯值测试不访问GLTF组件。
        material_name: "测试材质".to_owned(), // 补齐发现阶段冻结的材质身份。
        requires_source_uv: true,             // 本组测试网格明确提供UV并覆盖原格式契约。
        terrain_surface: true,                // 不参与捕获结果身份判断，仅保持真实地形候选语义。
        entity: source_entity,                // 当前源实体。
        mesh: source_mesh,                    // 保持源Mesh强句柄。
        mesh_id: source_mesh_id,              // 当前源Mesh身份。
        source_revision: 3,                   // 当前源修订。
        vertex_count: 3,                      // 最小合法三角形顶点数。
        triangle_count: 1,                    // 最小合法三角形数量。
        derived_bytes: 156,                   // 三顶点四属性共144字节，加三个U32索引12字节。
        adaptation_ticket: Some(11),          // 只有这一票据允许采用。
        derived_mesh: None,                   // 尚未采用后台结果。
    });
    (runtime, identity, source_mesh_id) // 返回完整当前事务供成功与失败路径复用。
}

#[test]
fn capture_never_copies_more_than_supplied_frame_budget() {
    let mesh = indexed_mesh(200_000); // Position单属性已超过2MiB。
    let mut job =
        MeshCaptureJob::new(test_identity(), &mesh, 200_000, 3, true).expect("夹具格式合法");
    let copied = job
        .capture(&mesh, MAX_CAPTURE_BYTES_PER_FRAME)
        .expect("分片捕获应成功");
    assert!(copied <= MAX_CAPTURE_BYTES_PER_FRAME);
    assert!(!job.is_complete()); // 大属性不得在一帧绕过限制完成全部复制。
}

#[test]
fn incomplete_capture_cannot_become_background_input() {
    let mesh = indexed_mesh(3);
    let mut job = MeshCaptureJob::new(test_identity(), &mesh, 3, 3, true).expect("夹具格式合法");
    job.capture(&mesh, 12).expect("只复制一个位置");
    assert!(job.into_input().is_err()); // 部分捕获绝不能启动任务。
}

#[test]
fn late_result_requires_current_generation_revision_entity_and_ticket() {
    let (runtime, identity, source_mesh_id) = runtime_with_current_candidate(); // 使用完整当前事务。

    assert!(adaptation_identity_values_match(
        &runtime,
        identity,
        3,
        Some(source_mesh_id)
    )); // 全部身份仍当前时允许采用。
    assert!(!adaptation_identity_values_match(
        &runtime,
        AdaptationIdentity {
            ticket: 10,
            ..identity
        },
        3,
        Some(source_mesh_id)
    )); // 同槽旧票据必须作为迟到结果丢弃。
    assert!(!adaptation_identity_values_match(
        &runtime,
        identity,
        4,
        Some(source_mesh_id)
    )); // 同AssetId热更后的旧修订不得采用。
    assert!(!adaptation_identity_values_match(
        &runtime, identity, 3, None
    )); // 源实体销毁或换代后不得采用。
}

#[test]
fn current_task_failure_requests_failure_but_late_failure_is_ignored() {
    let (mut current_runtime, identity, source_mesh_id) = runtime_with_current_candidate(); // 当前票据仍有效。
    record_task_failure_if_values_current(
        &mut current_runtime,       // 当前事务应被推进到硬失败回退。
        identity,                   // 完全匹配的任务身份。
        "当前任务失败".to_string(), // 应公开到F1的失败详情。
        3,                          // 当前源修订仍未变化。
        Some(source_mesh_id),       // 当前实体仍绑定同一Mesh。
    );
    assert_eq!(
        current_runtime
            .pending_rollback
            .as_ref()
            .map(|pending| pending.disposition),
        Some(RollbackDisposition::Failed)
    ); // 当前任务失败必须整体失败，不能保留部分光追。

    let (mut late_runtime, identity, source_mesh_id) = runtime_with_current_candidate(); // 另起无回退事务。
    record_task_failure_if_values_current(
        &mut late_runtime, // 迟到失败不得修改新会话。
        AdaptationIdentity {
            ticket: identity.ticket - 1, // 模拟同一固定槽的旧任务票据。
            ..identity
        },
        "迟到任务失败".to_string(), // 此详情不得发布到新会话。
        3,                          // 其他身份值保持一致以隔离票据条件。
        Some(source_mesh_id),       // 源实体仍绑定相同Mesh。
    );
    assert!(late_runtime.pending_rollback.is_none()); // 旧票据失败只回收，不污染当前状态。
}
