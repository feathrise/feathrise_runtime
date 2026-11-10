//! 光追目标、RenderApp遥测桥与可回退会话的私有运行时资源。

use std::{
    collections::VecDeque, // 完成结果与关闭态宿主CPU样本使用有界队列。
    sync::{Arc, Mutex},    // 主世界与RenderApp之间只共享固定小型遥测快照。
    time::Instant,         // GPU诊断时间戳用于拒绝复用旧会话测量。
};

use bevy::{
    asset::{AssetId, UntypedAssetId}, // Mesh修订与跨具体类型的RenderApp材质目标身份。
    material::OpaqueRendererMethod,   // 回退必须恢复每份光栅材质原有Forward/Deferred/Auto作者值。
    pbr::StandardMaterial,            // 普通PBR材质兼容桥。
    platform::collections::HashMap,   // 当前场景候选数量固定且很小。
    prelude::*,                       // Resource、Entity、Handle与Mesh。
    render::extract_resource::ExtractResource, // 把固定上限的材质目标身份复制到RenderApp核对。
};
use feathrise_environment::{
    // 风动树木与通用地表都在Solari期提供有效Deferred GBuffer。
    mesh_wind::TreeWindMaterial,
    terrain_surface::TerrainSurfaceMaterial,
};
use feathrise_settings::GraphicsRuntimeLeaseId; // 精确释放设置crate中的相机运行时租约。
use feathrise_scene_core::state::ScenePhase; // 只接受数据驱动动态场景正式目标。
use feathrise_vegetation::VegetationMeshBillboardMaterial; // 远景AlphaMask公告板继续光栅并写入Deferred GBuffer。

use crate::{
    mesh_adapter::{CompletedAdaptation, MeshCaptureJob, PendingAdaptationTask},
    types::{
        DlssRayReconstructionUnavailableReason, PreparedSourceMaterial, RaytracingBudgetSnapshot,
        RaytracingGpuTimings, RaytracingPrototypeBlocker, RaytracingPrototypeStage,
        RaytracingPrototypeTarget, SolariHistoryResetReason, SourceCandidate,
    },
};

#[cfg(feature = "dlss_rr")] // 厂商相机组件快照只存在于真实DLSS RR构建。
use crate::types::DlssRayReconstructionCameraSnapshot; // 回退时精确恢复抖动、Mip偏移和作者DLSS组件。

/// 启动后从 RenderApp 冻结的适配器与 Solari 能力结论。
#[derive(Resource, Clone, Debug, Default)]
pub(crate) struct RaytracingCapability {
    pub(crate) available: bool, // Vulkan且包含全部required_wgpu_features时为true。
    pub(crate) adapter_name: String, // 实际适配器名。
    pub(crate) backend: String, // 实际wgpu后端调试名。
    pub(crate) blocker: Option<RaytracingPrototypeBlocker>, // 不可用时的稳定公开原因。
    pub(crate) missing_capabilities: String, // 空集合或缺失features调试文本。
}

/// 由动态场景地形观察系统更新的场景目标身份。
#[derive(Resource, Clone, Copy, Debug, Default)]
pub(crate) struct RaytracingSceneTracker {
    pub(crate) target: RaytracingPrototypeTarget, // 只记录目录驱动地形的目标类型。
    pub(crate) root: Option<Entity>, // 动态场景的唯一正式primitive实体。
    pub(crate) scene: Option<ScenePhase>, // 防止其他场景复用标记后误接管。
    pub(crate) target_mesh: Option<AssetId<Mesh>>, // 动态未保存Live Apply换句柄时立即生成新代次。
    pub(crate) dynamic_status_revision: u64, // 动态场景Ready修订参与目标身份。
    pub(crate) terrain_revision: u64, // 已采用洞穴/悬崖集合换代使整个TLAS重新准备。
    pub(crate) generation: u64,      // 每次根ready或动态源换代独立递增。
}

