//! 请求、目标换代、资产热更、阶段超时、完整回退与公开状态发布。

use std::mem; // 回退事务一次取走本会话所有权集合，避免清理中留下半套状态。

use bevy::{
    asset::{AssetEvent, AssetId}, // 热更按稳定资产身份失效后台结果和材质副本。
    camera::RenderTarget,         // Solari只能接管窗口玩法视图，纹理目标即使误带Marker也必须拒绝。
    pbr::{MeshMaterial3d, StandardMaterial}, // 精确恢复普通可见源材质句柄。
    prelude::*,                   // ECS资源、消息、相机、Mesh与时间。
    time::Real,                   // F1会暂停虚拟时间，冷启动停止线必须继续按墙钟推进。
};
use bevy_solari::prelude::SolariLighting;
use feathrise_scene_core::view::PrimarySceneView; // 只允许项目唯一玩法主相机取得Solari租约。
use feathrise_environment::terrain_surface::{TerrainSurfaceMaterial, TerrainSurfaceMaterialOverride}; // 通用地表热更、Deferred租约与精确材质恢复。
use feathrise_scene::{
    DynamicSceneRuntimeStatus, DynamicSceneTerrainPrimitive,
}; // 生命周期只读取两类目标的公共身份；动态植被、水和放置物完全不进入候选查询。
use feathrise_settings::GraphicsRuntimeLeaseRegistry; // 释放本crate持有的明确运行时租约。
use feathrise_scene_core::state::ScenePhase; // 只接受目录驱动的地形目标。 // 相机回退时移除本crate插入的Solari入口。

use crate::{
    components::SolariHistoryState,
    constants::STAGE_TIMEOUT_SECONDS,
    resources::{
        MeshRevisionRegistry, RaytracingCapability, RaytracingPrototypeRequest,
        RaytracingPrototypeStatus, RaytracingRuntime, RaytracingSceneTracker, RollbackDisposition,
    },
    types::{
        RaytracingBudgetSnapshot, RaytracingGpuTimings, RaytracingPrototypeBlocker,
        RaytracingPrototypeStage, RaytracingPrototypeTarget, SourceMaterialBinding,
    },
};

use super::target::{unique_window_gameplay_camera, validated_target_root}; // 场景与相机目标共用不可混用入口。

