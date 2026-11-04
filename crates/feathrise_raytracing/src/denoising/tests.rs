//! DLSS RR 相机所有权、能力降级和Solari历史同步的轻量ECS回归。

use bevy::{
    anti_alias::dlss::{
        Dlss, DlssPerfQualityMode, DlssRayReconstructionFeature,
        DlssRayReconstructionInputColorDebug, DlssRayReconstructionSupported,
        DlssSuperResolutionFeature,
    }, // 使用正式Bevy组件接口与显式质量档构造主世界夹具。
    prelude::*,                                // App、Update、Camera3d与数学类型。
    render::camera::{MipBias, TemporalJitter}, // 验证有值required附件的精确恢复。
};
use feathrise_settings::{GraphicsRuntimeLeaseRegistry, GraphicsRuntimeRequirements};
use bevy_solari::prelude::SolariLighting; // 构造与RR共享历史代次的Solari相机状态。 // 验证RR通过公共声明暂停SR并无泄漏恢复。

use super::{
    maintain_dlss_ray_reconstruction, restore_dlss_ray_reconstruction_on_rollback,
    sync_dlss_antialiasing_requirement, sync_dlss_ray_reconstruction_reset,
}; // 直接验证生产调度注册的三个系统入口。
use crate::{
    resources::{
        PendingRollback, RaytracingPrototypeRequest, RaytracingRuntime, RollbackDisposition,
    }, // 构造F1模式请求、统一回退事务与私有会话状态。
    types::{
        DlssRayReconstructionCameraSnapshot, DlssRayReconstructionDebugView,
        DlssRayReconstructionMode, DlssRayReconstructionUnavailableReason,
        RaytracingPrototypeStage, SolariHistoryResetReason,
    }, // 构造接管前作者快照并核对稳定原因。
};

/// 构造固定为Ray Reconstruction的相机组件，避免各测试复制特征标记字段。
fn ray_reconstruction(reset: bool) -> Dlss<DlssRayReconstructionFeature> {
    Dlss::<DlssRayReconstructionFeature> {
        perf_quality_mode: DlssPerfQualityMode::Quality, // 与正式默认请求相同地显式使用Quality。
        reset,                                           // 测试分别构造需要和不需要清空历史的状态。
        _phantom_data: Default::default(),               // 固定为Ray Reconstruction特征类型。
    }
}

/// 构造只含私有运行时资源的轻量App，不创建窗口、RenderApp或真实GPU。
fn runtime_app() -> App {
    let mut app = App::new(); // 组件所有权测试只需要主世界Update调度。
    let mut request = RaytracingPrototypeRequest::default(); // 生产环境由正式GraphicsSettings桥写入兼容请求。
    request.dlss_rr_mode = DlssRayReconstructionMode::Quality; // 本模块夹具显式选择Quality，禁止依赖已改为Off的内部默认值。
    app.insert_resource(request) // 提供生产维护系统读取的内部兼容模式请求。
        .init_resource::<RaytracingRuntime>() // 使用生产默认值建立未接管会话。
        .init_resource::<GraphicsRuntimeLeaseRegistry>(); // 交接用例复用正式可合并租约。
    app // 返回可由每个用例按需登记单一系统的App。
}

/// 构造即将由RR替换的独立Super Resolution组件。
fn super_resolution() -> Dlss<DlssSuperResolutionFeature> {
    Dlss::<DlssSuperResolutionFeature> {
        perf_quality_mode: DlssPerfQualityMode::Quality, // 与RR请求使用同一明确模式。
        reset: false,                                    // 模拟已经拥有稳定历史的SR。
        _phantom_data: Default::default(),               // 固定为Super Resolution特征类型。
    }
}