impl RaytracingSceneTracker {
    /// 发布一张完整就绪动态场景的唯一地形目标；相同身份稳定帧不重复换代。
    pub(crate) fn mark_dynamic_ready(
        &mut self,
        root: Entity,
        mesh: AssetId<Mesh>,
        status_revision: u64,
    ) {
        if self.target == RaytracingPrototypeTarget::DynamicSceneTerrain
            && self.root == Some(root)
            && self.target_mesh == Some(mesh)
            && self.dynamic_status_revision == status_revision
        {
            return; // 稳定场景每帧只做固定字段比较，不反复重建BLAS。
        }
        self.generation = self.generation.wrapping_add(1).max(1);
        self.target = RaytracingPrototypeTarget::DynamicSceneTerrain;
        self.root = Some(root);
        self.scene = Some(ScenePhase::Active);
        self.target_mesh = Some(mesh);
        self.dynamic_status_revision = status_revision;
    }

    /// 离开动态场景或其Ready证据失效时撤销明确目标，使生命周期完整回退光栅。
    pub(crate) fn clear_dynamic(&mut self) {
        if self.target != RaytracingPrototypeTarget::DynamicSceneTerrain {
            return; // 无目标时不重复递增代次。
        }
        self.generation = self.generation.wrapping_add(1).max(1);
        self.target = RaytracingPrototypeTarget::None;
        self.root = None;
        self.scene = None;
        self.target_mesh = None;
        self.dynamic_status_revision = 0;
    }
}

/// Mesh资产事件递增修订，解决热重载复用同一AssetId时的迟到结果问题。
#[derive(Resource, Default)]
pub(crate) struct MeshRevisionRegistry {
    revisions: HashMap<AssetId<Mesh>, u64>, // 只为实际观察到的Mesh保存小型计数。
}

impl MeshRevisionRegistry {
    pub(crate) fn revision(&mut self, id: AssetId<Mesh>) -> u64 {
        *self.revisions.entry(id).or_insert(1) // 首次发现的已加载资产从修订一开始。
    }

    pub(crate) fn bump(&mut self, id: AssetId<Mesh>) -> u64 {
        let revision = self.revisions.entry(id).or_insert(0); // Modified早于发现时也能建立记录。
        *revision = revision.wrapping_add(1).max(1); // 回绕后仍跳过零值。
        *revision
    }

    pub(crate) fn current(&self, id: AssetId<Mesh>) -> u64 {
        self.revisions.get(&id).copied().unwrap_or(1) // 未观察修改的加载资产等价于修订一。
    }
}

/// RenderApp 在每个完整渲染帧末尾发布的固定小型快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RenderTelemetrySnapshot {
    pub(crate) render_frame: u64, // 每次Render调度完成递增，预热不能重复消费同一帧。
    pub(crate) proxy_count: u32,  // 渲染世界实际提取的光追代理数。
    pub(crate) geometry_generation: u64,
    pub(crate) geometry_count: u32,
    pub(crate) height_instance_index: Option<u32>, // 只有整批GPU几何及纹理完整时才发布实际槽位。
    pub(crate) bindings_ready: bool, // RaytracingSceneBindings.bind_group是否完整存在。
    pub(crate) solari_camera_count: u32, // 渲染世界实际提取的Solari相机数。
    pub(crate) deferred_bridge_revision: u64, // 当前核对的主世界兼容材质目标代次。
    pub(crate) deferred_bridge_target_count: u32, // 本代需要写入有效GBuffer的具体材质资产数。
    pub(crate) deferred_bridge_ready_count: u32, // RenderApp已准备为Deferred的目标数。
    pub(crate) deferred_bridge_ready_frames: u8, // 全部就绪后连续完成的渲染帧数。
    pub(crate) dlss_output_resolution: UVec2, // 目标RR视图的最终输出物理尺寸。
    pub(crate) dlss_render_resolution: UVec2, // `MainPassResolutionOverride`发布的真实内部尺寸。
    pub(crate) dlss_context_generation: u64, // 通过内部尺寸组件变更间接计数NGX上下文创建。
    pub(crate) dlss_resolution_change_count: u64, // 输出或内部尺寸变化次数。
}

