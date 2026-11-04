//! 光追原型的公开阶段/诊断值与内部不可混用身份类型。

use bevy::{
    asset::AssetId,        // 后台结果必须携带源与派生资产身份。
    pbr::StandardMaterial, // 普通可见源与光追代理的材质类型。
    prelude::*,            // Entity、Handle、Mesh与数学类型。
};
use feathrise_environment::terrain_surface::TerrainSurfaceMaterial; // 动态地图共用权重地表扩展材质类型。

/// 当前隔离Solari会话允许接管的地图级目标；其余地图和内容始终保持光栅。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RaytracingPrototypeTarget {
    /// 当前场景没有受支持且完整就绪的静态目标。
    #[default]
    None,
    /// 数据驱动动态地图的唯一正式地形primitive。
    DynamicMapTerrain,
}

impl RaytracingPrototypeTarget {
    /// F1显示当前真正进入TLAS的内容边界。
    pub fn display_name(self) -> &'static str {
        match self {
            Self::None => "无",
            Self::DynamicMapTerrain => "动态地图唯一地形",
        }
    }

    /// 当前目标完整事务所需TLAS实例数。
    pub(crate) fn expected_instance_count(self) -> u32 {
        match self {
            Self::None => 0,
            Self::DynamicMapTerrain => crate::constants::DYNAMIC_EXPECTED_INSTANCE_COUNT,
        }
    }

    /// 当前目标完整事务所需真实材质资产数。
    pub(crate) fn expected_unique_material_count(self) -> u32 {
        match self {
            Self::None => 0,
            Self::DynamicMapTerrain => crate::constants::DYNAMIC_EXPECTED_UNIQUE_MATERIAL_COUNT,
        }
    }
}

#[cfg(feature = "dlss_rr")] // 普通Solari构建不得把NVIDIA SDK类型带入依赖图。
use bevy::{
    anti_alias::dlss::{Dlss, DlssRayReconstructionFeature}, // 保存接管前可能存在的作者DLSS RR组件。
    render::camera::{MipBias, TemporalJitter}, // 精确恢复DLSS required组件中的有值相机附件。
};

/// 地图地形 Solari 原型的唯一公开阶段枚举。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RaytracingPrototypeStage {
    #[default]
    Unavailable, // 后端或设备能力不满足Solari硬要求。
    RasterReady,           // 完整光栅路径可用，等待请求或目标地图/相机。
    DiscoveringCandidates, // 正在核对当前目标的完整资产契约与预算。
    AdaptingMeshes,        // 主线程分帧捕获并由有界后台任务派生兼容Mesh。
    AwaitingGpuAssets,     // 派生Mesh、材质、BLAS和TLAS绑定正在进入RenderApp。
    WarmingSolari, // 光栅材质先完整写入Deferred GBuffer并保留标准阴影，再挂接Solari收集两帧证据。
    Active,        // 当前目标全部光追代理与兼容材质已就绪；其余光栅内容继续生成标准阴影。
    RollingBack,   // 正在撤销代理、材质和相机租约。
    Failed,        // 当前地图代次发生硬失败，等待关闭重开或新代次。
}

impl RaytracingPrototypeStage {
    /// 返回 F1 使用的稳定中文阶段名。
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Unavailable => "不可用",
            Self::RasterReady => "光栅就绪",
            Self::DiscoveringCandidates => "候选发现",
            Self::AdaptingMeshes => "后台适配",
            Self::AwaitingGpuAssets => "等待GPU资产",
            Self::WarmingSolari => "Solari预热",
            Self::Active => "光追活动",
            Self::RollingBack => "回退中",
            Self::Failed => "失败",
        }
    }
}

/// 隔离原型允许从F1请求的DLSS Ray Reconstruction模式；不直接暴露厂商crate类型。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DlssRayReconstructionMode {
    Off,  // 完全关闭RR，基础Solari继续运行并恢复单帧FXAA。
    Dlaa, // 使用原生内部尺寸，只做时间重建与抗锯齿，画面最清晰但GPU成本最高。
    #[default]
    Quality, // 明确使用Quality放大，替代封装内部含义隐蔽的Auto选择。
    Balanced, // 平衡内部尺寸与输出质量。
    Performance, // 高输出分辨率优先性能。
    UltraPerformance, // 4K及以上输出的超性能档。
}

