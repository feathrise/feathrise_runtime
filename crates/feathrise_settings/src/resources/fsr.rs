//! FSR 3超分结构化运行状态与相机租约索引。

use crate::FsrMode; // 唯一持久请求模式。
use bevy::prelude::*; // Resource、Entity、UVec2。

#[cfg(feature = "fsr3_upscaling")]
use crate::graphics::GraphicsRuntimeLeaseId; // 相机销毁时回收稳定租约票据。
#[cfg(feature = "fsr3_upscaling")]
use bevy::platform::collections::HashMap; // 合格相机数量有限的稳定索引。

/// FSR请求未能成为实际超分路径时的单一结构化原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fsr3RuntimeBlocker {
    BuildDisabled,                         // 当前二进制未编译FSR候选特性。
    BackendUnsupported { detail: String }, // 非Vulkan或管线初始化失败。
    NoEligibleCamera,                      // 没有活动、完整窗口、透视HDR Camera3d。
    ConflictsWithDlss,                     // 手工配置同时请求DLSS/RR时优先保持既有NVIDIA路径。
    MissingInputs,                         // 深度、运动矢量或HDR格式不满足固定ABI。
    RuntimeFailure { detail: String },     // 资源创建或dispatch校验失败。
}

impl Fsr3RuntimeBlocker {
    /// 返回设置页使用的明确单行文本。
    pub fn display_text(&self) -> String {
        match self {
            Self::BuildDisabled => "当前构建未启用 FSR 3 超分候选".into(),
            Self::BackendUnsupported { detail } => format!("FSR 3 后端不可用：{detail}"),
            Self::NoEligibleCamera => "当前没有可由 FSR 3 接管的窗口透视相机".into(),
            Self::ConflictsWithDlss => "FSR 3 与 DLSS/RR 请求冲突，当前优先保留 DLSS".into(),
            Self::MissingInputs => "FSR 3 缺少 HDR、深度或运动矢量输入".into(),
            Self::RuntimeFailure { detail } => format!("FSR 3 运行失败，已安全回退：{detail}"),
        }
    }
}

/// 画质页、诊断和自动测试共享的FSR 3只读运行状态。
#[derive(Resource, Clone, Debug)]
pub struct Fsr3RuntimeStatus {
    pub requested_mode: FsrMode,       // settings.json与UI中的玩家请求。
    pub effective_mode: FsrMode,       // RenderApp上下文真实创建成功的模式。
    pub target_camera_count: u32,      // 实际具备匹配上下文的合格窗口相机数量。
    pub output_resolution: UVec2,      // 第一台合格相机的最终物理输出尺寸。
    pub internal_resolution: UVec2,    // RenderApp报告的固定低分辨率尺寸。
    pub supported: bool,               // Vulkan后端与共享FSR计算管线是否就绪。
    pub reactive_mask_available: bool, // GPU自动反应遮罩已经进入真实dispatch。
    pub transparency_and_composition_mask_available: bool, // GPU T&C遮罩和颜色历史已经进入真实dispatch。
    pub blocker: Option<Fsr3RuntimeBlocker>,               // 当前单一主因；无请求时为None。
}

impl Default for Fsr3RuntimeStatus {
    fn default() -> Self {
        Self {
            requested_mode: FsrMode::Off,
            effective_mode: FsrMode::Off,
            target_camera_count: 0,
            output_resolution: UVec2::ZERO,
            internal_resolution: UVec2::ZERO,
            supported: false,
            reactive_mask_available: false,
            transparency_and_composition_mask_available: false,
            blocker: (!cfg!(feature = "fsr3_upscaling"))
                .then_some(Fsr3RuntimeBlocker::BuildDisabled), // 便携构建仍能解释保存的请求。
        }
    }
}

impl Fsr3RuntimeStatus {
    /// 只有模式、相机和实际内部尺寸同时成立时才允许UI宣称生效。
    pub fn is_effective(&self) -> bool {
        self.effective_mode.is_enabled()
            && self.target_camera_count > 0
            && self.output_resolution.x > 0
            && self.output_resolution.y > 0
            && self.internal_resolution.x > 0
            && self.internal_resolution.y > 0
            && self.blocker.is_none() // 任一当前阻断都表示普通画质回退。
    }
}

/// 被正式设置接管的相机到唯一图形租约票据的稳定索引。
#[cfg(feature = "fsr3_upscaling")]
#[derive(Resource, Default)]
pub(crate) struct Fsr3LeaseIndex {
    pub(crate) leases: HashMap<Entity, GraphicsRuntimeLeaseId>, // 相机销毁后仍能释放不挂实体的注册表项。
}