/// RenderApp内RR上下文与分辨率的有界跟踪状态；只维护当前唯一Solari相机。
#[cfg(feature = "dlss_rr")] // 普通test_panel不需要查询厂商RenderApp组件。
#[derive(Resource, Default)]
pub(crate) struct DlssRenderTelemetryState {
    pub(crate) camera: Option<Entity>, // 当前唯一RR渲染实体，换代后全部计数重新开始。
    pub(crate) last_output_resolution: Option<UVec2>, // 上一完整渲染帧输出尺寸。
    pub(crate) last_render_resolution: Option<UVec2>, // 上一完整渲染帧内部尺寸。
    pub(crate) context_generation: u64, // 当前相机会话观察到的上下文创建/重建次数。
    pub(crate) resolution_change_count: u64, // 排除首帧后的真实尺寸变化次数。
}

/// Solari会话期间所有仍由光栅绘制的不透明/AlphaMask材质原值与固定目标集合。
#[derive(Resource, Default)]
pub(crate) struct DeferredRasterBridgeState {
    pub(crate) installed: bool, // true表示全局Auto默认和下列具体资产已经由本crate临时接管。
    pub(crate) revision: u64,   // 目标成员变化时递增，RenderApp不得复用上一集合的就绪证据。
    pub(crate) standard: HashMap<AssetId<StandardMaterial>, OpaqueRendererMethod>, // 普通地物、角色与岩石。
    pub(crate) custom: HashMap<UntypedAssetId, OpaqueRendererMethod>, // 应用显式登记的材质，与内建材质共用预算和回退事务。
    pub(crate) tree_wind: HashMap<AssetId<TreeWindMaterial>, OpaqueRendererMethod>, // 树木与灌木动态顶点材质。
    pub(crate) terrain_surface: HashMap<AssetId<TerrainSurfaceMaterial>, OpaqueRendererMethod>, // 通用地表扩展。
    pub(crate) billboard: HashMap<AssetId<VegetationMeshBillboardMaterial>, OpaqueRendererMethod>, // 远景动态公告板。
    pub(crate) sparse_clipmap:
        HashMap<AssetId<feathrise_scene::SparseClipmapSurfaceMaterial>, OpaqueRendererMethod>, // 可见双层高度材质。
}

/// 全局 Deferred 材质桥期间，显式辅助 PBR 相机持有的最小兼容租约。
#[derive(Resource, Default)]
pub(crate) struct DeferredAuxiliaryCameraLeaseState {
    pub(crate) leases: HashMap<Entity, GraphicsRuntimeLeaseId>, // 当前正式消费者为三台；公共标记允许后续有限辅助视图接入。
}

/// 提取到RenderApp的只读材质身份表；最多256项且只在成员变化时重建。
#[derive(Resource, Clone, Debug, Default, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub(crate) struct DeferredRasterBridgeTargets {
    pub(crate) revision: u64,            // 与主世界接管状态使用同一代次。
    pub(crate) ids: Vec<UntypedAssetId>, // 类型擦除后仍由TypeId区分不同材质资产表。
}

/// RenderApp按目标代次累计的连续Deferred准备帧；只保存两个标量且不回传GPU对象。
#[derive(Resource, Default)]
pub(crate) struct DeferredRasterBridgeRenderState {
    pub(crate) revision: u64,    // 目标代次变化时立即清零旧连续帧。
    pub(crate) ready_frames: u8, // 全部目标均为Deferred时饱和递增，否则归零。
}

/// 主世界与 RenderApp 共享的遥测桥；锁内永远只有一个 Copy 快照。
#[derive(Resource, Clone, Default)]
pub(crate) struct RaytracingTelemetryBridge(
    pub(crate) Arc<Mutex<RenderTelemetrySnapshot>>, // 不传输Mesh、材质或GPU对象。
);

/// 回退完成后要进入的稳定状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RollbackDisposition {
    RasterReady, // 请求关闭、场景/相机换代后允许后续重新发现。
    Failed,      // 当前代次硬失败，保持完整光栅直到请求重置或新代次。
}

