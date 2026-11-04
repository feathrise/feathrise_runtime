//! RenderApp 绑定证据、Solari GPU 分段计时和关闭态宿主开销记录。
mod terrain_geometry;
pub(crate) use terrain_geometry::{
    TerrainGeometryTargets, geometry_ready, publish_geometry_targets,
};

use bevy::{
    diagnostic::{DiagnosticPath, DiagnosticsStore}, // 读取RenderDiagnosticsPlugin同步回主世界的GPU毫秒。
    material::OpaqueRendererMethod,                 // 核对每个类型擦除目标实际准备出的渲染方式。
    pbr::PreparedMaterial,                          // ErasedRenderAssets中的统一PBR准备结果。
    prelude::*,                                     // App、Resource、Query与Assets。
    render::{
        Render,
        RenderApp,
        RenderSystems, // 在完整渲染帧Cleanup阶段发布绑定快照。
        erased_render_asset::ErasedRenderAssets, // 跨Standard/ExtendedMaterial读取统一准备属性。
    },
};
use bevy_solari::{
    prelude::{RaytracingMesh3d, SolariLighting}, // 统计RenderApp真实提取的代理与目标相机。
    scene::RaytracingSceneBindings,              // 直接读取公开bind_group就绪位。
};

#[cfg(feature = "dlss_rr")] // 普通test_panel不能引用需要外部NVIDIA SDK的RenderApp组件。
use bevy::{
    anti_alias::dlss::{Dlss, DlssRayReconstructionFeature}, // 精确筛选真实RR目标视图。
    camera::MainPassResolutionOverride, // Bevy DLSS准备阶段发布的真实内部主通道尺寸。
    render::view::ExtractedView,        // 读取最终输出物理viewport尺寸。
};

use crate::{
    constants::{
        DIFFUSE_INDIRECT_DIAGNOSTIC, DIRECT_LIGHTING_DIAGNOSTIC,
        DLSS_RAY_RECONSTRUCTION_CPU_DIAGNOSTIC, DLSS_RAY_RECONSTRUCTION_DIAGNOSTIC,
        EARLY_PREPASS_GPU_DIAGNOSTIC, MAIN_OPAQUE_PASS_GPU_DIAGNOSTIC,
        MAIN_TRANSPARENT_PASS_GPU_DIAGNOSTIC, SPECULAR_INDIRECT_DIAGNOSTIC,
        WORLD_CACHE_ACTIVE_CELLS_DIAGNOSTIC, WORLD_CACHE_DIAGNOSTIC,
    },
    resources::{
        DeferredRasterBridgeRenderState, DeferredRasterBridgeTargets, RaytracingPrototypeRequest,
        RaytracingPrototypeStatus, RaytracingRuntime, RaytracingTelemetryBridge,
    },
    types::{RaytracingGpuTimings, RaytracingPrototypeStage},
};

#[cfg(feature = "dlss_rr")] // 只有真实厂商构建维护上下文与分辨率代次。
use crate::resources::DlssRenderTelemetryState; // RenderApp内固定小型RR诊断状态。

/// 一组本crate主世界系统链的关闭态轻量计时器。
#[derive(Resource, Default)]
pub(crate) struct DisabledHostTimer {
    started: Option<std::time::Instant>, // 只在请求关闭时记录系统链开始时间。
    last_p95_ms: Option<f64>,            // 每三十个样本刷新一次，避免稳定帧排序分配。
    samples_since_refresh: u8,           // 环形队列满后仍按三十帧节奏刷新，不能每帧排序。
}

/// 在 RenderApp 存在时安装固定小型遥测桥和帧末发布系统。
pub(crate) fn install_render_telemetry(app: &mut App) {
    let bridge = RaytracingTelemetryBridge::default(); // 锁内仅一个Copy快照，不共享任何GPU资源。
    app.insert_resource(bridge.clone()); // 主世界读取上一完整RenderApp帧。
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app
            .insert_resource(bridge) // RenderApp写入同一Arc内的小型快照。
            .init_resource::<DeferredRasterBridgeRenderState>() // 按提取目标代次累计连续就绪帧。
            .add_systems(
                Render,
                publish_render_telemetry.in_set(RenderSystems::Cleanup), // BLAS/TLAS绑定准备和本帧渲染均已完成。
            );
        #[cfg(feature = "dlss_rr")] // 普通Solari构建不安装厂商组件查询。
        render_app
            .init_resource::<DlssRenderTelemetryState>() // 当前唯一RR视图使用固定四标量状态。
            .add_systems(
                Render,
                publish_dlss_render_telemetry
                    .in_set(RenderSystems::Cleanup)
                    .after(publish_render_telemetry), // 先发布通用绑定快照，再补同一帧RR尺寸与上下文代次。
            );
    }
}