/// 把F1请求、实际设备能力、获批场景目标和玩法相机收敛为唯一状态机入口。
#[allow(clippy::type_complexity)] // 唯一玩法窗口相机必须同时满足 Marker、Camera3d 与窗口目标三项资格。
pub(crate) fn coordinate_lifecycle(
    request: Res<RaytracingPrototypeRequest>, // F1只写这一份非持久请求。
    capability: Res<RaytracingCapability>,    // finish阶段从真实RenderDevice冻结的结论。
    tracker: Res<RaytracingSceneTracker>,     // 由动态地形观察器推进。
    current_scene: Res<State<ScenePhase>>,     // 阻断离场后残留实体或状态取得Solari所有权。
    dynamic_terrains: Query<
        &Mesh3d,
        Or<(
            With<DynamicSceneTerrainPrimitive>,
            With<feathrise_scene::ClipmapRaytracingAnchor>,
        )>,
    >, // 只读真实源，不读移动环带。
    dynamic_status: Res<DynamicSceneRuntimeStatus>, // 动态场景必须保持完整Ready修订。
    cameras: Query<(Entity, &RenderTarget), (With<PrimarySceneView>, With<Camera3d>)>, // Marker身份之外再以目标类型排除纹理相机。
    time: Res<Time<Real>>, // 暂停玩法或打开F1也继续执行三十秒停止线。
    mut runtime: ResMut<RaytracingRuntime>, // 状态机与全部可撤销所有权。
    clipmap_status: Option<Res<feathrise_scene::ClipmapRaytracingStatus>>, // 无真实源时显示后台阶段/失败，避免误称未支持场景。
) {
    let request_rose = request.enabled && !runtime.last_request_enabled;
    runtime.last_request_enabled = request.enabled;
    if request_rose {
        runtime.failed_generation = None; // 关闭后重新开启显式解除当前代次失败锁。
        if runtime.stage == RaytracingPrototypeStage::Failed {
            runtime.blocker = None;
            runtime.transition(RaytracingPrototypeStage::RasterReady);
        }
    }

    if !capability.available {
        if runtime_has_session(&runtime) {
            runtime.request_rollback(capability.blocker.clone(), RollbackDisposition::RasterReady);
        } else {
            runtime.blocker = capability.blocker.clone();
            runtime.target = RaytracingPrototypeTarget::None;
            runtime.transition(RaytracingPrototypeStage::Unavailable);
        }
        return; // 能力不足时绝不创建候选、代理、租约或Solari相机组件。
    }

    if !request.enabled {
        runtime.failed_generation = None; // 关闭请求同时为下一次显式重开清除失败锁。
        if runtime_has_session(&runtime) {
            runtime.request_rollback(None, RollbackDisposition::RasterReady);
        } else if runtime.stage != RaytracingPrototypeStage::RollingBack {
            runtime.blocker = None;
            runtime.target = RaytracingPrototypeTarget::None;
            runtime.transition(RaytracingPrototypeStage::RasterReady);
        }
        return;
    }

    let target_root = validated_target_root(
        &tracker,
        *current_scene.get(),
        &dynamic_terrains,
        &dynamic_status,
    );
    let Some(target_root) = target_root else {
        if runtime_has_session(&runtime) {
            runtime.request_rollback(
                Some(RaytracingPrototypeBlocker::WaitingForSupportedScene),
                RollbackDisposition::RasterReady,
            );
        } else {
            runtime.target = RaytracingPrototypeTarget::None;
            runtime.blocker = Some(
                if *current_scene.get() == ScenePhase::Active
                    && let Some(status) = clipmap_status.as_ref()
                    && status.phase != feathrise_scene::ClipmapRaytracingPhase::Disabled
                {
                    RaytracingPrototypeBlocker::MeshUnavailable {
                        detail: status.detail.clone().unwrap_or_else(|| {
                            format!("Clipmap 地形光追代理：{}", status.phase.label())
                        }),
                    }
                } else {
                    RaytracingPrototypeBlocker::WaitingForSupportedScene
                },
            );
            runtime.transition(RaytracingPrototypeStage::RasterReady);
        }
        return;
    };

    let Some(target_camera) = unique_window_gameplay_camera(cameras.iter()) else {
        if runtime_has_session(&runtime) {
            runtime.request_rollback(
                Some(RaytracingPrototypeBlocker::CameraRebuilt),
                RollbackDisposition::RasterReady,
            );
        } else {
            runtime.blocker = Some(RaytracingPrototypeBlocker::WaitingForGameplayCamera);
            runtime.transition(RaytracingPrototypeStage::RasterReady);
        }
        return;
    };

    if runtime.stage == RaytracingPrototypeStage::RollingBack {
        return; // 同帧后续统一回退系统会完整撤销旧会话，下一帧再接受新目标。
    }
    if runtime.root.is_some()
        && (runtime.root != Some(target_root)
            || runtime.generation != tracker.generation
            || runtime.target != tracker.target)
    {
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::SourceChanged),
            RollbackDisposition::RasterReady,
        );
        return; // 热重载或根换代不能把旧任务、代理或材质带入新代。
    }
    if runtime.camera.is_some() && runtime.camera != Some(target_camera) {
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::CameraRebuilt),
            RollbackDisposition::RasterReady,
        );
        return; // 旧相机附件必须先完整恢复，下一帧才能租赁新相机。
    }

    if runtime.stage == RaytracingPrototypeStage::Failed {
        if runtime.failed_generation == Some(tracker.generation) {
            return; // 同一场景代次硬失败后保持完整光栅，禁止逐帧自动重试。
        }
        runtime.failed_generation = None;
        runtime.blocker = None;
        runtime.transition(RaytracingPrototypeStage::RasterReady); // 新ready代次允许重新执行完整契约。
    }

    if matches!(
        runtime.stage,
        RaytracingPrototypeStage::Unavailable | RaytracingPrototypeStage::RasterReady
    ) {
        runtime.generation = tracker.generation;
        runtime.target = tracker.target; // 后续数量、预算、材质和F1全部读取同一场景级契约。
        runtime.root = Some(target_root);
        runtime.camera = Some(target_camera);
        runtime.blocker = None;
        runtime.transition(RaytracingPrototypeStage::DiscoveringCandidates);
    }

    if stage_has_timeout(runtime.stage) {
        runtime.stage_elapsed_seconds += time.delta_secs();
        if stage_timeout_exceeded(runtime.stage_elapsed_seconds) {
            let stage = runtime.stage;
            runtime.request_rollback(
                Some(RaytracingPrototypeBlocker::StageTimedOut { stage }),
                RollbackDisposition::Failed,
            );
        }
    }
}