/// 任一系统提出的单一回退事务请求。
#[derive(Clone, Debug)]
pub(crate) struct PendingRollback {
    pub(crate) blocker: Option<RaytracingPrototypeBlocker>, // 恢复完成后保留的原因。
    pub(crate) disposition: RollbackDisposition,            // 决定是否自动允许本代重试。
}

/// P1 全部暂存状态；任何可见或GPU副作用都能由这里精确撤销。
#[derive(Resource)]
pub(crate) struct RaytracingRuntime {
    pub(crate) generation: u64, // 当前采用或正在构建的场景代次。
    pub(crate) target: RaytracingPrototypeTarget, // 当前候选与预算使用的场景级契约。
    pub(crate) root: Option<Entity>, // 动态场景唯一正式地形实体。
    pub(crate) camera: Option<Entity>, // 当前明确玩法主相机。
    pub(crate) stage: RaytracingPrototypeStage, // 私有状态机权威值。
    pub(crate) blocker: Option<RaytracingPrototypeBlocker>, // 等待或失败原因。
    pub(crate) stage_elapsed_seconds: f32, // 当前冷启动阶段耗时。
    pub(crate) last_request_enabled: bool, // 识别关闭后重新开启以解除同代失败锁。
    pub(crate) failed_generation: Option<u64>, // 同代硬失败不逐帧自动重试。
    pub(crate) candidates: Vec<SourceCandidate>, // 当前场景完整白名单顺序候选。
    pub(crate) budget: RaytracingBudgetSnapshot, // 发现与运行期预算快照。
    pub(crate) capture_cursor: usize, // 下一个尚未启动捕获的候选槽。
    pub(crate) capture_job: Option<MeshCaptureJob>, // 当前唯一主线程分帧捕获。
    pub(crate) tasks: Vec<PendingAdaptationTask>, // 最多两个，含过期待回收任务。
    pub(crate) completed: VecDeque<CompletedAdaptation>, // 任务完成后等待每帧一个采用。
    pub(crate) next_ticket: u64, // 后台任务唯一票据源。
    pub(crate) prepared_materials: Vec<PreparedSourceMaterial>, // 当前目标原始/延迟材质事务。
    pub(crate) created_standard_materials: Vec<Handle<StandardMaterial>>, // 回退时从Assets显式移除。
    pub(crate) created_surface_materials: Vec<Handle<TerrainSurfaceMaterial>>, // 通用地表延迟副本。
    pub(crate) proxy_entities: Vec<Entity>,                               // 本目标无Mesh3d代理。
    pub(crate) visible_materials_activated: bool, // true时回退必须恢复源句柄。
    pub(crate) gpu_assets_prepared: bool,         // 避免等待阶段重复克隆材质和生成代理。
    pub(crate) lease: Option<GraphicsRuntimeLeaseId>, // 设置crate相机租约所有权。
    pub(crate) lease_settle_pending: bool,        // 先让PostUpdate捕获作者状态，再插入Solari组件。
    pub(crate) solari_inserted: bool,             // 相机当前是否拥有SolariLighting。
    pub(crate) dlss_rr_supported: bool, // 当前适配器/后端/NGX是否发布RR支持，不作为基础Solari阻断。
    pub(crate) dlss_rr_active: bool,    // 目标相机当前是否挂接RR组件。
    pub(crate) dlss_rr_unavailable_reason: Option<DlssRayReconstructionUnavailableReason>, // F1稳定原因。
    pub(crate) dlss_rr_cpu_ms: Option<f64>, // 最近新鲜的厂商节点Render线程CPU时间。
    pub(crate) dlss_rr_gpu_ms: Option<f64>, // 最近新鲜的厂商降噪GPU时间。
    #[cfg(feature = "dlss_rr")] // 普通test_panel不持有任何厂商组件类型。
    pub(crate) dlss_rr_camera_snapshot: Option<DlssRayReconstructionCameraSnapshot>, // 回退所有权。
    pub(crate) history_pending_reset_reason: Option<SolariHistoryResetReason>, // Update与PostUpdate在同帧汇合的首个reset原因。
    pub(crate) history_last_reset_reason: Option<SolariHistoryResetReason>, // 最近一次已经完成计数的reset原因。
    pub(crate) history_reset_count: u64, // 当前进程累计发生reset请求的帧数，同帧只累加一次。
    pub(crate) history_stable_frames: u64, // 当前Solari会话连续未请求公开reset的完整帧数；不描述局部GI植被历史门。
    pub(crate) warmup_frames: u8,          // 已确认的不同完整诊断帧数。
    pub(crate) last_warmup_render_frame: u64, // 防止同一遥测快照被重复计数。
    pub(crate) last_warmup_gpu_diagnostic_frame: u64, // 防止半秒内旧诊断或部分更新被重复计数。
    pub(crate) render_telemetry: RenderTelemetrySnapshot, // 最近一次从桥读取的快照。
    pub(crate) gpu_timings: RaytracingGpuTimings, // 最近一次Solari通道诊断。
    pub(crate) world_cache_active_cells: Option<u32>, // 最近半秒内官方活跃cell诊断；旧会话不得复用。
    pub(crate) gpu_diagnostics_fresh: bool, // 各通道测量都来自最近半秒。
    pub(crate) gpu_diagnostic_frame: u64,   // 各通道测量全部换新后才递增一次的完整诊断帧号。
    pub(crate) last_gpu_diagnostic_stamps: Option<[Instant; 3]>, // 各通道上次已消费的独立时间戳。
    pub(crate) pending_rollback: Option<PendingRollback>, // 本帧末尾统一执行的原子撤销。
    pub(crate) disabled_host_samples_ms: VecDeque<f64>, // 最多120份关闭态主世界宿主耗时。
}