/// 在一个完整渲染帧末尾发布实际提取数量与TLAS场景绑定状态。
fn publish_render_telemetry(
    bridge: Res<RaytracingTelemetryBridge>, // 与主世界共享的固定快照。
    scene_bindings: Option<Res<RaytracingSceneBindings>>, // 能力不足时Solari不会创建该资源。
    proxies: Query<(), With<RaytracingMesh3d>>, // 只统计RenderApp实际提取实体。
    cameras: Query<(), With<SolariLighting>>, // 预热必须确认目标相机已经进入RenderApp。
    targets: Res<DeferredRasterBridgeTargets>, // 主世界本帧提取的最多256个兼容材质ID。
    materials: Res<ErasedRenderAssets<PreparedMaterial>>, // 五类具体材质共享的统一GPU准备资产表。
    mut bridge_state: ResMut<DeferredRasterBridgeRenderState>, // 连续完整Deferred帧证据。
    geometry_targets: Res<TerrainGeometryTargets>,
    instances: Query<(
        Entity,
        &RaytracingMesh3d,
        &MeshMaterial3d<StandardMaterial>,
        &GlobalTransform,
        Option<&bevy::pbr::PreviousGlobalTransform>,
    )>,
    gpu_meshes: Res<bevy::render::render_asset::RenderAssets<bevy::render::mesh::RenderMesh>>,
    allocator: Res<bevy::render::mesh::allocator::MeshAllocator>,
    textures: Res<bevy::render::render_asset::RenderAssets<bevy::render::texture::GpuImage>>,
) {
    if bridge_state.revision != targets.revision {
        bridge_state.revision = targets.revision; // 新集合不得继承旧集合连续帧。
        bridge_state.ready_frames = 0;
    }
    let deferred_ready_count = targets
        .ids
        .iter()
        .filter(|id| {
            materials.get(**id).is_some_and(|material| {
                material.properties.render_method == OpaqueRendererMethod::Deferred
            }) // 绑定存在但仍按Forward准备不能冒充有效GBuffer。
        })
        .count()
        .min(u32::MAX as usize) as u32;
    let target_count = targets.ids.len().min(u32::MAX as usize) as u32;
    if target_count > 0 && deferred_ready_count == target_count {
        bridge_state.ready_frames = bridge_state.ready_frames.saturating_add(1); // 只在完整Render Cleanup帧递增。
    } else {
        bridge_state.ready_frames = 0; // 任一资产重新准备或移除都先重新稳定两帧。
    }

    let Ok(mut snapshot) = bridge.0.try_lock() else {
        return; // 渲染线程绝不等待主世界；竞争或锁中毒时下一完整帧再发布。
    };
    snapshot.render_frame = snapshot.render_frame.wrapping_add(1).max(1); // 每次Render调度只递增一次。
    snapshot.proxy_count = proxies.iter().count() as u32; // P1必须严格等于当前全部候选固定契约。
    snapshot.bindings_ready = scene_bindings
        .as_deref()
        .is_some_and(|bindings| bindings.bind_group.is_some()); // bind group存在即BLAS/TLAS、材质和纹理完整。
    let (generation, count, index) = terrain_geometry::geometry_proof(
        &geometry_targets,
        &instances,
        &gpu_meshes,
        &allocator,
        &textures,
    );
    snapshot.geometry_generation = generation;
    snapshot.geometry_count = count;
    snapshot.height_instance_index = index;
    snapshot.solari_camera_count = cameras.iter().count() as u32; // 预热期应严格观察到唯一玩法相机。
    snapshot.deferred_bridge_revision = targets.revision; // 主世界按同一代次拒绝旧集合证据。
    snapshot.deferred_bridge_target_count = target_count; // F1显示兼容材质固定规模。
    snapshot.deferred_bridge_ready_count = deferred_ready_count; // 实际PreparedMaterial就绪数量。
    snapshot.deferred_bridge_ready_frames = bridge_state.ready_frames; // Solari挂接前至少要求两个完整帧。
}

