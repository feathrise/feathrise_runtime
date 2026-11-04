//! DLSS/Solari 正式运行状态、结构化阻断原因与设置文件迁移阶段。

use bevy::prelude::*; // Resource、UVec2 与 ECS 公共类型。

use crate::DlssMode; // 请求与实际模式必须复用持久化公开枚举。

#[cfg(feature = "dlss")] // 租约索引只在 NVIDIA 构建存在。
use crate::graphics::GraphicsRuntimeLeaseId;
#[cfg(feature = "dlss")] // 只有正式 SR 管理器需要追踪实体销毁后的租约回收。
use bevy::platform::collections::HashMap; // 相机数量极小，稳定映射用于清理已销毁实体。 // 目标实体消失后仍可按稳定票据释放租约。

/// 独立 SR 相机到画质租约票据的索引，防止相机直接销毁时泄漏临时控制权。
#[cfg(feature = "dlss")] // 普通构建不创建任何厂商所有权资源。
#[derive(Resource, Default)]
pub(crate) struct DlssSuperResolutionLeaseIndex {
    pub(crate) leases: HashMap<Entity, GraphicsRuntimeLeaseId>, // 每台被正式设置接管的窗口相机至多一份票据。
}

/// 当前正式渲染组合的可观察路径。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DlssRuntimePath {
    /// 纯光栅路径，DLSS 未实际生效。
    #[default]
    Raster,
    /// 光栅照明配合 DLSS Super Resolution。
    RasterSuperResolution,
    /// Solari 活动且 DLSS 关闭，最终使用普通 FXAA。
    SolariFxaa,
    /// Solari 活动且由 DLSS Super Resolution 完成重建。
    SolariSuperResolution,
    /// Solari 活动且由 DLSS Ray Reconstruction 替换 SR。
    SolariRayReconstruction,
}

impl DlssRuntimePath {
    /// 返回设置页“当前有效状态”使用的稳定中文名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Raster => "光栅",                              // 没有厂商重建组件。
            Self::RasterSuperResolution => "光栅 + DLSS SR",     // 独立超分路径。
            Self::SolariFxaa => "Solari + FXAA",                 // 光追开启但 DLSS 关闭。
            Self::SolariSuperResolution => "Solari + DLSS SR",   // 光追与独立超分组合。
            Self::SolariRayReconstruction => "Solari + DLSS RR", // RR 替换 SR 的正式组合。
        }
    }

    /// 判断任一 DLSS 重建器当前是否拥有目标相机。
    pub const fn uses_dlss(self) -> bool {
        matches!(
            self,
            Self::RasterSuperResolution
                | Self::SolariSuperResolution
                | Self::SolariRayReconstruction
        ) // 三条正式 DLSS 路径都临时接管普通画质组件。
    }
}

/// 当前场景相对既有 Solari 内容白名单的覆盖状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RaytracingSceneCoverage {
    /// 没有请求光追，或当前只走光栅路径。
    #[default]
    RasterOnly,
    /// 当前场景受支持，但仅既有白名单内容进入 Solari。
    Partial,
    /// 当前场景没有受支持的 Solari 目标，已安全回退光栅。
    Unsupported,
}

/// DLSS、RR 或 Solari 请求未能成为实际路径时的结构化原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DlssRuntimeBlocker {
    /// 当前二进制未编译正式 RTX 能力。
    BuildDisabled,
    /// 正式 RTX 包要求 Vulkan，但实际后端不同。
    BackendUnsupported { actual: String },
    /// 当前图形设备不具备所需 RTX 或 Solari 能力。
    DeviceUnsupported { detail: String },
    /// NVIDIA 驱动版本不足以创建所请求模式。
    DriverUnsupported { detail: String },
    /// NGX SDK 或所需运行库未完成初始化。
    NgxUnavailable { detail: String },
    /// Auto 输出低于 1080p，按正式规则主动关闭 DLSS。
    ResolutionTooLow { output: UVec2 },
    /// SDK 拒绝当前输出尺寸与质量档组合，但其他模式仍可尝试。
    ModeUnavailable { mode: DlssMode, output: UVec2 },
    /// 当前没有符合窗口、透视、活动且无局部 viewport 约束的相机。
    NoEligibleCamera,
    /// 当前场景没有既有 Solari 内容目标，光追请求回退光栅。
    SceneUnsupported,
    /// SR 与 RR 正在按先移除、后挂接协议交接。
    Transitioning,
    /// RenderApp 创建或执行 DLSS 上下文失败并已非致命回退。
    RuntimeFailure { detail: String },
}