impl Default for RaytracingRuntime {
    fn default() -> Self {
        Self {
            generation: 0,
            target: RaytracingPrototypeTarget::None,
            root: None,
            camera: None,
            stage: RaytracingPrototypeStage::Unavailable,
            blocker: None,
            stage_elapsed_seconds: 0.0,
            last_request_enabled: false,
            failed_generation: None,
            candidates: Vec::new(),
            budget: RaytracingBudgetSnapshot::default(),
            capture_cursor: 0,
            capture_job: None,
            tasks: Vec::new(),
            completed: VecDeque::new(),
            next_ticket: 0,
            prepared_materials: Vec::new(),
            created_standard_materials: Vec::new(),
            created_surface_materials: Vec::new(),
            proxy_entities: Vec::new(),
            visible_materials_activated: false,
            gpu_assets_prepared: false,
            lease: None,
            lease_settle_pending: false,
            solari_inserted: false,
            dlss_rr_supported: false,
            dlss_rr_active: false,
            dlss_rr_unavailable_reason: Some(
                DlssRayReconstructionUnavailableReason::BuildDisabled,
            ), // 启用feature后由降噪系统按支持资源改为等待或能力原因。
            dlss_rr_cpu_ms: None, // 普通构建与尚未活动的RR会话没有同步NGX CPU样本。
            dlss_rr_gpu_ms: None,
            #[cfg(feature = "dlss_rr")] // 真实构建启动时尚未接管任何相机。
            dlss_rr_camera_snapshot: None,
            history_pending_reset_reason: None,
            history_last_reset_reason: None,
            history_reset_count: 0,
            history_stable_frames: 0,
            warmup_frames: 0,
            last_warmup_render_frame: 0,
            last_warmup_gpu_diagnostic_frame: 0,
            render_telemetry: RenderTelemetrySnapshot::default(),
            gpu_timings: RaytracingGpuTimings::default(),
            world_cache_active_cells: None,
            gpu_diagnostics_fresh: false,
            gpu_diagnostic_frame: 0,
            last_gpu_diagnostic_stamps: None,
            pending_rollback: None,
            disabled_host_samples_ms: VecDeque::with_capacity(120),
        }
    }
}

impl RaytracingRuntime {
    /// 当前事务完整目标数量；发现、预热、活动验证和F1使用同一权威值。
    pub(crate) fn expected_instance_count(&self) -> u32 {
        if self.target == RaytracingPrototypeTarget::DynamicSceneTerrain
            && !self.candidates.is_empty()
        {
            self.candidates.len() as u32
        } else {
            self.target.expected_instance_count()
        }
    }