#[test]
fn rr_waits_until_super_resolution_is_absent_and_never_coexists() {
    let mut app = runtime_app();
    app.insert_resource(DlssRayReconstructionSupported)
        .add_systems(Update, maintain_dlss_ray_reconstruction);
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            SolariLighting { reset: false },
            super_resolution(),
        ))
        .id(); // 第一帧模拟独立SR仍由设置模块拥有。
    {
        let mut runtime = app.world_mut().resource_mut::<RaytracingRuntime>();
        runtime.camera = Some(camera);
        runtime.stage = RaytracingPrototypeStage::Active;
        runtime.solari_inserted = true;
    }

    app.update(); // RR必须观察到SR仍存在并等待，不得同帧叠加。
    assert!(
        app.world()
            .get::<Dlss<DlssSuperResolutionFeature>>(camera)
            .is_some()
    );
    assert!(
        app.world()
            .get::<Dlss<DlssRayReconstructionFeature>>(camera)
            .is_none()
    );
    assert_eq!(
        app.world()
            .resource::<RaytracingRuntime>()
            .dlss_rr_unavailable_reason,
        Some(DlssRayReconstructionUnavailableReason::TransitioningFromSuperResolution)
    );

    app.world_mut()
        .entity_mut(camera)
        .remove::<Dlss<DlssSuperResolutionFeature>>(); // 模拟设置模块完成SR快照恢复与组件移除。
    app.update(); // 下一帧确认SR缺失后才允许RR挂接。
    assert!(
        app.world()
            .get::<Dlss<DlssSuperResolutionFeature>>(camera)
            .is_none()
    );
    assert!(
        app.world()
            .get::<Dlss<DlssRayReconstructionFeature>>(camera)
            .is_some()
    );

    app.world_mut()
        .resource_mut::<RaytracingPrototypeRequest>()
        .dlss_rr_mode = DlssRayReconstructionMode::Off;
    app.update(); // 关闭RR先归还其作者层；SR由设置模块在后续资格结算重新取得。
    assert!(
        app.world()
            .get::<Dlss<DlssRayReconstructionFeature>>(camera)
            .is_none()
    );
    assert!(
        app.world()
            .get::<Dlss<DlssSuperResolutionFeature>>(camera)
            .is_none()
    ); // 允许这一帧两者都缺失，但从未允许两者共存。
}

#[test]
fn rr_lease_suspends_sr_only_while_rr_can_progress() {
    let mut app = runtime_app();
    app.add_systems(Update, sync_dlss_antialiasing_requirement);
    let camera = app.world_mut().spawn_empty().id();
    let lease = app
        .world_mut()
        .resource_mut::<GraphicsRuntimeLeaseRegistry>()
        .acquire(GraphicsRuntimeRequirements::solari_warmup(camera));
    {
        let mut runtime = app.world_mut().resource_mut::<RaytracingRuntime>();
        runtime.camera = Some(camera);
        runtime.lease = Some(lease);
        runtime.stage = RaytracingPrototypeStage::Active;
        runtime.solari_inserted = true;
        runtime.dlss_rr_supported = true;
    }

    app.update(); // RR请求可推进时先暂停SR，尚未挂接RR前仍保留基础FXAA。
    let pending = app
        .world()
        .resource::<GraphicsRuntimeLeaseRegistry>()
        .requirements_for_camera(camera)
        .expect("Solari租约必须仍存在");
    assert!(pending.suspend_dlss_super_resolution);
    assert!(!pending.suspend_fxaa);

    app.world_mut()
        .resource_mut::<RaytracingRuntime>()
        .dlss_rr_active = true;
    app.update(); // RR真实活动后才暂停FXAA二次平滑。
    let active = app
        .world()
        .resource::<GraphicsRuntimeLeaseRegistry>()
        .requirements_for_camera(camera)
        .expect("活动RR仍复用同一租约");
    assert!(active.suspend_dlss_super_resolution);
    assert!(active.suspend_fxaa);

    {
        let mut runtime = app.world_mut().resource_mut::<RaytracingRuntime>();
        runtime.dlss_rr_active = false;
        runtime.dlss_rr_unavailable_reason =
            Some(DlssRayReconstructionUnavailableReason::ContextCreationFailed);
    }
    app.update(); // 同模式/尺寸上下文失败后解除SR暂停，形成Solari+SR回退。
    let failed = app
        .world()
        .resource::<GraphicsRuntimeLeaseRegistry>()
        .requirements_for_camera(camera)
        .expect("失败回退不释放Solari会话租约");
    assert!(!failed.suspend_dlss_super_resolution);
    assert!(!failed.suspend_fxaa);
}

