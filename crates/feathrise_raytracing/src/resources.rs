//! 光追原型公开请求/状态与私有生命周期资源。

use bevy::prelude::*; // 公开请求/状态只需要Resource、UVec2等ECS基础类型。

use crate::{
    constants::{MAX_INCREMENTAL_VRAM_MIB, MAX_INSTANCE_COUNT},
    types::{
        DlssRayReconstructionDebugView, DlssRayReconstructionMode,
        DlssRayReconstructionUnavailableReason, RaytracingBudgetSnapshot, RaytracingGpuTimings,
        RaytracingPrototypeBlocker, RaytracingPrototypeStage, RaytracingPrototypeTarget,
        SolariHistoryResetReason,
    },
};

/// Solari 生命周期内部兼容请求；正式画质设置系统每帧把唯一玩家请求镜像到此资源。
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RaytracingPrototypeRequest {
    pub enabled: bool, // true仅表示请求；能力或资产不满足时有效状态仍完整回退为光栅。
    pub dlss_rr_mode: DlssRayReconstructionMode, // 只控制隔离RR A/B，不写settings.json或正式综合预设。
    pub dlss_rr_debug_view: DlssRayReconstructionDebugView, // 只切最终输出/RR前颜色呈现，不改变历史或质量档。
}

impl Default for RaytracingPrototypeRequest {
    fn default() -> Self {
        Self {
            enabled: false, // 没有正式设置同步时保持安全光栅，不再由F1维护第二份开关。
            dlss_rr_mode: DlssRayReconstructionMode::Off, // 旧配置和普通构建不自动挂接RR。
            dlss_rr_debug_view: DlssRayReconstructionDebugView::FinalOutput, // 默认保持正常游戏输出，诊断必须手动开启。
        }
    }
}

/// 正式画质页、F1 与自动验收共享的只读 Solari/DLSS RR 运行状态。
#[derive(Resource, Clone, Debug)]
pub struct RaytracingRuntimeStatus {
    requested: bool,                                             // 当前F1请求值。
    effective: bool,                                             // 只有完整全部候选激活时为true。
    stage: RaytracingPrototypeStage,                             // 当前固定状态机阶段。
    target: RaytracingPrototypeTarget, // 当前完整事务真正允许进入TLAS的场景内容边界。
    blocker: Option<RaytracingPrototypeBlocker>, // 当前等待或硬失败原因。
    adapter_name: String,              // Bevy实际选择的适配器名。
    backend: String,                   // Bevy实际选择的wgpu后端。
    missing_capabilities: String,      // Solari缺失feature的可读集合。
    budget: RaytracingBudgetSnapshot,  // 全部候选固定统计和CPU工作集。
    active_tasks: u32,                 // 当前含过期待回收任务在内的异步槽数。
    bindings_ready: bool,              // RenderApp完整TLAS材质bind group是否存在。
    render_proxy_count: u32,           // RenderApp实际观察到的RaytracingMesh3d数量。
    deferred_raster_target_count: u32, // 需要为Solari提供有效GBuffer的光栅材质总数。
    deferred_raster_ready_count: u32,  // RenderApp已经按Deferred准备的兼容材质数。
    deferred_raster_ready_frames: u8,  // 全部就绪后连续完成的光栅渲染帧数。
    history_last_reset_reason: Option<SolariHistoryResetReason>, // 最近一次Solari reset请求的首要原因；不代表world cache已整体清空。
    history_reset_count: u64,    // 当前进程内发生过重置请求的渲染帧总数。
    history_stable_frames: u64, // 当前Solari会话连续没有公开reset请求的完整主世界帧数；不代表局部GI植被像素复用temporal。
    dlss_rr_build_enabled: bool, // 当前二进制是否链接真实NVIDIA DLSS SDK路径。
    dlss_rr_mode: DlssRayReconstructionMode, // 当前F1请求的关闭/DLAA/Quality模式。
    dlss_rr_supported: bool,    // Bevy启动后是否发布真实Ray Reconstruction支持资源。
    dlss_rr_active: bool,       // 目标玩法相机当前是否挂载DLSS RR组件。
    dlss_rr_unavailable_reason: Option<DlssRayReconstructionUnavailableReason>, // 构建、能力或相机等待原因。
    dlss_rr_cpu_ms: Option<f64>, // 最近半秒内同步NGX Evaluate与命令录制的Render线程CPU时间。
    dlss_rr_gpu_ms: Option<f64>, // 最近半秒内的Ray Reconstruction GPU诊断时间。
    dlss_rr_output_resolution: UVec2, // RenderApp观察到的最终输出物理尺寸。
    dlss_rr_render_resolution: UVec2, // NGX上下文实际要求的内部主通道尺寸。
    dlss_rr_context_generation: u64, // 当前相机会话中RR上下文创建/重建次数。
    dlss_rr_resolution_change_count: u64, // 当前相机会话中输出或内部尺寸实际变化次数。
    gpu_timings: RaytracingGpuTimings, // Solari各通道GPU毫秒。
    world_cache_active_cells: Option<u32>, // Bevy现有诊断发布的活跃world-cache cell数。
    disabled_host_cpu_p95_ms: Option<f64>, // 关闭态本crate主世界宿主系统P95；feature A/B仍需外部测量Solari提取成本。
    standard_material_host_count: u32,     // Solari关闭态仍会提取的StandardMaterial资产数量。
}