impl DlssRayReconstructionMode {
    /// 返回F1按钮与诊断使用的稳定短名称。
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Off => "关闭",                           // 不挂接厂商组件。
            Self::Dlaa => "DLAA原生",                      // 原生分辨率重建。
            Self::Quality => "Quality",                    // 质量档空间放大。
            Self::Balanced => "Balanced",                  // 平衡档空间放大。
            Self::Performance => "Performance",            // 性能档空间放大。
            Self::UltraPerformance => "Ultra Performance", // 超性能档空间放大。
        }
    }

    /// 按正式 UI 质量顺序循环；兼容旧 F1 调试调用，实际请求仍由 `RenderSettings` 覆盖。
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Dlaa,                     // 先比较最清晰的原生重建。
            Self::Dlaa => Self::Quality,                 // 再比较较低内部尺寸的性能档。
            Self::Quality => Self::Balanced,             // 继续进入平衡档。
            Self::Balanced => Self::Performance,         // 继续进入性能档。
            Self::Performance => Self::UltraPerformance, // 最后比较超性能档。
            Self::UltraPerformance => Self::Off,         // 回到基础Solari作为对照。
        }
    }
}

/// F1 隔离诊断允许展示的 DLSS RR 阶段；只改变呈现目标，不改变重建输入或历史。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DlssRayReconstructionDebugView {
    #[default]
    FinalOutput, // 正常展示 RR 的最终输出，作为游戏默认路径。
    InputColor, // RR 仍执行，但展示它同一帧收到的 HDR pInColor。
}

impl DlssRayReconstructionDebugView {
    /// 返回 F1 按钮与诊断使用的稳定短名称。
    pub fn display_name(self) -> &'static str {
        match self {
            Self::FinalOutput => "RR结果",  // 正常时间重建后的最终颜色。
            Self::InputColor => "RR前颜色", // 最近邻呈现内部尺寸输入，避免过滤掩盖孤立黑样本。
        }
    }

    /// 在最终输出与同帧输入之间切换，不触发 RR / Solari 历史重置。
    pub fn next(self) -> Self {
        match self {
            Self::FinalOutput => Self::InputColor, // 进入 RR 前输入隔离臂。
            Self::InputColor => Self::FinalOutput, // 返回正常 RR 输出。
        }
    }
}

/// 原型不能进入或继续光追状态的公开原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RaytracingPrototypeBlocker {
    BackendNotVulkan { backend: String }, // P1只批准Vulkan，不静默强制后端或回退DX12。
    MissingDeviceFeatures { features: String }, // 当前设备缺少Solari要求的wgpu能力。
    WaitingForSupportedMap,               // 当前没有已完成实例化的动态地形目标。
    WaitingForGameplayCamera,             // 当前玩法主相机尚未建立或正在重建。
    CandidateContractMismatch { detail: String }, // 全部候选数量、身份或唯一性不一致。
    AssetBudgetMismatch { detail: String }, // 固定统计或硬预算不一致。
    MeshUnavailable { detail: String },   // 源Mesh资产尚未加载或已被移除。
    MeshFormatUnsupported { detail: String }, // 属性、拓扑、索引或切线生成不兼容。
    MaterialUnavailable { detail: String }, // 普通或通用地表材质资产未能完整准备。
    AsyncTaskFailed { detail: String },   // 后台适配返回明确失败。
    CpuWorkingSetExceeded { bytes: u64 }, // 捕获与任务估算工作集超过64MiB。
    UploadBudgetExceeded { bytes: u64 },  // 单个派生Mesh超过20MiB采用预算。
    GpuBindingsUnavailable,               // 全部候选BLAS/TLAS或材质绑定尚未形成完整bind group。
    DeferredRasterMaterialBudgetExceeded { actual: usize, maximum: usize }, // 光栅GBuffer兼容材质超过固定上限。
    DeferredRasterBridgePreparing { ready: u32, total: u32 }, // 仍有角色/植被等光栅材质未在RenderApp准备为Deferred。
    StageTimedOut { stage: RaytracingPrototypeStage },        // 冷启动阶段超过三十秒。
    SourceChanged,                                            // Mesh或材质热更使当前代次结果失效。
    CameraRebuilt, // 目标玩法相机实体换代，旧租约必须先释放。
}