#[test]
fn supported_solari_camera_gets_dlss_once_and_preserves_baseline_values() {
    let mut app = runtime_app(); // 不启动真实渲染器即可验证主世界挂接事务。
    app.insert_resource(DlssRayReconstructionSupported); // 模拟Bevy完成RTX/Vulkan/NGX能力初始化。
    app.add_systems(Update, maintain_dlss_ray_reconstruction); // 使用生产幂等挂接系统。
    let authored_jitter = Vec2::new(0.25, -0.125); // 构造可与DLSS默认值区分的作者抖动。
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(), // 标记目标为玩法三维相机。
            TemporalJitter {
                offset: authored_jitter, // 保存应进入回退快照的具体作者偏移。
            },
            MipBias(0.375), // 保存应进入回退快照的具体作者Mip偏移。
        ))
        .id(); // 记录生命周期锁定的目标实体。
    {
        let mut runtime = app.world_mut().resource_mut::<RaytracingRuntime>(); // 配置已完成租约结算的Solari预热现场。
        runtime.camera = Some(camera); // 只允许该玩法相机进入RR。
        runtime.stage = RaytracingPrototypeStage::WarmingSolari; // RR应在Solari首帧前挂接。
        runtime.solari_inserted = true; // 表示Deferred桥和相机租约已经完成。
    }

    app.update(); // 首帧捕获作者值并插入RR组件。
    assert!(
        app.world()
            .get::<Dlss<DlssRayReconstructionFeature>>(camera)
            .is_some()
    ); // 支持设备的目标相机必须实际拥有RR组件。
    let runtime = app.world().resource::<RaytracingRuntime>(); // 读取挂接后的单一会话状态。
    assert!(runtime.dlss_rr_supported); // 官方支持资源必须反映到F1状态。
    assert!(runtime.dlss_rr_active); // 相机组件插入后活动位必须同步提交。
    assert!(runtime.pending_rollback.is_none()); // 厂商降噪成功不得改变基础Solari状态机。
    assert_eq!(
        app.world()
            .get::<Dlss<DlssRayReconstructionFeature>>(camera)
            .map(|dlss| dlss.perf_quality_mode),
        Some(DlssPerfQualityMode::Quality)
    ); // 1080p实验默认不得再依赖含义隐蔽的Auto阈值。
    let snapshot = runtime
        .dlss_rr_camera_snapshot
        .as_ref()
        .expect("挂接RR前必须保存一次相机作者快照"); // 回退所有权不能依赖当前可变组件。
    assert!(snapshot.authored_dlss.is_none()); // 本用例相机原本没有厂商组件。
    assert_eq!(
        snapshot
            .temporal_jitter
            .as_ref()
            .map(|jitter| jitter.offset),
        Some(authored_jitter)
    ); // 抖动必须保存具体值而不只是存在位。
    assert_eq!(snapshot.mip_bias.as_ref().map(|bias| bias.0), Some(0.375)); // Mip偏移同样完整保存。

    let _ = runtime; // 明确结束本段只读断言，随后推进第二个稳定帧。
    app.update(); // 稳定帧只能复用既有快照和组件。
    let runtime = app.world().resource::<RaytracingRuntime>(); // 再次读取稳定状态。
    assert_eq!(
        runtime
            .dlss_rr_camera_snapshot
            .as_ref()
            .map(|snapshot| snapshot.camera),
        Some(camera)
    ); // 幂等维护不能用DLSS运行值覆盖首次作者快照。
}