fn runtime_has_session(runtime: &RaytracingRuntime) -> bool {
    runtime.root.is_some()
        || runtime.camera.is_some()
        || !runtime.candidates.is_empty()
        || runtime.capture_job.is_some()
        || !runtime.prepared_materials.is_empty()
        || !runtime.proxy_entities.is_empty()
        || runtime.lease.is_some()
        || runtime.solari_inserted
        || matches!(
            runtime.stage,
            RaytracingPrototypeStage::DiscoveringCandidates
                | RaytracingPrototypeStage::AdaptingMeshes
                | RaytracingPrototypeStage::AwaitingGpuAssets
                | RaytracingPrototypeStage::WarmingSolari
                | RaytracingPrototypeStage::Active
        )
}

fn stage_has_timeout(stage: RaytracingPrototypeStage) -> bool {
    matches!(
        stage,
        RaytracingPrototypeStage::DiscoveringCandidates
            | RaytracingPrototypeStage::AdaptingMeshes
            | RaytracingPrototypeStage::AwaitingGpuAssets
            | RaytracingPrototypeStage::WarmingSolari
    )
}

/// 精确锁定“超过30秒”边界；恰好30秒仍给本阶段本帧完成机会。
fn stage_timeout_exceeded(elapsed_seconds: f32) -> bool {
    elapsed_seconds > STAGE_TIMEOUT_SECONDS // 30.0秒不超时，下一次真实时间推进后才失败。
}

/// 完整消费源资产消息；只有当前全部候选的稳定身份发生变化才失效当前会话。
#[allow(clippy::too_many_arguments)]
pub(crate) fn monitor_source_asset_changes(
    mut mesh_events: feathrise_scene_core::residency::ResidentAssetEventReader<Mesh>, // Mesh热更同时推进源修订以拒收迟到后台结果。
    mut standard_material_events: feathrise_scene_core::residency::ResidentAssetEventReader<StandardMaterial>, // 普通源材质热更使延迟克隆过期。
    mut surface_material_events: feathrise_scene_core::residency::ResidentAssetEventReader<
        TerrainSurfaceMaterial,
    >, // 通用地表扩展热更同样整体回退。
    standard_sources: Query<&MeshMaterial3d<StandardMaterial>>, // 准备前按当前实体句柄识别源资产。
    surface_sources: Query<&MeshMaterial3d<TerrainSurfaceMaterial>>, // 地表承接后按扩展句柄识别。
    mut revisions: ResMut<MeshRevisionRegistry>, // 只记录全部候选实际源Mesh，不积累全世界资产表。
    mut runtime: ResMut<RaytracingRuntime>,      // 聚合整帧变化后只提交一次回退请求。
) {
    let mut source_changed = false;
    for event in mesh_events.read() {
        let id = mesh_event_id(event);
        if runtime
            .candidates
            .iter()
            .any(|candidate| candidate.mesh_id == id)
        {
            revisions.bump(id); // Modified复用AssetId时仍让正在执行的任务身份失效。
            source_changed = true;
        }
    }
    for event in standard_material_events.read() {
        if standard_material_is_source(
            standard_material_event_id(event),
            &runtime,
            &standard_sources,
        ) {
            source_changed = true;
        }
    }
    for event in surface_material_events.read() {
        if surface_material_is_source(surface_material_event_id(event), &runtime, &surface_sources)
        {
            source_changed = true;
        }
    }

    if source_changed && runtime.pending_rollback.is_none() {
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::SourceChanged),
            RollbackDisposition::RasterReady,
        ); // 不尝试局部重建BLAS或材质；新ready代次必须重新通过全部候选整体契约。
    }
}