impl DlssRuntimeBlocker {
    /// 返回面向玩家的明确单行原因，不暴露内部枚举调试格式。
    pub fn display_text(&self) -> String {
        match self {
            Self::BuildDisabled => "当前构建未启用正式 RTX 渲染能力".to_string(),
            Self::BackendUnsupported { actual } => {
                format!("当前后端为 {actual}，正式 RTX 包仅支持 Vulkan")
            }
            Self::DeviceUnsupported { detail } => format!("当前设备不支持：{detail}"),
            Self::DriverUnsupported { detail } => format!("NVIDIA 驱动不满足要求：{detail}"),
            Self::NgxUnavailable { detail } => format!("NGX 不可用：{detail}"),
            Self::ResolutionTooLow { output } => format!(
                "Auto 在 {}×{} 下关闭 DLSS（低于 1080p）",
                output.x, output.y
            ),
            Self::ModeUnavailable { mode, output } => format!(
                "{}模式不支持当前 {}×{} 输出",
                mode.display_name(),
                output.x,
                output.y
            ),
            Self::NoEligibleCamera => "当前没有可由 DLSS 接管的窗口透视相机".to_string(),
            Self::SceneUnsupported => "当前场景不支持 Solari，使用光栅路径".to_string(),
            Self::Transitioning => "DLSS SR 与 RR 正在安全切换".to_string(),
            Self::RuntimeFailure { detail } => format!("DLSS 运行失败，已安全回退：{detail}"),
        }
    }
}

/// 画质页、诊断与自动测试共享的 DLSS/Solari 只读运行状态。
#[derive(Resource, Clone, Debug)]
pub struct DlssRuntimeStatus {
    pub requested_mode: DlssMode,           // 玩家设置中的持久请求模式。
    pub effective_mode: DlssMode,           // Auto 解析后且真实生效的明确模式。
    pub path: DlssRuntimePath,              // 本帧最终正式路径，RR 与 SR 永不同时计入。
    pub target_camera_count: u32,           // 当前由 SR 或 RR 实际接管的合格相机数量。
    pub output_resolution: UVec2,           // 当前主要目标相机最终输出物理尺寸。
    pub internal_resolution: UVec2,         // RenderApp 最近报告的 DLSS 内部尺寸。
    pub super_resolution_supported: bool,   // 官方 Bevy 支持资源是否存在。
    pub ray_reconstruction_supported: bool, // 官方 RR 支持资源是否存在。
    pub raytracing_requested: bool,         // 玩家是否请求 Solari。
    pub raytracing_effective: bool,         // 当前场景是否真实挂接 Solari。
    pub ray_reconstruction_requested: bool, // 玩家保留的 RR 偏好。
    pub ray_reconstruction_effective: bool, // RR 是否真实替换 SR。
    pub scene_coverage: RaytracingSceneCoverage, // 当前场景的白名单覆盖状态。
    pub super_resolution_blocker: Option<DlssRuntimeBlocker>, // SR/NGX/分辨率侧当前主因。
    pub raytracing_blocker: Option<DlssRuntimeBlocker>, // Solari/场景/RR 侧当前主因。
    pub transitioning: bool,                // SR 与 RR 是否正在执行至少一帧的安全交接。
    pub blockers: Vec<DlssRuntimeBlocker>,  // 当前可同时存在的后端、能力、场景或切换原因。
}

impl Default for DlssRuntimeStatus {
    fn default() -> Self {
        Self {
            requested_mode: DlssMode::Off,       // 旧配置与普通构建从关闭开始。
            effective_mode: DlssMode::Off,       // 没有真实相机前不宣称任一模式生效。
            path: DlssRuntimePath::Raster,       // 安全基线永远是光栅。
            target_camera_count: 0,              // 尚无相机所有权。
            output_resolution: UVec2::ZERO,      // 尚无可报告目标尺寸。
            internal_resolution: UVec2::ZERO,    // 尚无 NGX 内部尺寸。
            super_resolution_supported: false,   // 由启动后的官方能力资源覆盖。
            ray_reconstruction_supported: false, // 由正式 DLSS 构建能力资源覆盖。
            raytracing_requested: false,         // 设置默认关闭 Solari。
            raytracing_effective: false,         // 生命周期尚未激活。
            ray_reconstruction_requested: false, // 旧配置不预选 RR。
            ray_reconstruction_effective: false, // RR 尚未挂接。
            scene_coverage: RaytracingSceneCoverage::RasterOnly, // 无请求时不显示场景错误。
            super_resolution_blocker: None,      // 首帧能力系统会发布精确 SR 原因。
            raytracing_blocker: None,            // 光追插件存在时会发布精确场景或设备原因。
            transitioning: false,                // 初始没有任何厂商组件需要交接。
            blockers: if cfg!(feature = "dlss") {
                Vec::new() // 正式构建由首个运行帧发布精确能力原因。
            } else {
                vec![DlssRuntimeBlocker::BuildDisabled] // 便携构建明确说明能力未编译。
            },
        }
    }
}