impl RaytracingPrototypeBlocker {
    /// 生成适合 F1 单行显示的中文原因。
    pub fn display_text(&self) -> String {
        match self {
            Self::BackendNotVulkan { backend } => {
                format!("后端为{backend}，P1请以WGPU_BACKEND=vulkan启动")
            }
            Self::MissingDeviceFeatures { features } => format!("设备缺少能力：{features}"),
            Self::WaitingForSupportedMap => "等待动态地图正式地形目标".to_string(),
            Self::WaitingForGameplayCamera => "等待主场景相机".to_string(),
            Self::CandidateContractMismatch { detail } => format!("候选资产契约不符：{detail}"),
            Self::AssetBudgetMismatch { detail } => format!("目标预算不符：{detail}"),
            Self::MeshUnavailable { detail } => format!("源Mesh不可用：{detail}"),
            Self::MeshFormatUnsupported { detail } => format!("Mesh格式不兼容：{detail}"),
            Self::MaterialUnavailable { detail } => format!("材质未就绪：{detail}"),
            Self::AsyncTaskFailed { detail } => format!("后台适配失败：{detail}"),
            Self::CpuWorkingSetExceeded { bytes } => {
                format!("CPU增量工作集超限：{:.2} MiB", *bytes as f64 / 1_048_576.0)
            }
            Self::UploadBudgetExceeded { bytes } => {
                format!("单帧上传超限：{:.2} MiB", *bytes as f64 / 1_048_576.0)
            }
            Self::GpuBindingsUnavailable => "Solari BLAS/TLAS或材质绑定未完整就绪".to_string(),
            Self::DeferredRasterMaterialBudgetExceeded { actual, maximum } => {
                format!("光栅GBuffer兼容材质超限：至少{actual}/{maximum}")
            }
            Self::DeferredRasterBridgePreparing { ready, total } => {
                format!("等待光栅GBuffer兼容材质：{ready}/{total}")
            }
            Self::StageTimedOut { stage } => format!("{}阶段超过30秒", stage.display_name()),
            Self::SourceChanged => "地图Mesh或材质已热更新，旧代次已回退".to_string(),
            Self::CameraRebuilt => "主场景相机已换代，旧相机状态已回退".to_string(),
        }
    }
}

/// F1 展示的固定资产与运行时预算快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RaytracingBudgetSnapshot {
    pub(crate) instances: u32,             // 当前完整匹配实例数。
    pub(crate) vertices: u64,              // 当前源顶点合计。
    pub(crate) triangles: u64,             // 当前源三角形合计。
    pub(crate) unique_materials: u32,      // 当前唯一材质名数量。
    pub(crate) derived_bytes: u64,         // 预计或已登记派生几何字节。
    pub(crate) cpu_working_set_bytes: u64, // 捕获、任务与完成结果估算工作集。
}

/// Solari 核心计算与相邻主要光栅通道的上一份 GPU 毫秒诊断。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RaytracingGpuTimings {
    pub(crate) direct_lighting_ms: Option<f64>, // 直接光ReSTIR阶段。
    pub(crate) diffuse_indirect_ms: Option<f64>, // 漫反射间接光阶段。
    pub(crate) specular_indirect_ms: Option<f64>, // 镜面间接光/反射阶段。
    pub(crate) world_cache_ms: Option<f64>,     // 世界缓存维护阶段。
    pub(crate) early_prepass_gpu_ms: Option<f64>, // 深度、运动矢量、GBuffer及草地预通道的聚合GPU时间。
    pub(crate) main_opaque_pass_gpu_ms: Option<f64>, // 主不透明与AlphaMask颜色阶段的聚合GPU时间。
    pub(crate) main_transparent_pass_gpu_ms: Option<f64>, // 主透明颜色阶段的聚合GPU时间。
}

impl RaytracingGpuTimings {
    pub(crate) fn all_present(self) -> bool {
        self.direct_lighting_ms.is_some()
            && self.diffuse_indirect_ms.is_some()
            && self.specular_indirect_ms.is_some()
            && self.world_cache_ms.is_some()
    }
}

/// DLSS Ray Reconstruction 未能作用于目标相机时的内部稳定原因。
#[cfg_attr(not(feature = "dlss_rr"), allow(dead_code))] // 普通构建只会使用BuildDisabled分支。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DlssRayReconstructionUnavailableReason {
    #[default]
    BuildDisabled, // 普通test_panel没有显式选择需要外部NVIDIA SDK的真实DLSS构建。
    DeviceOrBackendUnsupported, // 当前平台、适配器、Vulkan后端或NGX运行库没有发布RR支持资源。
    WaitingForSolariCamera,     // 设备支持，但当前地图目标相机尚未进入Solari预热或正在完整回退。
    TransitioningFromSuperResolution, // RR请求已确认，等待独立SR组件先从同一相机退出。
    ContextCreationFailed,      // SDK拒绝同一模式/尺寸；释放RR并允许Solari+SR安全回退。
    RequestedOff,               // 用户在F1中明确选择关闭RR，基础Solari保持活动。
}