fn mesh_event_id(event: &AssetEvent<Mesh>) -> AssetId<Mesh> {
    match *event {
        AssetEvent::Added { id }
        | AssetEvent::Modified { id }
        | AssetEvent::Removed { id }
        | AssetEvent::Unused { id }
        | AssetEvent::LoadedWithDependencies { id } => id,
    }
}

fn standard_material_event_id(event: &AssetEvent<StandardMaterial>) -> AssetId<StandardMaterial> {
    match *event {
        AssetEvent::Added { id }
        | AssetEvent::Modified { id }
        | AssetEvent::Removed { id }
        | AssetEvent::Unused { id }
        | AssetEvent::LoadedWithDependencies { id } => id,
    }
}

fn surface_material_event_id(
    event: &AssetEvent<TerrainSurfaceMaterial>,
) -> AssetId<TerrainSurfaceMaterial> {
    match *event {
        AssetEvent::Added { id }
        | AssetEvent::Modified { id }
        | AssetEvent::Removed { id }
        | AssetEvent::Unused { id }
        | AssetEvent::LoadedWithDependencies { id } => id,
    }
}

fn standard_material_is_source(
    id: AssetId<StandardMaterial>,
    runtime: &RaytracingRuntime,
    current: &Query<&MeshMaterial3d<StandardMaterial>>,
) -> bool {
    if let Some(matches_source) = prepared_standard_source_match(id, runtime) {
        return matches_source; // 原子准备完成后只认事务保存的作者源ID，绝不能把实体当前挂载的会话Deferred副本误判为热重载源。
    }
    runtime.candidates.iter().any(|candidate| {
        current
            .get(candidate.entity)
            .is_ok_and(|handle| handle.id() == id)
    }) // 材质事务建立前，候选实体当前句柄仍是唯一可用的权威源身份。
}

fn surface_material_is_source(
    id: AssetId<TerrainSurfaceMaterial>,
    runtime: &RaytracingRuntime,
    current: &Query<&MeshMaterial3d<TerrainSurfaceMaterial>>,
) -> bool {
    if let Some(matches_source) = prepared_surface_source_match(id, runtime) {
        return matches_source; // 通用地表切到Deferred扩展副本后同样只监听原始作者资产，阻断自发Modified回退循环。
    }
    runtime.candidates.iter().any(|candidate| {
        current
            .get(candidate.entity)
            .is_ok_and(|handle| handle.id() == id)
    }) // TerrainSurfaceCarrier准备阶段尚无事务时继续按候选实体当前句柄识别真实源。
}

/// 材质事务存在时返回普通作者源匹配结论；`None`表示仍需使用准备前的实体句柄回退路径。
fn prepared_standard_source_match(
    id: AssetId<StandardMaterial>, // 待判定的普通材质资产事件身份。
    runtime: &RaytracingRuntime,   // 原子保存全部候选作者源与会话副本的事务。
) -> Option<bool> {
    if runtime.prepared_materials.is_empty() {
        return None; // 后台适配阶段尚未冻结材质事务，只能由调用方读取候选实体当前作者句柄。
    }
    Some(runtime.prepared_materials.iter().any(|prepared| {
        matches!(
            &prepared.binding,
            SourceMaterialBinding::Standard { source, .. } if source.id() == id
        ) // Deferred会话副本和通用地表候选均不会命中作者源字段。
    }))
}

/// 材质事务存在时返回通用地表作者源匹配结论；会话Deferred扩展副本必须明确返回`false`。
fn prepared_surface_source_match(
    id: AssetId<TerrainSurfaceMaterial>, // 待判定的地表扩展材质事件身份。
    runtime: &RaytracingRuntime,         // 保存通用地表原始与Deferred类型化句柄。
) -> Option<bool> {
    if runtime.prepared_materials.is_empty() {
        return None; // 地表正式承接完成前仍由调用方按候选实体当前句柄识别源。
    }
    Some(runtime.prepared_materials.iter().any(|prepared| {
        matches!(
            &prepared.binding,
            SourceMaterialBinding::TerrainSurface { source, .. } if source.id() == id
        ) // 只有原始扩展材质热更才允许使全部候选事务整体失效。
    }))
}