#[test]
fn quality_dlaa_and_off_switch_once_and_reset_both_histories() {
    let mut app = runtime_app(); // 使用同一目标相机验证无需整体光追回退的RR A/B。
    app.insert_resource(DlssRayReconstructionSupported); // 模拟RTX + Vulkan + NGX支持。
    app.add_systems(Update, maintain_dlss_ray_reconstruction); // 使用生产模式维护系统。
    let camera = app
        .world_mut()
        .spawn((Camera3d::default(), SolariLighting { reset: true }))
        .id(); // 初次Solari历史尚未建立。
    {
        let mut runtime = app.world_mut().resource_mut::<RaytracingRuntime>(); // 构造已激活基础Solari会话。
        runtime.camera = Some(camera); // 锁定唯一玩法相机。
        runtime.stage = RaytracingPrototypeStage::Active; // 允许直接进行模式A/B。
        runtime.solari_inserted = true; // 表示图形租约和GBuffer已经完成。
    }

    app.update(); // 默认Quality首次挂接。
    assert_eq!(
        app.world()
            .get::<Dlss<DlssRayReconstructionFeature>>(camera)
            .map(|dlss| dlss.perf_quality_mode),
        Some(DlssPerfQualityMode::Quality)
    ); // 默认行为显式可见。
    app.world_mut()
        .get_mut::<SolariLighting>(camera)
        .expect("测试Solari组件必须存在")
        .reset = false; // 模拟首帧历史已被Render提取消费。
    {
        let mut runtime = app.world_mut().resource_mut::<RaytracingRuntime>(); // 清理测试未注册的PostUpdate诊断提交状态。
        runtime.history_pending_reset_reason = None; // 下一次切档应独立记录新根因。
    }
    app.world_mut()
        .resource_mut::<RaytracingPrototypeRequest>()
        .dlss_rr_mode = DlssRayReconstructionMode::Dlaa; // F1切到原生内部尺寸。

    app.update(); // 只重建一次目标RR上下文并同步两套历史。
    let dlss = app
        .world()
        .get::<Dlss<DlssRayReconstructionFeature>>(camera)
        .expect("DLAA模式必须保留RR组件"); // 读取切档后的厂商组件。
    assert_eq!(dlss.perf_quality_mode, DlssPerfQualityMode::Dlaa); // 模式必须精确更新为DLAA。
    assert!(dlss.reset); // RR历史同帧失效。
    assert!(
        app.world()
            .get::<SolariLighting>(camera)
            .expect("Solari组件必须继续存在")
            .reset
    ); // Solari历史不能继续复用Quality内部分辨率代次。
    assert_eq!(
        app.world()
            .resource::<RaytracingRuntime>()
            .history_pending_reset_reason,
        Some(SolariHistoryResetReason::DenoisingModeChanged)
    ); // F1能够解释本次历史换代。

    app.world_mut()
        .resource_mut::<RaytracingPrototypeRequest>()
        .dlss_rr_mode = DlssRayReconstructionMode::Off; // F1切回基础Solari对照。
    app.update(); // 独立恢复作者附件，不触发完整TLAS回退。
    assert!(
        app.world()
            .get::<Dlss<DlssRayReconstructionFeature>>(camera)
            .is_none()
    ); // 本crate拥有的RR组件必须立即移除。
    let runtime = app.world().resource::<RaytracingRuntime>(); // 核对主动关闭诊断。
    assert!(!runtime.dlss_rr_active); // 基础Solari继续运行但RR活动位关闭。
    assert_eq!(
        runtime.dlss_rr_unavailable_reason,
        Some(DlssRayReconstructionUnavailableReason::RequestedOff)
    ); // 主动关闭不得误报设备不支持。
}

#[test]
fn input_color_debug_view_toggles_without_rebuilding_or_resetting_histories() {
    let mut app = runtime_app(); // 使用轻量主世界证明诊断只改变相机呈现标记。
    app.insert_resource(DlssRayReconstructionSupported); // 模拟真实RR支持设备。
    app.add_systems(Update, maintain_dlss_ray_reconstruction); // 复用生产幂等同步入口。
    let camera = app
        .world_mut()
        .spawn((Camera3d::default(), SolariLighting { reset: false }))
        .id(); // 构造已可接管但尚无RR组件的唯一玩法相机。
    {
        let mut runtime = app.world_mut().resource_mut::<RaytracingRuntime>(); // 配置已激活基础Solari会话。
        runtime.camera = Some(camera); // 锁定诊断目标相机。
        runtime.stage = RaytracingPrototypeStage::Active; // 允许直接挂接默认Quality RR。
        runtime.solari_inserted = true; // 图形租约与GBuffer已经就绪。
    }

    app.update(); // 首帧建立正常Quality RR基线。
    assert!(
        app.world()
            .get::<DlssRayReconstructionInputColorDebug>(camera)
            .is_none()
    ); // 默认必须展示RR结果而不是调试输入。
    app.world_mut()
        .get_mut::<Dlss<DlssRayReconstructionFeature>>(camera)
        .expect("默认Quality RR必须已经挂接")
        .reset = false; // 模拟首帧RR历史失效已经被Render提取消费。
    app.world_mut()
        .get_mut::<SolariLighting>(camera)
        .expect("测试Solari组件必须存在")
        .reset = false; // 模拟首帧Solari历史失效已经被Render提取消费。
    app.world_mut()
        .resource_mut::<RaytracingRuntime>()
        .history_pending_reset_reason = None; // 后续断言只观察呈现切换本身。
    app.world_mut()
        .resource_mut::<RaytracingPrototypeRequest>()
        .dlss_rr_debug_view = DlssRayReconstructionDebugView::InputColor; // F1请求同帧RR前颜色。

    app.update(); // 只插入可提取的输入呈现标记。
    assert!(
        app.world()
            .get::<DlssRayReconstructionInputColorDebug>(camera)
            .is_some()
    ); // RenderApp必须能据此在RR后选择同帧pInColor。
    let dlss = app
        .world()
        .get::<Dlss<DlssRayReconstructionFeature>>(camera)
        .expect("诊断模式不能移除RR组件"); // 核对厂商上下文配置保持原状。
    assert_eq!(dlss.perf_quality_mode, DlssPerfQualityMode::Quality); // 呈现切换不能暗中改为DLAA或其他档。
    assert!(!dlss.reset); // 呈现切换不使RR历史换代。
    assert!(
        !app.world()
            .get::<SolariLighting>(camera)
            .expect("Solari组件必须继续存在")
            .reset
    ); // 呈现切换也不能使Solari历史换代。
    assert!(
        app.world()
            .resource::<RaytracingRuntime>()
            .history_pending_reset_reason
            .is_none()
    ); // F1 reset诊断不得伪造新的不连续原因。

    app.world_mut()
        .resource_mut::<RaytracingPrototypeRequest>()
        .dlss_rr_debug_view = DlssRayReconstructionDebugView::FinalOutput; // 返回正常RR结果。
    app.update(); // 只移除诊断标记。
    assert!(
        app.world()
            .get::<DlssRayReconstructionInputColorDebug>(camera)
            .is_none()
    ); // 正常路径不得残留覆盖RR输出的标记。
    assert!(
        !app.world()
            .get::<Dlss<DlssRayReconstructionFeature>>(camera)
            .expect("返回正常输出后RR仍必须活动")
            .reset
    ); // 往返呈现不重置RR历史。
}