/// 发布RR真实输出/内部尺寸，并用内部尺寸组件的变更位间接计数昂贵NGX上下文创建。
#[cfg(feature = "dlss_rr")] // 真实DLSS构建才会在RenderApp产生这些组件。
#[allow(clippy::type_complexity)] // 唯一 RR 视图由 Solari、DLSS 特性和可选内部尺寸组件共同限定。
fn publish_dlss_render_telemetry(
    bridge: Res<RaytracingTelemetryBridge>, // 与主世界共享同一固定Copy快照。
    mut state: ResMut<DlssRenderTelemetryState>, // 只跟踪当前唯一RR相机会话。
    views: Query<
        (
            Entity,                                  // RenderApp同步后的目标视图实体。
            &ExtractedView,                          // 最终输出viewport。
            Option<Ref<MainPassResolutionOverride>>, // NGX实际内部尺寸及本帧变更位。
        ),
        (
            With<SolariLighting>,                     // 排除普通DLSS或其他辅助相机。
            With<Dlss<DlssRayReconstructionFeature>>, // 只接受真实RR目标。
        ),
    >,
) {
    let Ok((camera, view, resolution_override)) = views.single() else {
        *state = DlssRenderTelemetryState::default(); // 关闭、回退或多视图异常时不得沿用旧会话计数。
        if let Ok(mut snapshot) = bridge.0.try_lock() {
            snapshot.dlss_output_resolution = UVec2::ZERO; // 无唯一目标时清空输出尺寸。
            snapshot.dlss_render_resolution = UVec2::ZERO; // 无唯一目标时清空内部尺寸。
            snapshot.dlss_context_generation = 0; // 下一次相机会话从首次创建重新计数。
            snapshot.dlss_resolution_change_count = 0; // 旧相机尺寸变化不能泄漏到新会话。
        }
        return; // 渲染线程不扫描或猜测多个RR目标。
    };
    if state.camera != Some(camera) {
        *state = DlssRenderTelemetryState::default(); // 相机换代不能继承旧NGX上下文证据。
        state.camera = Some(camera); // 从当前唯一渲染实体重新建立会话。
    }
    let output_resolution = UVec2::new(view.viewport.z, view.viewport.w); // `viewport.zw`就是最终物理输出宽高。
    let render_resolution = resolution_override
        .as_deref()
        .map_or(output_resolution, |resolution| resolution.0); // 首帧准备中暂以输出尺寸托底，随后发布NGX真实override。
    let resolution_changed = state
        .last_output_resolution
        .zip(state.last_render_resolution)
        .is_some_and(|(old_output, old_render)| {
            old_output != output_resolution || old_render != render_resolution
        }); // 首帧只建立基线，不计为运行期尺寸抖动。
    if resolution_changed {
        state.resolution_change_count = state.resolution_change_count.saturating_add(1); // 窗口缩放或切档各计一次实际变化。
    }
    if resolution_override
        .as_ref()
        .is_some_and(|resolution| resolution.is_added() || resolution.is_changed())
    {
        state.context_generation = state.context_generation.saturating_add(1); // Bevy只在新建NGX上下文时重新插入override。
    }
    state.last_output_resolution = Some(output_resolution); // 保存本完整渲染帧输出基线。
    state.last_render_resolution = Some(render_resolution); // 保存本完整渲染帧内部基线。

    let Ok(mut snapshot) = bridge.0.try_lock() else {
        return; // 渲染线程绝不等待主世界读取，下一完整帧会重新发布当前累计值。
    };
    snapshot.dlss_output_resolution = output_resolution; // F1直接显示最终输出像素。
    snapshot.dlss_render_resolution = render_resolution; // F1直接显示NGX实际输入像素。
    snapshot.dlss_context_generation = state.context_generation; // 稳定窗口下必须固定不变。
    snapshot.dlss_resolution_change_count = state.resolution_change_count; // 合法缩放/切档与异常抖动可直接区分。
}

/// 系统链开始时记录关闭态时间；开启请求不记录，避免把后台适配成本混入宿主门。
pub(crate) fn begin_disabled_host_measurement(
    request: Res<RaytracingPrototypeRequest>, // F1当前非持久请求。
    mut timer: ResMut<DisabledHostTimer>,     // 保存本链开始时刻。
) {
    timer.started = (!request.enabled).then(std::time::Instant::now); // 开启态明确清空旧开始值。
}