impl DlssRayReconstructionUnavailableReason {
    /// 返回 F1 使用的固定中文原因；不在稳定帧构造临时字符串。
    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::BuildDisabled => "当前构建未启用DLSS RR（使用dlss或rtx_rendering feature）", // 引导到现有入口，避免推荐已移除的旧命令。
            Self::DeviceOrBackendUnsupported => {
                "当前设备/后端/NGX运行库不支持DLSS RR，基础Solari仍可用"
            } // AMD等设备不得因此阻断P1光追状态机。
            Self::WaitingForSolariCamera => "等待当前地图Solari目标相机", // 相机建立前没有可挂接的降噪视图。
            Self::TransitioningFromSuperResolution => "等待DLSS SR退出后挂接RR", // 明确显示互斥交接而非能力失败。
            Self::ContextCreationFailed => "DLSS RR上下文创建失败，同一模式/尺寸已停止重试并回退SR",
            Self::RequestedOff => "画质设置已请求关闭DLSS RR，基础Solari仍可用", // 明确区分主动关闭与能力失败。
        }
    }
}

/// DLSS RR 挂接前的相机有值附件快照；HDR与两个预通道由更早取得的图形租约统一恢复。
#[cfg(feature = "dlss_rr")] // 只有真实DLSS构建需要持有厂商组件的作者副本。
#[derive(Clone)]
pub(crate) struct DlssRayReconstructionCameraSnapshot {
    pub(crate) camera: Entity, // 快照只允许恢复到本次会话的明确玩法相机实体。
    pub(crate) authored_dlss: Option<Dlss<DlssRayReconstructionFeature>>, // 外部作者组件存在时回退原值，否则移除本crate组件。
    pub(crate) temporal_jitter: Option<TemporalJitter>, // 保存接管前完整抖动偏移，而非只记录存在位。
    pub(crate) mip_bias: Option<MipBias>,               // 保存接管前纹理Mip偏移值。
}

/// Solari reset请求的首要原因；该标志不整体清空world cache，F1据此审计屏幕历史失效频率。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SolariHistoryResetReason {
    InitialHistory,            // 首次挂接Solari时没有任何可复用的历史。
    CameraTranslation,         // 相机单帧位移越过十二米瞬移阈值。
    CameraRotation,            // 相机单帧旋转越过四十五度切镜阈值。
    ProjectionChanged,         // FOV、裁剪面或投影类型的矩阵位模式发生变化。
    TargetSizeChanged,         // 窗口或目标纹理的物理尺寸发生变化。
    MapGenerationChanged,      // 当前地图目标代次已经换代。
    CelestialLightChanged,     // 天体光方向、照度或颜色发生单帧突变。
    MaterialMembershipChanged, // Deferred光栅兼容材质成员发生新增或移除。
    MaterialReprepared,        // 既有兼容材质被热更并重新准备GPU管线。
    #[cfg_attr(not(feature = "dlss_rr"), allow(dead_code))]
    // 普通Solari构建保留统一显示枚举但不会产生厂商切档事件。
    DenoisingModeChanged, // RR关闭/DLAA/Quality切换导致内部尺寸与时间历史换代。
    ExternalRequest,           // 本crate没有匹配到原因，但Solari reset标志已被其他系统写入。
}

impl SolariHistoryResetReason {
    /// 返回 F1 使用的稳定中文原因，避免诊断系统每帧分配临时字符串。
    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::InitialHistory => "首次启用/历史不存在",
            Self::CameraTranslation => "相机单帧位移超过12米",
            Self::CameraRotation => "相机单帧旋转超过45度",
            Self::ProjectionChanged => "相机投影变化",
            Self::TargetSizeChanged => "渲染目标尺寸变化",
            Self::MapGenerationChanged => "地图代次变化",
            Self::CelestialLightChanged => "天体光突变",
            Self::MaterialMembershipChanged => "兼容材质成员变化",
            Self::MaterialReprepared => "兼容材质重新准备",
            Self::DenoisingModeChanged => "DLSS RR模式/内部分辨率变化",
            Self::ExternalRequest => "外部/未识别请求",
        }
    }
}