#[test]
fn unsupported_device_keeps_base_solari_session_without_rollback() {
    let mut app = runtime_app(); // 不插入支持资源模拟AMD或NGX初始化失败。
    app.add_systems(Update, maintain_dlss_ray_reconstruction); // 设备能力门只发布诊断。
    let camera = app.world_mut().spawn(Camera3d::default()).id(); // 建立仍可运行基础Solari的相机。
    {
        let mut runtime = app.world_mut().resource_mut::<RaytracingRuntime>(); // 构造已经挂接Solari的P1会话。
        runtime.camera = Some(camera); // 锁定唯一玩法相机。
        runtime.stage = RaytracingPrototypeStage::Active; // 基础Solari已经完成预热。
        runtime.solari_inserted = true; // 相机拥有Solari入口。
    }

    app.update(); // 能力门观察不到RR支持资源。
    let runtime = app.world().resource::<RaytracingRuntime>(); // 读取降级结果。
    assert!(!runtime.dlss_rr_supported); // 设备支持位必须保持false。
    assert!(!runtime.dlss_rr_active); // 不得向不支持设备插入RR组件。
    assert_eq!(
        runtime.dlss_rr_unavailable_reason,
        Some(DlssRayReconstructionUnavailableReason::DeviceOrBackendUnsupported)
    ); // F1必须给出明确厂商/后端能力原因。
    assert!(runtime.pending_rollback.is_none()); // 缺少DLSS不能阻断AMD基础Solari诊断。
    assert_eq!(runtime.stage, RaytracingPrototypeStage::Active); // P1有效阶段保持不变。
}