/// 把上一完整RenderApp快照和四段GPU诊断同步到生命周期资源。
pub(crate) fn sync_render_telemetry(
    bridge: Res<RaytracingTelemetryBridge>, // 只读复制锁内固定快照。
    diagnostics: Res<DiagnosticsStore>,     // RenderDiagnosticsPlugin同步回来的主世界存储。
    mut runtime: ResMut<RaytracingRuntime>, // 预热状态机随后消费不同render_frame。
) {
    if let Ok(snapshot) = bridge.0.try_lock() {
        runtime.render_telemetry = *snapshot; // 主线程非阻塞复制；竞争帧沿用上一完整快照。
    }
    runtime.gpu_timings = RaytracingGpuTimings {
        direct_lighting_ms: diagnostic_value(&diagnostics, &DIRECT_LIGHTING_DIAGNOSTIC),
        diffuse_indirect_ms: diagnostic_value(&diagnostics, &DIFFUSE_INDIRECT_DIAGNOSTIC),
        specular_indirect_ms: diagnostic_value(&diagnostics, &SPECULAR_INDIRECT_DIAGNOSTIC),
        world_cache_ms: diagnostic_value(&diagnostics, &WORLD_CACHE_DIAGNOSTIC),
        early_prepass_gpu_ms: fresh_diagnostic_value(&diagnostics, &EARLY_PREPASS_GPU_DIAGNOSTIC), // 包含新增GPU草地运动矢量绘制且过期后立即隐藏。
        main_opaque_pass_gpu_ms: fresh_diagnostic_value(
            &diagnostics,                     // 读取RenderDiagnosticsPlugin上一完整回读帧。
            &MAIN_OPAQUE_PASS_GPU_DIAGNOSTIC, // 主不透明与AlphaMask聚合通道。
        ),
        main_transparent_pass_gpu_ms: fresh_diagnostic_value(
            &diagnostics,                          // 透明通道没有绘制时允许诊断自然过期。
            &MAIN_TRANSPARENT_PASS_GPU_DIAGNOSTIC, // 只复用Bevy已有时间戳。
        ),
    }; // 缺失任一值时预热不会计入完整诊断帧。
    runtime.world_cache_active_cells = (runtime.solari_inserted
        && matches!(
            runtime.stage,
            RaytracingPrototypeStage::WarmingSolari | RaytracingPrototypeStage::Active
        ))
    .then(|| fresh_diagnostic_count(&diagnostics, &WORLD_CACHE_ACTIVE_CELLS_DIAGNOSTIC))
    .flatten(); // 复用官方已有回读并沿用半秒新鲜度门；关闭或回退阶段不得重新吸回旧会话值。
    runtime.dlss_rr_cpu_ms = runtime
        .dlss_rr_active
        .then(|| {
            fresh_diagnostic_value(&diagnostics, &DLSS_RAY_RECONSTRUCTION_CPU_DIAGNOSTIC) // 捕获同步NGX Evaluate和命令录制的Render线程时间。
        })
        .flatten(); // 只在目标相机活动且测量不超过半秒时发布，回退后不复用旧厂商CPU诊断。
    runtime.dlss_rr_gpu_ms = runtime
        .dlss_rr_active
        .then(|| {
            fresh_diagnostic_value(&diagnostics, &DLSS_RAY_RECONSTRUCTION_DIAGNOSTIC) // 保留原RR GPU时间并统一过期判断。
        })
        .flatten(); // 只在目标相机活动且测量不超过半秒时发布，回退后不复用旧厂商GPU诊断。
    runtime.gpu_diagnostics_fresh = [
        &DIRECT_LIGHTING_DIAGNOSTIC,
        &DIFFUSE_INDIRECT_DIAGNOSTIC,
        &SPECULAR_INDIRECT_DIAGNOSTIC,
        &WORLD_CACHE_DIAGNOSTIC,
    ]
    .into_iter()
    .all(|path| diagnostic_is_fresh(&diagnostics, path)); // 旧会话残留值超过半秒后不能通过新预热门。
    if runtime.gpu_diagnostics_fresh
        && let Some(stamps) = diagnostic_stamps(&diagnostics)
        && diagnostic_frame_advanced(runtime.last_gpu_diagnostic_stamps, stamps)
    {
        runtime.last_gpu_diagnostic_stamps = Some(stamps); // 四段时间戳一起提交，部分更新不污染比较基线。
        runtime.gpu_diagnostic_frame = runtime.gpu_diagnostic_frame.wrapping_add(1).max(1); // 预热只消费完整换新的诊断帧。
    }
}

