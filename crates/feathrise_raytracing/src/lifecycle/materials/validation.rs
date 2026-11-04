//! 动态地形材质源身份、事务完整性与活动期稳定性核对。

use bevy::{
    asset::AssetId,
    pbr::{MeshMaterial3d, StandardMaterial},
    prelude::*,
};
use bevy_solari::prelude::SolariLighting;

use super::{SourceIdentityData, SurfaceSourceData, SurfaceSourceFilter};
use crate::{
    resources::{RaytracingRuntime, RollbackDisposition},
    types::{RaytracingPrototypeBlocker, RaytracingPrototypeStage, SourceMaterialBinding},
};

pub(super) fn validate_candidate_identity(
    candidate: &crate::types::SourceCandidate,
    identities: &Query<SourceIdentityData<'_>>,
) -> Result<(), RaytracingPrototypeBlocker> {
    let Ok((mesh_name, material_name, mesh)) = identities.get(candidate.entity) else {
        return Err(RaytracingPrototypeBlocker::SourceChanged);
    };
    if mesh_name.map_or("", |name| name.0.as_str()) != candidate.mesh_name
        || material_name.map_or("", |name| name.0.as_str()) != candidate.material_name
        || candidate.mesh.id() != candidate.mesh_id
        || mesh.0.id() != candidate.mesh_id
    {
        return Err(RaytracingPrototypeBlocker::SourceChanged);
    }
    Ok(())
}

pub(super) fn prepared_sources_still_match(
    runtime: &RaytracingRuntime,
    identities: &Query<SourceIdentityData<'_>>,
    standard_sources: &Query<&MeshMaterial3d<StandardMaterial>>,
    surface_sources: &Query<SurfaceSourceData<'_>, SurfaceSourceFilter>,
) -> bool {
    if !prepared_transaction_has_expected_count(
        runtime.candidates.len(),
        runtime.prepared_materials.len(),
        runtime.expected_instance_count() as usize,
    ) {
        return false; // `zip`会忽略较长一侧，短事务必须在逐项比较前整体拒绝。
    }
    runtime
        .candidates
        .iter()
        .zip(&runtime.prepared_materials)
        .all(|(candidate, prepared)| {
            validate_candidate_identity(candidate, identities).is_ok()
                && match &prepared.binding {
                    SourceMaterialBinding::Standard { source, deferred } => {
                        let expected = expected_material_id(
                            source.id(),
                            deferred.id(),
                            runtime.visible_materials_activated,
                        );
                        standard_sources
                            .get(prepared.source_entity)
                            .is_ok_and(|current| current.0.id() == expected)
                    }
                    SourceMaterialBinding::TerrainSurface {
                        source, deferred, ..
                    } => {
                        let expected = expected_material_id(
                            source.id(),
                            deferred.id(),
                            runtime.visible_materials_activated,
                        );
                        surface_sources.get(prepared.source_entity).is_ok_and(
                            |(current, material_override)| {
                                let override_valid = if runtime.visible_materials_activated {
                                    material_override.is_some_and(|material_override| {
                                        material_override.matches(source.id(), deferred.id())
                                    })
                                } else {
                                    material_override.is_none()
                                };
                                current.0.id() == expected && override_valid
                            },
                        ) // 激活后句柄与租约必须同时完整，撤销或抢占任一项都整体回退。
                    }
                }
        })
}

/// 当前地图目标事务两侧都完整时才允许使用逐项`zip`校验。
pub(super) fn prepared_transaction_has_expected_count(
    candidate_count: usize,
    prepared_count: usize,
    expected_count: usize,
) -> bool {
    candidate_count == expected_count && prepared_count == expected_count
}

/// 区分可等待的异步材质状态与必须立即结束当前事务的确定性错误。
pub(super) fn handle_gpu_preparation_blocker(
    runtime: &mut RaytracingRuntime,
    blocker: RaytracingPrototypeBlocker,
) {
    match blocker {
        RaytracingPrototypeBlocker::MaterialUnavailable { .. } => {
            runtime.blocker = Some(blocker); // 资产或地表承接可能在后续帧就绪，由三十秒停止线兜底。
        }
        RaytracingPrototypeBlocker::SourceChanged => {
            runtime.request_rollback(Some(blocker), RollbackDisposition::RasterReady)
        }
        _ => runtime.request_rollback(Some(blocker), RollbackDisposition::Failed),
    }
}

pub(super) fn expected_material_id<T: Asset>(
    source: AssetId<T>,
    deferred: AssetId<T>,
    activated: bool,
) -> AssetId<T> {
    if activated { deferred } else { source }
}

pub(super) fn validate_opaque_material(
    material: &StandardMaterial,
    contract_index: usize,
) -> Result<(), RaytracingPrototypeBlocker> {
    if material.alpha_mode != AlphaMode::Opaque {
        return Err(RaytracingPrototypeBlocker::CandidateContractMismatch {
            detail: format!(
                "固定候选{contract_index}的源材质AlphaMode为{:?}，P1只接受Opaque",
                material.alpha_mode
            ),
        });
    }
    Ok(())
}

pub(super) fn validate_source_material_identity_count(
    standard_count: usize,
    surface_count: usize,
    expected_count: usize,
) -> Result<(), RaytracingPrototypeBlocker> {
    let actual_count = standard_count + surface_count;
    if actual_count != expected_count {
        return Err(RaytracingPrototypeBlocker::CandidateContractMismatch {
            detail: format!(
                "当前光追目标应使用{expected_count}个真实源材质AssetId，实际为{actual_count}个（Standard={standard_count}，通用地表={surface_count}）"
            ),
        });
    }
    Ok(())
}

/// 活动期只定向核对当前事务的已知实体；身份或材质变化立即请求完整回退。
pub(crate) fn validate_active_sources(
    identities: Query<SourceIdentityData<'_>>,
    standard_sources: Query<&MeshMaterial3d<StandardMaterial>>,
    surface_sources: Query<SurfaceSourceData<'_>, SurfaceSourceFilter>,
    solari_cameras: Query<(), With<SolariLighting>>,
    mut runtime: ResMut<RaytracingRuntime>,
) {
    if runtime.stage != RaytracingPrototypeStage::Active {
        return;
    }
    let camera_valid = runtime
        .camera
        .is_some_and(|camera| solari_cameras.get(camera).is_ok());
    if !camera_valid
        || !prepared_sources_still_match(&runtime, &identities, &standard_sources, &surface_sources)
        || runtime.render_telemetry.proxy_count != runtime.expected_instance_count()
        || !runtime.render_telemetry.bindings_ready
        || !crate::telemetry::geometry_ready(&runtime)
    {
        runtime.request_rollback(
            Some(if camera_valid {
                RaytracingPrototypeBlocker::SourceChanged
            } else {
                RaytracingPrototypeBlocker::CameraRebuilt
            }),
            RollbackDisposition::RasterReady,
        );
    }
}