#[test]
fn rollback_removes_owned_dlss_and_restores_jitter_and_mip_presence() {
    let mut app = runtime_app(); // 使用单系统验证降噪域先于图形租约释放的回退步骤。
    app.add_systems(Update, restore_dlss_ray_reconstruction_on_rollback); // 只登记厂商附件恢复系统。
    let authored_jitter = Vec2::new(-0.2, 0.3); // 模拟DLSS接管前已有的作者抖动。
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),                   // 目标玩法相机。
            ray_reconstruction(false),             // 模拟本crate当前活动的RR组件。
            DlssRayReconstructionInputColorDebug,  // 模拟回退前正在显示RR前颜色。
            TemporalJitter { offset: Vec2::ZERO }, // 模拟RR已经改写的运行期抖动。
            MipBias(-1.0),                         // 模拟RR required关系引入的Mip偏移。
        ))
        .id(); // 保存明确恢复目标。
    {
        let mut runtime = app.world_mut().resource_mut::<RaytracingRuntime>(); // 构造生产回退前的所有权现场。
        runtime.camera = Some(camera); // 生命周期仍指向该相机。
        runtime.dlss_rr_supported = true; // 设备能力不会因关闭会话消失。
        runtime.dlss_rr_active = true; // F1当前仍显示相机活动。
        runtime.dlss_rr_cpu_ms = Some(8.5); // 构造必须随旧视图一起失效的Render线程同步停顿样本。
        runtime.dlss_rr_gpu_ms = Some(3.8); // 构造必须随旧视图一起失效的厂商GPU样本。
        runtime.dlss_rr_camera_snapshot = Some(DlssRayReconstructionCameraSnapshot {
            camera,              // 快照严格绑定同一实体。
            authored_dlss: None, // 作者原本没有RR，因此回退必须移除。
            temporal_jitter: Some(TemporalJitter {
                offset: authored_jitter, // 回退必须恢复该具体偏移。
            }),
            mip_bias: None, // 作者原本没有MipBias，因此不能残留required组件。
        }); // HDR与预通道由后续图形租约恢复，不属于本测试范围。
        runtime.pending_rollback = Some(PendingRollback {
            blocker: None,                                 // 普通用户关闭不需要硬失败原因。
            disposition: RollbackDisposition::RasterReady, // 回退后允许再次开启。
        }); // 只有统一生命周期提出回退时才能消费快照。
    }

    app.update(); // 一次原子恢复厂商组件与两个有值附件。
    assert!(
        app.world()
            .get::<Dlss<DlssRayReconstructionFeature>>(camera)
            .is_none()
    ); // 仅由本crate拥有的RR必须移除。
    assert!(
        app.world()
            .get::<DlssRayReconstructionInputColorDebug>(camera)
            .is_none()
    ); // 本会话拥有的诊断呈现标记也必须原子清理。
    assert_eq!(
        app.world()
            .get::<TemporalJitter>(camera)
            .map(|jitter| jitter.offset),
        Some(authored_jitter)
    ); // 作者抖动具体值必须精确恢复。
    assert!(app.world().get::<MipBias>(camera).is_none()); // 作者缺失的MipBias不得泄漏。
    let runtime = app.world().resource::<RaytracingRuntime>(); // 核对一次性所有权已经归还。
    assert!(runtime.dlss_rr_camera_snapshot.is_none()); // 快照不能被下一会话重复消费。
    assert!(!runtime.dlss_rr_active); // 回退帧F1立即显示目标相机不活动。
    assert!(runtime.dlss_rr_cpu_ms.is_none()); // 旧视图Render线程CPU时间必须同时失效。
    assert!(runtime.dlss_rr_gpu_ms.is_none()); // 旧视图GPU时间必须同时失效。
}

#[test]
fn solari_reset_is_forwarded_once_while_stable_frame_stays_false() {
    let mut app = runtime_app(); // 历史同步只需要主世界组件查询。
    app.add_systems(Update, sync_dlss_ray_reconstruction_reset); // 用Update代替生产PostUpdate不改变纯同步语义。
    let camera = app
        .world_mut()
        .spawn((
            SolariLighting { reset: true }, // 模拟切镜系统本帧请求清空Solari历史。
            ray_reconstruction(false),      // RR尚未收到同帧reset。
        ))
        .id(); // 保存唯一历史目标。
    {
        let mut runtime = app.world_mut().resource_mut::<RaytracingRuntime>(); // 配置活动RR会话。
        runtime.camera = Some(camera); // 同步系统只访问锁定相机。
        runtime.dlss_rr_active = true; // 非活动设备路径必须保持零查询。
    }

    app.update(); // 把Solari最终reset同步到RR。
    assert!(
        app.world()
            .get::<Dlss<DlssRayReconstructionFeature>>(camera)
            .expect("活动RR组件必须仍存在")
            .reset
    ); // 两套时间历史必须在同一帧进入新代次。
    app.world_mut()
        .get_mut::<SolariLighting>(camera)
        .expect("测试Solari组件必须存在")
        .reset = false; // 模拟Render提取已自动消费Solari reset。
    app.world_mut()
        .get_mut::<Dlss<DlssRayReconstructionFeature>>(camera)
        .expect("测试RR组件必须存在")
        .reset = false; // 模拟DLSS提取已自动消费RR reset。

    app.update(); // 稳定帧没有任何历史失效来源。
    assert!(
        !app.world()
            .get::<Dlss<DlssRayReconstructionFeature>>(camera)
            .expect("稳定帧RR组件必须仍存在")
            .reset
    ); // 同步系统不得逐帧把RR reset重新写回true。
}