fn diagnostic_value(diagnostics: &DiagnosticsStore, path: &DiagnosticPath) -> Option<f64> {
    diagnostics
        .get(path)
        .and_then(|diagnostic| diagnostic.value()) // 使用最新完整测量，不用平滑值判定管线是否执行。
}

/// 只返回最近半秒内更新的固定Render诊断，避免关闭通道后继续显示上一模式残值。
fn fresh_diagnostic_value(
    diagnostics: &DiagnosticsStore, // Bevy主世界同步完成的固定诊断存储。
    path: &DiagnosticPath,          // 调用方提供的唯一CPU或GPU诊断路径。
) -> Option<f64> {
    diagnostic_is_fresh(diagnostics, path)
        .then(|| diagnostic_value(diagnostics, path)) // 新鲜性与数值读取使用同一条稳定路径。
        .flatten() // 路径存在但尚无完整测量时继续返回None。
}

/// 把最近半秒内的非负有限计数诊断安全转换为u32；异常值不得伪装成零。
fn fresh_diagnostic_count(diagnostics: &DiagnosticsStore, path: &DiagnosticPath) -> Option<u32> {
    fresh_diagnostic_value(diagnostics, path).and_then(|value| {
        (value.is_finite() && (0.0..=u32::MAX as f64).contains(&value))
            .then_some(value.round() as u32) // GPU计数以f64诊断承载，正常值本应为整数。
    })
}

fn diagnostic_is_fresh(diagnostics: &DiagnosticsStore, path: &DiagnosticPath) -> bool {
    diagnostics
        .get(path)
        .and_then(|diagnostic| diagnostic.measurement())
        .is_some_and(|measurement| measurement.time.elapsed().as_secs_f32() <= 0.5) // 正常帧率下覆盖多个完整渲染帧但排除旧会话。
}

fn diagnostic_stamps(diagnostics: &DiagnosticsStore) -> Option<[std::time::Instant; 4]> {
    let measurement_time = |path: &DiagnosticPath| {
        diagnostics
            .get(path)
            .and_then(|diagnostic| diagnostic.measurement())
            .map(|measurement| measurement.time) // 每段保留自己的时间戳，不能用最早或最晚一项代替完整帧。
    };
    Some([
        measurement_time(&DIRECT_LIGHTING_DIAGNOSTIC)?,
        measurement_time(&DIFFUSE_INDIRECT_DIAGNOSTIC)?,
        measurement_time(&SPECULAR_INDIRECT_DIAGNOSTIC)?,
        measurement_time(&WORLD_CACHE_DIAGNOSTIC)?,
    ])
}

fn diagnostic_frame_advanced(
    previous: Option<[std::time::Instant; 4]>, // 上次完整提交的四段时间戳。
    current: [std::time::Instant; 4],          // 本帧主世界看到的四段最新时间戳。
) -> bool {
    previous.is_none_or(|previous| {
        current
            .into_iter()
            .zip(previous)
            .all(|(current, previous)| current > previous) // 任一阶段仍是旧值时都不能冒充新完整诊断帧。
    })
}