/// 执行单一原子回退事务，精确撤销可见材质、相机租约、代理与派生资产。
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_pending_rollback(
    mut commands: Commands, // 恢复源实体并回收本crate拥有的代理和相机组件。
    mut meshes: ResMut<Assets<Mesh>>, // 显式移除全部候选派生Mesh资产。
    mut standard_materials: ResMut<Assets<StandardMaterial>>, // 移除去重延迟副本和地形代理材质。
    mut surface_materials: ResMut<Assets<TerrainSurfaceMaterial>>, // 移除通用地表延迟扩展副本。
    surface_overrides: Query<(
        Option<&MeshMaterial3d<TerrainSurfaceMaterial>>,
        &TerrainSurfaceMaterialOverride,
    )>, // 只恢复仍由本事务精确持有的地表替代，不能覆盖场景已采用的新修订。
    mut leases: ResMut<GraphicsRuntimeLeaseRegistry>, // 释放后由设置crate精确恢复AA/HDR/预通道等作者状态。
    mut runtime: ResMut<RaytracingRuntime>,           // 一次取走并清空当前会话全部所有权。
) {
    let Some(pending) = runtime.pending_rollback.take() else {
        return;
    };

    for task in &mut runtime.tasks {
        task.cancelled = true; // 任务池工作不阻塞等待，完成值由轮询系统按迟到结果直接丢弃。
    }
    runtime.capture_job = None;
    runtime.completed.clear();

    let prepared_materials = mem::take(&mut runtime.prepared_materials);
    if runtime.visible_materials_activated {
        for prepared in &prepared_materials {
            let Ok(mut entity) = commands.get_entity(prepared.source_entity) else {
                continue; // 切换场景或GLB热重建已销毁的旧源不需要也不能恢复。
            };
            match &prepared.binding {
                SourceMaterialBinding::Standard { source, .. } => {
                    entity.insert(MeshMaterial3d(source.clone())); // 精确恢复激活前普通源句柄。
                }
                SourceMaterialBinding::TerrainSurface {
                    source, deferred, ..
                } => {
                    let Ok((current, material_override)) =
                        surface_overrides.get(prepared.source_entity)
                    else {
                        continue; // 场景已撤销租约并采用新修订时，回退不得把旧作者句柄写回去。
                    };
                    if !material_override.matches(source.id(), deferred.id()) {
                        continue; // 其他代次的精确租约不属于本事务。
                    }
                    entity.remove::<TerrainSurfaceMaterialOverride>(); // 无论当前句柄是否仍在，先释放本事务租约。
                    if current.is_some_and(|current| current.0.id() == deferred.id()) {
                        entity.insert(MeshMaterial3d(source.clone())); // 仅本Deferred仍实际安装时恢复原作者句柄。
                    }
                }
            }
        }
    }

    if let Some(camera) = runtime.camera
        && let Ok(mut camera_commands) = commands.get_entity(camera)
    {
        camera_commands.remove::<(SolariLighting, SolariHistoryState)>(); // required附件由租约作者快照统一恢复。
    }
    if let Some(lease) = runtime.lease.take() {
        leases.release(lease); // 重复或外部提前释放安全返回false，不影响其他调用方租约。
    }

    for proxy in runtime.proxy_entities.drain(..) {
        commands.entity(proxy).try_despawn(); // 与源层级销毁幂等，不扫描全世界代理。
    }
    for candidate in &mut runtime.candidates {
        if let Some(derived) = candidate.derived_mesh.take() {
            meshes.remove(derived.id()); // 原始Mesh强句柄和资产内容从未被修改。
        }
    }
    for material in runtime.created_standard_materials.drain(..) {
        standard_materials.remove(material.id());
    }
    for material in runtime.created_surface_materials.drain(..) {
        surface_materials.remove(material.id());
    }

    runtime.root = None;
    runtime.camera = None;
    runtime.candidates.clear();
    runtime.capture_cursor = 0;
    runtime.visible_materials_activated = false;
    runtime.gpu_assets_prepared = false;
    runtime.lease_settle_pending = false;
    runtime.solari_inserted = false;
    runtime.warmup_frames = 0;
    runtime.last_warmup_render_frame = 0;
    runtime.last_warmup_gpu_diagnostic_frame = 0;
    runtime.gpu_timings = RaytracingGpuTimings::default();
    runtime.world_cache_active_cells = None; // 回退后旧world-cache会销毁，F1不得继续显示上一会话cell数。
    runtime.gpu_diagnostics_fresh = false;
    runtime.blocker = pending.blocker;
    let outstanding_task_bytes = runtime.estimated_cpu_working_set(); // 取消不等于抢占，过期任务结束前继续公开真实占用。

    match pending.disposition {
        RollbackDisposition::RasterReady => {
            runtime.failed_generation = None;
            runtime.target = RaytracingPrototypeTarget::None; // 普通换代先完整回到无目标，再由下一帧原子采用新契约。
            runtime.budget = RaytracingBudgetSnapshot::default();
            runtime.budget.cpu_working_set_bytes = outstanding_task_bytes; // 资产统计清零但后台占用不能伪装为零。
            runtime.transition(RaytracingPrototypeStage::RasterReady);
        }
        RollbackDisposition::Failed => {
            runtime.budget.cpu_working_set_bytes = outstanding_task_bytes; // 保留失败统计同时刷新尚未回收的任务占用。
            runtime.failed_generation = Some(runtime.generation); // 保留失败代次锁与最后预算供F1审计。
            runtime.transition(RaytracingPrototypeStage::Failed);
        }
    }
}