/// 一项源 primitive 在当前地图代次中的不可混用身份与统计。
#[derive(Clone, Debug)]
pub(crate) struct SourceCandidate {
    pub(crate) contract_index: usize,          // 固定白名单顺序索引。
    pub(crate) mesh_name: String, // 发现时冻结的GLTF网格身份，由当前目标显式提供。
    pub(crate) material_name: String, // 发现时冻结的GLTF原始材质身份。
    pub(crate) requires_source_uv: bool, // 捕获阶段使用本候选自己的格式契约。
    pub(crate) terrain_surface: bool, // true时准备通用地表扩展材质及静态Solari代理。
    pub(crate) entity: Entity,    // 当前WorldInstance中的源可见实体。
    pub(crate) mesh: Handle<Mesh>, // 保持源Mesh强句柄且绝不原位修改。
    pub(crate) mesh_id: AssetId<Mesh>, // 后台结果采用前再次核对。
    pub(crate) source_revision: u64, // Mesh资产事件递增修订。
    pub(crate) vertex_count: u64, // 发现阶段冻结的源顶点数。
    pub(crate) triangle_count: u64, // 发现阶段冻结的源三角形数。
    pub(crate) derived_bytes: u64, // 四属性加U32索引的预计载荷。
    pub(crate) adaptation_ticket: Option<u64>, // 当前槽位唯一后台任务票据，采用后立即清空。
    pub(crate) derived_mesh: Option<Handle<Mesh>>, // 主线程每帧一个登记后的派生Mesh。
}

/// 后台任务及其结果必须完整携带的地图、源资产和票据身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AdaptationIdentity {
    pub(crate) generation: u64,       // 光追自有地图代次，不复用InstanceId。
    pub(crate) root: Entity,          // 触发WorldInstanceReady的明确地图根。
    pub(crate) source_entity: Entity, // 结果只允许回到同一源primitive。
    pub(crate) source_mesh: AssetId<Mesh>, // 结果只允许对应同一源Mesh资产。
    pub(crate) source_revision: u64,  // 任务启动后热更即使句柄复用也会失效。
    pub(crate) ticket: u64,           // 每个任务唯一且取消后不复用。
    pub(crate) contract_index: usize, // 结果回到当前完整候选中的唯一槽位。
}

/// 一项可见源在预热与激活事务中的原始和 deferred 材质句柄。
#[derive(Clone, Debug)]
pub(crate) enum SourceMaterialBinding {
    Standard {
        source: Handle<StandardMaterial>, // GLB原始可见材质，回退时精确恢复。
        deferred: Handle<StandardMaterial>, // 按源AssetId去重的延迟副本，同时供代理使用。
    },
    TerrainSurface {
        source: Handle<TerrainSurfaceMaterial>, // 通用地表原扩展材质。
        deferred: Handle<TerrainSurfaceMaterial>, // 只把base切Deferred的扩展副本。
        proxy: Handle<StandardMaterial>,        // 由扩展base克隆的Solari标准代理材质。
    },
}

impl SourceMaterialBinding {
    /// 返回只能安装到无Mesh3d光追代理上的StandardMaterial副本。
    pub(crate) fn proxy_material(&self) -> Handle<StandardMaterial> {
        match self {
            Self::Standard { deferred, .. } => deferred.clone(), // 普通源与代理共享同一延迟副本。
            Self::TerrainSurface { proxy, .. } => proxy.clone(), // 扩展材质不能直接进入Solari绑定器。
        }
    }
}

/// 一项完整材质事务记录，与候选顺序一一对应。
#[derive(Clone, Debug)]
pub(crate) struct PreparedSourceMaterial {
    pub(crate) source_entity: Entity, // 激活与恢复都只写同一源实体。
    pub(crate) binding: SourceMaterialBinding, // 保存类型化原始与延迟句柄。
}

/// 相机历史判定使用的单盏天体方向光轻量快照。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CelestialLightSnapshot {
    pub(crate) illuminance: f32,              // 当前方向光照度。
    pub(crate) color: Vec3,                   // 线性RGB颜色。
    pub(crate) direction: Vec3,               // 由最终GlobalTransform得到的世界光线方向。
    pub(crate) angular_diameter_radians: f32, // 当前有效 SunDisk 角直径，变化时旧软阴影历史不可复用。
}