/// 系统链末尾记录关闭态P95，并单列Solari会全量提取的StandardMaterial规模。
pub(crate) fn finish_disabled_host_measurement(
    request: Res<RaytracingPrototypeRequest>, // 只有链首和链尾都处于关闭态才记样本。
    materials: Res<Assets<StandardMaterial>>, // 关闭态Solari仍会克隆的真实资产数量。
    mut timer: ResMut<DisabledHostTimer>,     // 取得链首时刻和缓存P95。
    mut runtime: ResMut<RaytracingRuntime>,   // 保存最多120个有界样本。
    mut status: ResMut<RaytracingPrototypeStatus>, // 发布F1只读宿主诊断。
) {
    if !request.enabled
        && let Some(started) = timer.started.take()
    {
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0; // 只覆盖本crate系统链，不伪称完整feature A/B差值。
        if runtime.disabled_host_samples_ms.len() == 120 {
            runtime.disabled_host_samples_ms.pop_front(); // 固定上限避免长期运行增长。
        }
        runtime.disabled_host_samples_ms.push_back(elapsed_ms);
        timer.samples_since_refresh = timer.samples_since_refresh.saturating_add(1); // 队列长度固定后继续独立计数。
        if timer.last_p95_ms.is_none() || timer.samples_since_refresh >= 30 {
            let mut samples = runtime
                .disabled_host_samples_ms
                .iter()
                .copied()
                .collect::<Vec<_>>(); // 每三十帧最多复制120个f64，远低于帧预算。
            samples.sort_by(f64::total_cmp);
            let index = ((samples.len() - 1) as f64 * 0.95).ceil() as usize;
            timer.last_p95_ms = samples.get(index).copied();
            timer.samples_since_refresh = 0; // 下一次排序至少等待三十个新样本。
        }
    }
    status.publish_host_overhead(timer.last_p95_ms, materials.len() as u32); // 开启态仍保留最近关闭态证据。
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant}; // 构造四段同步和部分更新的稳定时间戳。

    use bevy::diagnostic::{
        Diagnostic,            // 为固定测试路径建立诊断历史。
        DiagnosticMeasurement, // 注入明确时间和值的单份测量。
        DiagnosticPath,        // 与生产辅助函数使用相同键类型。
        DiagnosticsStore,      // 模拟RenderDiagnosticsPlugin同步后的主世界存储。
    };

    use super::{diagnostic_frame_advanced, fresh_diagnostic_count, fresh_diagnostic_value}; // 同时锁定预热帧判定、计数转换与半秒诊断过期门。

    /// 建立只含一个确定测量的固定诊断存储。
    fn diagnostic_store(age: Duration, value: f64) -> (DiagnosticsStore, DiagnosticPath) {
        let path = DiagnosticPath::new("render/test_pass/elapsed_cpu"); // 使用合法三段路径模拟RR CPU诊断。
        let mut diagnostic = Diagnostic::new(path.clone()); // 保存与查询必须共享同一路径身份。
        diagnostic.add_measurement(DiagnosticMeasurement {
            time: Instant::now() - age, // 由调用方明确控制样本是否越过半秒门。
            value,                      // 保留可精确断言的毫秒数值。
        });
        let mut diagnostics = DiagnosticsStore::default(); // 建立无其他噪声项的主世界存储。
        diagnostics.add(diagnostic); // 模拟RenderDiagnosticsPlugin已经完成跨世界同步。
        (diagnostics, path) // 返回存储与查询键供用例复用。
    }

    #[test]
    fn diagnostic_frame_requires_all_four_stages_to_advance() {
        let base = Instant::now();
        let previous = [base; 4];
        let mut partial = previous;
        partial[0] += Duration::from_millis(1); // 只有直接光换新不能代表完整Solari诊断帧。
        assert!(!diagnostic_frame_advanced(Some(previous), partial));
        let complete = [base + Duration::from_millis(1); 4];
        assert!(diagnostic_frame_advanced(Some(previous), complete)); // 四段全部换新后才允许预热计数。
    }

    #[test] // 新鲜样本必须直接进入F1，不能等待额外帧或做平滑替换。
    fn fresh_diagnostic_is_published() {
        let (diagnostics, path) = diagnostic_store(Duration::ZERO, 8.5); // 模拟当前帧测得的NGX同步停顿。
        assert_eq!(fresh_diagnostic_value(&diagnostics, &path), Some(8.5)); // 原始最新值必须保持不变。
    }

    #[test] // 通道关闭或模式切换后的旧样本不得被误归因到当前帧。
    fn stale_diagnostic_is_hidden() {
        let (diagnostics, path) = diagnostic_store(Duration::from_secs(1), 8.5); // 明确越过生产半秒新鲜度门。
        assert_eq!(fresh_diagnostic_value(&diagnostics, &path), None); // F1应显示“--”而不是旧模式残值。
    }

    #[test]
    fn fresh_world_cache_count_rejects_invalid_values() {
        let (diagnostics, path) = diagnostic_store(Duration::ZERO, 42_000.0);
        assert_eq!(fresh_diagnostic_count(&diagnostics, &path), Some(42_000)); // 官方f64计数应无损进入F1。
        let (diagnostics, path) = diagnostic_store(Duration::ZERO, f64::NAN);
        assert_eq!(fresh_diagnostic_count(&diagnostics, &path), None); // NaN不能被强转为误导性的零cell。
        let (diagnostics, path) = diagnostic_store(Duration::ZERO, -1.0);
        assert_eq!(fresh_diagnostic_count(&diagnostics, &path), None); // 负计数代表诊断损坏。
    }
}