/// 把私有状态机与RenderApp上一完整帧证据发布到只读F1状态资源。
pub(crate) fn publish_public_status(
    request: Res<RaytracingPrototypeRequest>,
    runtime: Res<RaytracingRuntime>,
    mut status: ResMut<RaytracingPrototypeStatus>,
) {
    status.publish_runtime(
        request.enabled,
        runtime.stage,
        runtime.target,
        runtime.blocker.clone(),
        runtime.budget,
        runtime.tasks.len() as u32,
        runtime.render_telemetry.bindings_ready,
        runtime.render_telemetry.proxy_count,
        runtime.render_telemetry.deferred_bridge_target_count,
        runtime.render_telemetry.deferred_bridge_ready_count,
        runtime.render_telemetry.deferred_bridge_ready_frames,
        runtime.history_last_reset_reason,
        runtime.history_reset_count,
        runtime.history_stable_frames,
        runtime.gpu_timings,
        runtime.world_cache_active_cells,
    );
    status.publish_denoising(
        cfg!(feature = "dlss_rr"), // F1明确区分普通test_panel与需要外部SDK的真实降噪构建。
        request.dlss_rr_mode,      // 保留关闭/DLAA/Quality请求，不从设备有效态反推。
        runtime.dlss_rr_supported, // 设备能力不影响基础Solari有效态。
        runtime.dlss_rr_active,    // 只表示目标相机组件已经挂接。
        runtime.dlss_rr_unavailable_reason, // 活动态为None，其余显示构建、设备或相机等待原因。
        runtime.dlss_rr_cpu_ms,    // 最近新鲜的同步NGX Evaluate与命令录制CPU时间。
        runtime.dlss_rr_gpu_ms,    // 未活动、过期或回退后必须为None。
        runtime.render_telemetry.dlss_output_resolution, // RenderApp真实最终输出尺寸。
        runtime.render_telemetry.dlss_render_resolution, // NGX实际内部主通道尺寸。
        runtime.render_telemetry.dlss_context_generation, // 稳定帧必须保持不变的上下文代次。
        runtime.render_telemetry.dlss_resolution_change_count, // 窗口缩放或切档的实际次数。
    ); // 厂商降噪状态与核心全部候选状态机保持只读诊断关系。
}

#[cfg(test)]
mod tests; // 生命周期边界与完整回退夹具独立放入同域子模块，保持运行时文件低于500行。