/// 旧原型名称保留为完整兼容 re-export；正式 API 与类型诊断使用新名称。
pub use RaytracingRuntimeStatus as RaytracingPrototypeStatus;

impl Default for RaytracingRuntimeStatus {
    fn default() -> Self {
        Self {
            requested: false,
            effective: false,
            stage: RaytracingPrototypeStage::Unavailable,
            target: RaytracingPrototypeTarget::None,
            blocker: None,
            adapter_name: "未初始化".to_string(),
            backend: "未知".to_string(),
            missing_capabilities: String::new(),
            budget: RaytracingBudgetSnapshot::default(),
            active_tasks: 0,
            bindings_ready: false,
            render_proxy_count: 0,
            deferred_raster_target_count: 0,
            deferred_raster_ready_count: 0,
            deferred_raster_ready_frames: 0,
            history_last_reset_reason: None,
            history_reset_count: 0,
            history_stable_frames: 0,
            dlss_rr_build_enabled: cfg!(feature = "dlss_rr"), // 默认crate构建不声明外部SDK已经安装。
            dlss_rr_mode: DlssRayReconstructionMode::Off,     // 与正式设置全关默认值保持一致。
            dlss_rr_supported: false,
            dlss_rr_active: false,
            dlss_rr_unavailable_reason: Some(DlssRayReconstructionUnavailableReason::BuildDisabled), // 真实feature系统首帧会按设备支持与相机阶段替换原因。
            dlss_rr_cpu_ms: None, // 未执行真实RR节点前没有Render线程CPU样本。
            dlss_rr_gpu_ms: None,
            dlss_rr_output_resolution: UVec2::ZERO, // 未进入RenderApp前没有可报告输出尺寸。
            dlss_rr_render_resolution: UVec2::ZERO, // 未创建NGX上下文前没有内部尺寸。
            dlss_rr_context_generation: 0,          // 尚未观察到任何上下文创建。
            dlss_rr_resolution_change_count: 0,     // 尚未发生任何尺寸变化。
            gpu_timings: RaytracingGpuTimings::default(),
            world_cache_active_cells: None, // 尚未收到Solari world-cache回读诊断。
            disabled_host_cpu_p95_ms: None,
            standard_material_host_count: 0,
        }
    }
}