    /// 当前事务需要的真实材质资产数量。
    pub(crate) fn expected_unique_material_count(&self) -> u32 {
        if self.target == RaytracingPrototypeTarget::DynamicSceneTerrain
            && !self.candidates.is_empty()
        {
            self.budget.unique_materials
        } else {
            self.target.expected_unique_material_count()
        }
    }

    /// 记录本帧首个 Solari 历史重置来源；同帧后续来源不会把根因覆盖掉。
    pub(crate) fn note_solari_history_reset(&mut self, reason: SolariHistoryResetReason) {
        if self.history_pending_reset_reason.is_none() {
            self.history_pending_reset_reason = Some(reason); // 首个来源最接近真正把reset从false改为true的位置。
        }
    }

    /// 返回本帧是否已经有已知重置来源，用于识别其他系统直接写入reset的情况。
    pub(crate) fn has_pending_solari_history_reset(&self) -> bool {
        self.history_pending_reset_reason.is_some()
    }

    /// 在 PostUpdate 末尾把同帧请求原子计入公开诊断，稳定帧只做饱和整数更新。
    pub(crate) fn finish_solari_history_frame(&mut self) {
        if let Some(reason) = self.history_pending_reset_reason.take() {
            self.history_last_reset_reason = Some(reason); // 最近原因只在真正发生reset的帧变化。
            self.history_reset_count = self.history_reset_count.saturating_add(1); // 极长运行也不允许计数回绕。
            self.history_stable_frames = 0; // reset帧本身没有可复用的连续稳定历史。
        } else {
            self.history_stable_frames = self.history_stable_frames.saturating_add(1); // O(1)证明稳定帧未重复写Solari组件。
        }
    }

    /// Solari未挂接或正在回退时停止当前会话计时，但保留累计次数与最近原因供F1审计。
    pub(crate) fn pause_solari_history_tracking(&mut self) {
        self.history_pending_reset_reason = None; // 回退帧尚未完成的诊断不得泄漏到下一会话。
        self.history_stable_frames = 0; // 新会话必须从首次reset后重新累计稳定帧。
    }

    pub(crate) fn transition(&mut self, stage: RaytracingPrototypeStage) {
        if self.stage != stage {
            self.stage = stage; // 状态真实变化时才更新公开阶段。
            self.stage_elapsed_seconds = 0.0; // 每个冷启动阶段独立执行30秒门。
        }
    }

    pub(crate) fn request_rollback(
        &mut self,
        blocker: Option<RaytracingPrototypeBlocker>,
        disposition: RollbackDisposition,
    ) {
        let replace = self.pending_rollback.as_ref().is_none_or(|pending| {
            pending.disposition == RollbackDisposition::RasterReady
                && disposition == RollbackDisposition::Failed
        }); // 硬失败可以覆盖同帧普通换代回退，普通回退不能掩盖硬失败。
        if replace {
            self.pending_rollback = Some(PendingRollback {
                blocker,
                disposition,
            });
            self.transition(RaytracingPrototypeStage::RollingBack); // F1立即可见回退已开始。
        }
    }

    pub(crate) fn next_task_ticket(&mut self) -> u64 {
        self.next_ticket = self.next_ticket.wrapping_add(1).max(1); // 取消后不复用旧票据。
        self.next_ticket
    }

    pub(crate) fn estimated_cpu_working_set(&self) -> u64 {
        let capture_bytes = self.capture_job.as_ref().map_or(0, |job| {
            job.estimated_allocated_bytes() // 捕获缓冲按实际Vec长度估算，不读取源Mesh。
        });
        let task_bytes = self
            .tasks
            .iter()
            .map(|task| task.estimated_bytes)
            .sum::<u64>(); // 过期任务即使候选已清空也继续占用槽位和保守工作集，直到真正完成回收。
        let completed_bytes = self
            .completed
            .iter()
            .filter_map(|result| result.as_ref().ok())
            .map(|result| result.derived_bytes)
            .sum::<u64>();
        capture_bytes + task_bytes + completed_bytes
    }
}