impl DlssRuntimeStatus {
    /// 判断 DLSS 是否实际接管普通抗锯齿、比例和屏幕空间效果。
    pub fn is_dlss_effective(&self) -> bool {
        self.path.uses_dlss() && self.target_camera_count > 0 // 路径与真实相机所有权必须同时成立。
    }

    /// 按 SR 所有权与 Solari/RR 集成标志重新计算互斥最终路径。
    pub fn refresh_path(&mut self, super_resolution_camera_count: u32) {
        self.target_camera_count = if self.ray_reconstruction_effective {
            1 // 当前 Solari 生命周期只允许一台 RR 玩法相机。
        } else {
            super_resolution_camera_count // 独立 SR 可覆盖主菜单或玩法窗口相机。
        };
        self.path = if self.raytracing_effective {
            if self.ray_reconstruction_effective {
                DlssRuntimePath::SolariRayReconstruction // RR 已完成交接，SR 必须为零。
            } else if super_resolution_camera_count > 0 {
                DlssRuntimePath::SolariSuperResolution // Solari 活动但 RR 未请求或不可用。
            } else {
                DlssRuntimePath::SolariFxaa // DLSS 关闭、Auto 低分辨率或失败时使用 FXAA。
            }
        } else if super_resolution_camera_count > 0 {
            DlssRuntimePath::RasterSuperResolution // 光追关闭时 SR 可独立生效。
        } else {
            DlssRuntimePath::Raster // 能力或场景回退后的稳定安全路径。
        };
        self.rebuild_blockers(); // 路径与所有权结算后同步公开原因列表。
    }

    /// 从 SR 与光追两个独立责任域重建公开阻断原因，避免稳定帧累积旧状态。
    pub fn rebuild_blockers(&mut self) {
        self.blockers.clear(); // 每帧只保留当前事实，不沿用上一场景或上一模式原因。
        if let Some(blocker) = self.super_resolution_blocker.clone() {
            self.blockers.push(blocker); // SR 主因保持第一项，便于设置页优先解释 DLSS 请求。
        }
        if let Some(blocker) = self.raytracing_blocker.clone()
            && !self.blockers.contains(&blocker)
        {
            self.blockers.push(blocker); // 场景或 Solari 原因作为第二项且不重复。
        }
        if self.transitioning {
            self.blockers.push(DlssRuntimeBlocker::Transitioning); // 交接只在真实过渡帧显示。
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*; // 组合矩阵只验证公开状态结算，不初始化真实GPU。

    #[test]
    fn runtime_path_matrix_keeps_sr_and_rr_mutually_exclusive() {
        let mut status = DlssRuntimeStatus::default(); // 从光栅安全路径逐项覆盖四种正式组合。

        status.refresh_path(1); // 光追关闭且一台SR相机有效。
        assert_eq!(status.path, DlssRuntimePath::RasterSuperResolution);
        assert_eq!(status.target_camera_count, 1);

        status.raytracing_effective = true; // 当前场景Solari已真实挂接。
        status.refresh_path(1);
        assert_eq!(status.path, DlssRuntimePath::SolariSuperResolution);
        assert_eq!(status.target_camera_count, 1);

        status.ray_reconstruction_effective = true; // RR完成交接后必须替换SR计数。
        status.refresh_path(0);
        assert_eq!(status.path, DlssRuntimePath::SolariRayReconstruction);
        assert_eq!(status.target_camera_count, 1);

        status.ray_reconstruction_effective = false; // DLSS关闭或失败时Solari回到FXAA。
        status.refresh_path(0);
        assert_eq!(status.path, DlssRuntimePath::SolariFxaa);
        assert_eq!(status.target_camera_count, 0);
    }

    #[test]
    fn blockers_are_rebuilt_without_leaking_previous_transition_state() {
        let mut status = DlssRuntimeStatus {
            super_resolution_blocker: Some(DlssRuntimeBlocker::NoEligibleCamera),
            raytracing_blocker: Some(DlssRuntimeBlocker::SceneUnsupported),
            transitioning: true,
            ..default()
        };
        status.rebuild_blockers();
        assert_eq!(status.blockers.len(), 3); // SR、场景和交接三类当前原因同时可见。

        status.super_resolution_blocker = None;
        status.raytracing_blocker = None;
        status.transitioning = false;
        status.rebuild_blockers();
        assert!(status.blockers.is_empty()); // 稳定恢复后不得残留上一帧失败文本。
    }
}