impl RaytracingRuntimeStatus {
    pub fn requested(&self) -> bool {
        self.requested
    }
    pub fn effective(&self) -> bool {
        self.effective
    }
    pub fn stage(&self) -> RaytracingPrototypeStage {
        self.stage
    }
    /// 返回当前锁定的场景级光追目标；None表示只保持光栅并等待受支持场景。
    pub fn target(&self) -> RaytracingPrototypeTarget {
        self.target
    }
    pub fn blocker(&self) -> Option<&RaytracingPrototypeBlocker> {
        self.blocker.as_ref()
    }
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }
    pub fn backend(&self) -> &str {
        &self.backend
    }
    pub fn missing_capabilities(&self) -> &str {
        &self.missing_capabilities
    }
    pub fn instance_count(&self) -> u32 {
        self.budget.instances
    }
    /// 返回当前场景目标完整光追契约实例数，避免F1复制目标分支常量。
    pub fn expected_instance_count(&self) -> u32 {
        self.target.expected_instance_count()
    }
    pub fn vertex_count(&self) -> u64 {
        self.budget.vertices
    }
    pub fn triangle_count(&self) -> u64 {
        self.budget.triangles
    }
    pub fn unique_material_count(&self) -> u32 {
        self.budget.unique_materials
    }
    pub fn derived_mesh_bytes(&self) -> u64 {
        self.budget.derived_bytes
    }
    pub fn cpu_working_set_bytes(&self) -> u64 {
        self.budget.cpu_working_set_bytes
    }
    pub fn active_tasks(&self) -> u32 {
        self.active_tasks
    }
    pub fn bindings_ready(&self) -> bool {
        self.bindings_ready
    }
    pub fn render_proxy_count(&self) -> u32 {
        self.render_proxy_count
    }
    pub fn deferred_raster_target_count(&self) -> u32 {
        self.deferred_raster_target_count
    }
    pub fn deferred_raster_ready_count(&self) -> u32 {
        self.deferred_raster_ready_count
    }
    pub fn deferred_raster_ready_frames(&self) -> u8 {
        self.deferred_raster_ready_frames
    }
    /// 返回最近一次历史重置的稳定中文原因；从未启用时明确显示“无”。
    pub fn history_last_reset_reason(&self) -> &'static str {
        self.history_last_reset_reason
            .map_or("无", SolariHistoryResetReason::display_name)
    }
    /// 返回当前进程中发生过历史重置请求的帧数；同帧多个来源只计一次。
    pub fn history_reset_count(&self) -> u64 {
        self.history_reset_count
    }
    /// 返回当前Solari会话距离最近一次公开reset请求的帧数；不代表编译期GI诊断历史状态。
    pub fn history_stable_frames(&self) -> u64 {
        self.history_stable_frames
    }
    /// 返回当前二进制是否编译了需要外部NVIDIA SDK的真实RR路径。
    pub fn dlss_rr_build_enabled(&self) -> bool {
        self.dlss_rr_build_enabled // 与Cargo feature事实一致，不从适配器能力反推构建状态。
    }
    /// 返回F1当前请求的RR模式；能力不足时仍保留该值用于诊断。
    pub fn dlss_rr_mode(&self) -> DlssRayReconstructionMode {
        self.dlss_rr_mode // 请求值与设备有效状态严格分离。
    }
    /// 返回Bevy初始化阶段是否确认当前平台、设备与Vulkan后端支持RR。
    pub fn dlss_rr_supported(&self) -> bool {
        self.dlss_rr_supported // AMD可以为false且不影响基础Solari有效状态。
    }
    /// 返回唯一玩法相机当前是否实际挂载RR组件。
    pub fn dlss_rr_active(&self) -> bool {
        self.dlss_rr_active // 设备支持但仍等待玩法相机时保持false。
    }
    /// 返回构建、能力或相机阶段造成的不可用原因；活动时明确显示“无”。
    pub fn dlss_rr_unavailable_reason(&self) -> &'static str {
        self.dlss_rr_unavailable_reason
            .map_or("无", DlssRayReconstructionUnavailableReason::display_name) // 稳定帧不分配诊断文本。
    }
    /// 返回 RR 是否因当前模式/尺寸上下文创建失败而稳定回退，供正式设置发布结构化原因。
    pub fn dlss_rr_runtime_failed(&self) -> bool {
        self.dlss_rr_unavailable_reason
            == Some(DlssRayReconstructionUnavailableReason::ContextCreationFailed)
    }
    /// 返回最近半秒内DLSS RR节点同步NGX调用与命令录制的Render线程CPU时间。
    pub fn dlss_rr_cpu_ms(&self) -> Option<f64> {
        self.dlss_rr_cpu_ms // 该值用于识别GPU时间无法解释的Render线程同步停顿。
    }
    /// 返回最近半秒内真实执行的DLSS Ray Reconstruction GPU时间。
    pub fn dlss_rr_gpu_ms(&self) -> Option<f64> {
        self.dlss_rr_gpu_ms // 未活动或诊断过期时返回None，禁止显示旧会话残值。
    }
    /// 返回RR最终输出物理尺寸；RenderApp尚未发布时返回None。
    pub fn dlss_rr_output_resolution(&self) -> Option<UVec2> {
        (self.dlss_rr_output_resolution != UVec2::ZERO).then_some(self.dlss_rr_output_resolution) // 零尺寸只作为跨世界桥默认哨兵。
    }
    /// 返回NGX实际采用的内部主通道尺寸；可直接识别1080p Quality降采样。
    pub fn dlss_rr_render_resolution(&self) -> Option<UVec2> {
        (self.dlss_rr_render_resolution != UVec2::ZERO).then_some(self.dlss_rr_render_resolution) // 未活动或首帧准备中不伪造分辨率。
    }
    /// 返回当前目标相机会话中RR上下文创建和重建的累计次数。
    pub fn dlss_rr_context_generation(&self) -> u64 {
        self.dlss_rr_context_generation // 稳定窗口与模式下该值必须保持不变。
    }
    /// 返回当前目标相机会话中输出或内部尺寸真实变化次数。
    pub fn dlss_rr_resolution_change_count(&self) -> u64 {
        self.dlss_rr_resolution_change_count // 窗口缩放或模式切换允许增加，静止帧不得增加。
    }
    /// 返回当前地形目标不可扩大的 TLAS 实例硬上限。
    pub fn maximum_instance_count(&self) -> u32 { MAX_INSTANCE_COUNT }
    /// Bevy 0.20将直接光合并到照明通道。旧接口保留并返回空值。
    pub fn direct_lighting_ms(&self) -> Option<f64> {
        None
    }
    /// Bevy 0.20将漫反射间接光合并到照明通道。旧接口保留并返回空值。
    pub fn diffuse_indirect_ms(&self) -> Option<f64> {
        None
    }
    /// Bevy 0.20将镜面间接光合并到照明通道。旧接口保留并返回空值。
    pub fn specular_indirect_ms(&self) -> Option<f64> {
        None
    }
    /// 返回Solari光源瓦片预采样的GPU时间。
    pub fn presample_light_tiles_ms(&self) -> Option<f64> {
        self.gpu_timings.presample_light_tiles_ms
    }
    /// 返回Solari合并照明通道的GPU时间，包含直接光、间接光和反射。
    pub fn solari_lighting_ms(&self) -> Option<f64> {
        self.gpu_timings.solari_lighting_ms
    }
    pub fn world_cache_ms(&self) -> Option<f64> {
        self.gpu_timings.world_cache_ms
    }
    /// 返回最近半秒内Bevy发布的活跃world-cache cell数量。
    pub fn world_cache_active_cells(&self) -> Option<u32> {
        self.world_cache_active_cells // 复用官方已有诊断，不触发额外GPU回读或场景扫描。
    }
    /// 返回包含Deferred、深度、运动矢量与GPU草地的Early Prepass聚合GPU时间。
    pub fn early_prepass_gpu_ms(&self) -> Option<f64> {
        self.gpu_timings.early_prepass_gpu_ms // 复用Bevy现有通道时间戳，不新增GPU查询。
    }
    /// 返回主不透明与AlphaMask颜色通道聚合GPU时间。
    pub fn main_opaque_pass_gpu_ms(&self) -> Option<f64> {
        self.gpu_timings.main_opaque_pass_gpu_ms // 用于判断RR内部尺寸是否真实降低主要光栅成本。
    }
    /// 返回主透明颜色通道聚合GPU时间。
    pub fn main_transparent_pass_gpu_ms(&self) -> Option<f64> {
        self.gpu_timings.main_transparent_pass_gpu_ms // 无透明绘制或诊断过期时允许返回None。
    }
    pub fn disabled_host_cpu_p95_ms(&self) -> Option<f64> {
        self.disabled_host_cpu_p95_ms
    }
    pub fn standard_material_host_count(&self) -> u32 {
        self.standard_material_host_count
    }
    pub fn maximum_incremental_vram_mib(&self) -> u32 {
        MAX_INCREMENTAL_VRAM_MIB
    }

    pub(crate) fn set_capability(
        &mut self,
        adapter_name: String,
        backend: String,
        missing_capabilities: String,
    ) {
        self.adapter_name = adapter_name; // 启动后适配器身份保持稳定。
        self.backend = backend; // F1明确显示Vulkan或阻断后端。
        self.missing_capabilities = missing_capabilities; // 空字符串表示能力集合完整。
    }

    #[allow(clippy::too_many_arguments)] // F1 遥测逐字段复制固定快照，显式参数避免状态机借用跨越发布边界。
    pub(crate) fn publish_runtime(
        &mut self,
        requested: bool,
        stage: RaytracingPrototypeStage,
        target: RaytracingPrototypeTarget,
        blocker: Option<RaytracingPrototypeBlocker>,
        budget: RaytracingBudgetSnapshot,
        active_tasks: u32,
        bindings_ready: bool,
        render_proxy_count: u32,
        deferred_raster_target_count: u32,
        deferred_raster_ready_count: u32,
        deferred_raster_ready_frames: u8,
        history_last_reset_reason: Option<SolariHistoryResetReason>,
        history_reset_count: u64,
        history_stable_frames: u64,
        gpu_timings: RaytracingGpuTimings,
        world_cache_active_cells: Option<u32>,
    ) {
        self.requested = requested;
        self.effective = stage == RaytracingPrototypeStage::Active;
        self.stage = stage;
        self.target = target;
        self.blocker = blocker;
        self.budget = budget;
        self.active_tasks = active_tasks;
        self.bindings_ready = bindings_ready;
        self.render_proxy_count = render_proxy_count;
        self.deferred_raster_target_count = deferred_raster_target_count;
        self.deferred_raster_ready_count = deferred_raster_ready_count;
        self.deferred_raster_ready_frames = deferred_raster_ready_frames;
        self.history_last_reset_reason = history_last_reset_reason;
        self.history_reset_count = history_reset_count;
        self.history_stable_frames = history_stable_frames;
        self.gpu_timings = gpu_timings;
        self.world_cache_active_cells = world_cache_active_cells;
    }

    /// 单独发布厂商降噪状态，避免继续扩大核心生命周期状态发布函数的参数列表。
    #[allow(clippy::too_many_arguments)] // RR 快照需同帧原子发布能力、耗时、双分辨率与上下文代次。
    pub(crate) fn publish_denoising(
        &mut self,
        build_enabled: bool,
        mode: DlssRayReconstructionMode,
        supported: bool,
        active: bool,
        unavailable_reason: Option<DlssRayReconstructionUnavailableReason>,
        cpu_ms: Option<f64>, // 单列同步NGX调用的Render线程CPU时间，禁止与GPU时间相加冒充整帧。
        gpu_ms: Option<f64>,
        output_resolution: UVec2,
        render_resolution: UVec2,
        context_generation: u64,
        resolution_change_count: u64,
    ) {
        self.dlss_rr_build_enabled = build_enabled; // 明确区分未编译与设备不支持。
        self.dlss_rr_mode = mode; // 即使构建或设备不支持也保留用户A/B请求。
        self.dlss_rr_supported = supported; // 只读取Bevy官方运行时支持资源。
        self.dlss_rr_active = active; // 只表示目标相机组件活动，不改变Solari有效状态。
        self.dlss_rr_unavailable_reason = unavailable_reason; // 活动态为None，其他状态保留稳定原因。
        self.dlss_rr_cpu_ms = cpu_ms; // 过期和回退帧必须发布None，避免把旧Render线程停顿归到新模式。
        self.dlss_rr_gpu_ms = gpu_ms; // 过期和回退帧必须发布None。
        let (published_output, published_render, published_generation, published_changes) =
            if active {
                (
                    output_resolution,
                    render_resolution,
                    context_generation,
                    resolution_change_count,
                ) // 活动态原子发布同一 RR 相机的完整身份与尺寸。
            } else {
                (UVec2::ZERO, UVec2::ZERO, 0, 0) // 回退或关闭同帧清空旧 NGX 会话证据。
            };
        self.dlss_rr_output_resolution = published_output; // 输出尺寸只属于活动会话。
        self.dlss_rr_render_resolution = published_render; // 内部尺寸只属于活动会话。
        self.dlss_rr_context_generation = published_generation; // 新会话从首次创建重新计数。
        self.dlss_rr_resolution_change_count = published_changes; // 旧相机尺寸变化不能泄漏到下一会话。
    }

    pub(crate) fn publish_host_overhead(
        &mut self,
        p95_ms: Option<f64>,
        standard_material_count: u32,
    ) {
        self.disabled_host_cpu_p95_ms = p95_ms; // 只记录本crate关闭态主世界宿主成本。
        self.standard_material_host_count = standard_material_count; // 单列Solari全材质提取规模供外部A/B。
    }

}

mod runtime; // 场景目标、RenderApp遥测桥与可回退会话按私有运行时职责独立维护。
pub(crate) use runtime::*; // 保持crate内部既有resources路径不变。
