//! 动态地形材质契约、Deferred 副本与原子事务回归测试。

use super::*; // 锁定普通材质延迟副本不修改源资产。
use crate::constants::DYNAMIC_EXPECTED_INSTANCE_COUNT;

#[test]
fn deferred_clone_changes_only_the_clone_renderer_method() {
    let source = StandardMaterial::default(); // 构造仍由全局Forward默认解释的源资产。
    let mut clone = source.clone(); // 模拟本原型按AssetId去重产生的会话副本。
    clone.opaque_render_method = OpaqueRendererMethod::Deferred; // 只修改副本渲染路径。
    assert_ne!(clone.opaque_render_method, source.opaque_render_method); // 源资产不得被就地改写。
    assert_eq!(source.opaque_render_method, OpaqueRendererMethod::Auto); // 原句柄保持作者值以便精确回退。
}

#[test] // 动态场景三角元数据不得改变非高度场材质路径。
fn shadow_terminator_metadata_is_scoped_to_dynamic_terrain_target() {
    // 无目标时继续使用普通GBuffer，只有动态高度场发布三角身份。
    assert!(!dynamic_terrain_publishes_shadow_terminator_data(
        RaytracingPrototypeTarget::None,
        0,
    )); // 未取得场景目标时不得占用自定义Deferred通道。
    assert!(dynamic_terrain_publishes_shadow_terminator_data(
        RaytracingPrototypeTarget::DynamicSceneTerrain,
        0,
    )); // 动态低模高度场是本轮唯一受影响目标。
    assert!(!dynamic_terrain_publishes_shadow_terminator_data(
        RaytracingPrototypeTarget::DynamicSceneTerrain,
        1,
    )); // 有纹理体积网格也使用扩展材质，但其三角编号不属于高度代理。
} // 完成目标范围回归测试。

#[test]
fn source_validation_switches_expected_handle_only_after_atomic_activation() {
    let mut materials = Assets::<StandardMaterial>::default(); // 提供稳定类型化AssetId。
    let source = materials.add(StandardMaterial::default()); // 激活前作者源句柄。
    let deferred = materials.add(StandardMaterial::default()); // 激活后的会话副本句柄。
    assert_eq!(
        expected_material_id(source.id(), deferred.id(), false), // 预热状态尚未提交事务。
        source.id(),                                             // 应继续接受作者源句柄。
    ); // 两帧预热期间可见源必须继续保持原始Forward句柄。
    assert_eq!(
        expected_material_id(source.id(), deferred.id(), true), // 原子事务已经提交。
        deferred.id(),                                          // 应只接受Deferred副本句柄。
    ); // 激活事务提交后活动校验必须接受Deferred副本，不能立即误回退。
}

#[test]
fn terrain_material_contract_requires_opaque_assets_and_one_source_id() {
    assert!(validate_opaque_material(&StandardMaterial::default(), 0).is_ok()); // Bevy默认材质满足P1不透明契约。
    let transparent = StandardMaterial {
        alpha_mode: AlphaMode::Blend, // 模拟GLB材质意外改为透明混合。
        ..default()                   // 其余字段不影响本契约。
    };
    assert!(validate_opaque_material(&transparent, 0).is_err()); // 非Opaque必须在创建代理前整体阻断。
    assert!(validate_source_material_identity_count(0, 1, 1).is_ok()); // 动态RGBA地形只允许唯一扩展材质。
    assert!(validate_source_material_identity_count(1, 1, 1).is_err()); // 意外混入普通源材质时拒绝接管。
    assert!(validate_source_material_identity_count(0, 0, 1).is_err()); // 尚无源材质时不得把空集合当成完整事务。
}

#[test]
fn prepared_transaction_rejects_either_short_side_before_zip_validation() {
    let expected = DYNAMIC_EXPECTED_INSTANCE_COUNT as usize; // 动态地形同样必须完整采用。
    assert!(prepared_transaction_has_expected_count(
        expected, expected, expected
    )); // 候选和材质都完整才允许逐项验证。
    assert!(!prepared_transaction_has_expected_count(
        expected - 1, // 模拟缺少一个候选。
        expected,     // 材质一侧仍完整。
        expected,     // 当前动态目标完整数量。
    )); // 缺失候选不得被zip误判完整。
    assert!(!prepared_transaction_has_expected_count(
        expected,     // 候选一侧仍完整。
        expected - 1, // 模拟缺少一份材质。
        expected,     // 当前动态目标完整数量。
    )); // 缺失材质不得被zip误判完整。
}

#[test]
fn gpu_preparation_waits_only_for_async_materials_and_fails_contract_errors_immediately() {
    let mut waiting = RaytracingRuntime::default(); // 构造尚无回退请求的等待事务。
    handle_gpu_preparation_blocker(
        &mut waiting, // 资源未来可能就绪，当前保持等待阶段。
        RaytracingPrototypeBlocker::MaterialUnavailable {
            detail: "等待地表承接".to_string(), // 模拟异步扩展材质尚未安装。
        },
    );
    assert!(waiting.pending_rollback.is_none()); // 可等待资源不应提前失败。
    assert!(matches!(
        waiting.blocker,
        Some(RaytracingPrototypeBlocker::MaterialUnavailable { .. })
    )); // F1保留当前等待原因。

    let mut failed = RaytracingRuntime::default(); // 构造独立确定性契约事务。
    handle_gpu_preparation_blocker(
        &mut failed, // 契约错误必须在同帧请求完整失败。
        RaytracingPrototypeBlocker::CandidateContractMismatch {
            detail: "源材质不是Opaque".to_string(), // 模拟不可通过等待自行修复的Alpha错误。
        },
    );
    assert_eq!(
        failed
            .pending_rollback
            .as_ref()
            .map(|pending| pending.disposition),
        Some(RollbackDisposition::Failed)
    ); // 不得把确定性错误拖到30秒超时后才回退。

    let mut changed = RaytracingRuntime::default(); // 构造源身份换代事务。
    handle_gpu_preparation_blocker(
        &mut changed,                              // 热更应立即撤销旧会话。
        RaytracingPrototypeBlocker::SourceChanged, // 新代次仍允许重新发现。
    );
    assert_eq!(
        changed
            .pending_rollback
            .as_ref()
            .map(|pending| pending.disposition),
        Some(RollbackDisposition::RasterReady)
    ); // 源换代不是同代硬失败。
}
