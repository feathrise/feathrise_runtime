//! FSR 3 超分公开契约与 RenderApp 私有状态。

use crate::masks::{Fsr3MaskPipeline, Fsr3MaskResources}; // 自动遮罩共享管线与逐视图历史保持在独立职责模块。
use bevy::{
    camera::MainPassResolutionOverride, // 私有上下文的低分辨率主通道身份。
    prelude::*,                         // Component、Resource、Reflect、数学类型。
    render::{
        camera::{MipBias, TemporalJitter}, // 时间域投影抖动与纹理 LOD 偏移。
        render_resource::{Buffer, Texture}, // Bevy GPU 包装只用于逐视图辅助资源。
    },
};
use std::sync::{
    Arc,
    Mutex,
    atomic::AtomicBool, // 首帧dispatch成功只发布一次，稳定帧不写共享遥测锁。
}; // RenderApp 与主世界共享低频遥测；逐视图历史受相机节点独占锁保护。
use wgpu_ffx::{FsrContext, FsrView}; // 固定上游管线与历史对象。

/// FSR 3 固定时间域超分质量档。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum Fsr3Quality {
    #[default]
    Quality, // 1.5x 线性放大，约 67% 内部尺寸。
    Balanced,         // 1.7x 线性放大，约 59% 内部尺寸。
    Performance,      // 2.0x 线性放大，50% 内部尺寸。
    UltraPerformance, // 3.0x 线性放大，约 33% 内部尺寸。
}

impl Fsr3Quality {
    /// 返回 AMD 固定档位使用的线性放大倍率。
    pub const fn upscale_ratio(self) -> f32 {
        match self {
            Self::Quality => 1.5,          // 质量档优先保留输入细节。
            Self::Balanced => 1.7,         // 平衡档采用 AMD 标准倍率。
            Self::Performance => 2.0,      // 性能档把宽高各减半。
            Self::UltraPerformance => 3.0, // 超性能档只面向高输出分辨率候选测试。
        }
    }

    /// 从最终物理输出计算大于等于 2 且对齐偶数的固定内部尺寸。
    pub fn render_size(self, output: UVec2) -> UVec2 {
        let ratio = self.upscale_ratio(); // 同一档位宽高使用相同比例，保持像素纵横比。
        let aligned = |dimension: u32| {
            let scaled = ((dimension as f32 / ratio).floor() as u32).max(2); // 极小窗口仍保持上游合法下限。
            (scaled & !1).max(2) // 偶数尺寸避免半分辨率金字塔出现额外奇数边界。
        };
        UVec2::new(aligned(output.x), aligned(output.y)) // 返回固定而非动态分辨率。
    }

    /// 计算时间域超分建议的材质纹理 mip 偏移。
    pub fn mip_bias(self) -> f32 {
        (1.0 / self.upscale_ratio()).log2() - 1.0 // 按输入/输出比例补偿低分辨率纹理选择。
    }
}

/// 添加到窗口透视相机即可请求 FSR 3 时间域超分。
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component, Clone)]
#[require(TemporalJitter, MipBias)]
pub struct Fsr3Upscaling {
    pub quality: Fsr3Quality, // 固定内部尺寸档，切换时重建逐视图历史。
    pub sharpening: f32,      // 上游 RCAS 强度，调用前钳制到 0..=1。
    pub reset: bool,          // 相机切换或历史失效时只消费一次。
}

impl Default for Fsr3Upscaling {
    fn default() -> Self {
        Self {
            quality: Fsr3Quality::Quality, // 候选默认组件使用最高输入质量，但设置默认仍为关闭。
            sharpening: 0.0,               // 不默认叠加 RCAS，避免改变作者锐度。
            reset: true,                   // 新组件没有可复用时间历史。
        }
    }
}

/// 当前渲染后端满足 FSR 候选前置条件时存在的主世界能力事实。
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct Fsr3BackendSupported; // 只表达 Vulkan 资格；管线就绪必须读取共享遥测。

/// 最近一次 FSR 上下文或 dispatch 的低频状态。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fsr3RuntimeTelemetryFeatureStatus {
    pub quality: Option<Fsr3Quality>, // 成功或失败对应的固定质量档。
    pub output_resolution: UVec2,     // 最终 HDR 输出物理尺寸。
    pub internal_resolution: UVec2,   // 实际低分辨率主通道尺寸；失败时可为零。
    pub failure: Option<String>,      // 相同键只记录一次的可显示错误。
}

/// 主世界可读取的 FSR 运行快照。
#[derive(Clone, Debug, Default)]
pub struct Fsr3RuntimeTelemetrySnapshot {
    pub status: Fsr3RuntimeTelemetryFeatureStatus, // 当前唯一窗口视图的候选状态。
    pub pipeline_ready: bool,                      // 首次真实请求后共享计算管线是否创建成功。
    pub reactive_mask_available: bool,             // 应用侧 GPU 反应遮罩管线与纹理已经可用。
    pub transparency_and_composition_mask_available: bool, // GPU T&C 遮罩与两帧颜色历史已经可用。
}

/// RenderApp 与主世界共享的低频遥测，不在稳定帧持续写锁。
#[derive(Resource, Clone, Default)]
pub struct Fsr3RuntimeTelemetry(pub(crate) Arc<Mutex<Fsr3RuntimeTelemetrySnapshot>>);

impl Fsr3RuntimeTelemetry {
    /// 读取一致快照；锁异常时安全返回空状态而不影响渲染。
    pub fn snapshot(&self) -> Fsr3RuntimeTelemetrySnapshot {
        self.0.lock().map(|value| value.clone()).unwrap_or_default() // UI 读取频率低且数据体积固定。
    }

    /// 首次真实请求创建共享计算管线后发布就绪事实。
    pub(crate) fn record_pipeline_ready(&self) {
        let Ok(mut snapshot) = self.0.lock() else {
            return; // 遥测锁损坏不能升级为渲染故障。
        };
        snapshot.pipeline_ready = true;
        if snapshot.status.quality.is_none() {
            snapshot.status = Fsr3RuntimeTelemetryFeatureStatus::default(); // 清除可能存在的全局初始化诊断。
        }
        snapshot.reactive_mask_available = true; // 共享 FSR 与应用侧自动遮罩管线在同一初始化事务中成功。
        snapshot.transparency_and_composition_mask_available = true; // 两种遮罩使用同一计算管线和能力门。
    }

    /// 只在上下文创建、恢复或失败时替换状态。
    pub(crate) fn record_status(&self, status: Fsr3RuntimeTelemetryFeatureStatus) {
        let Ok(mut snapshot) = self.0.lock() else {
            return; // 遥测锁损坏不能升级为渲染故障。
        };
        if status.quality.is_some() {
            snapshot.pipeline_ready = true; // 逐视图状态只可能出现在共享管线已经存在之后。
        }
        snapshot.status = status; // 遮罩能力事实由共享管线初始化决定，不随单视图状态改写。
    }

    /// 结束相机会话时清除逐视图成功/失败，避免下次同尺寸请求误用上一会话结果。
    pub fn clear_view_status(&self) {
        let Ok(mut snapshot) = self.0.lock() else {
            return; // 遥测清理失败不能影响原生回退路径。
        };
        if snapshot.status.quality.is_none() {
            return; // 保留非 Vulkan 或全局管线初始化失败的能力诊断。
        }
        snapshot.status = Fsr3RuntimeTelemetryFeatureStatus::default();
        // 共享遮罩管线仍然驻留；关闭一次相机会话只清理逐视图状态与纹理。
    }
}

/// RenderApp 的一次性共享管线初始化门；关闭或被互斥阻断时永远不触发驱动工作。
#[derive(Resource, Default)]
pub(crate) struct Fsr3PipelineInitialization {
    pub(crate) attempted: bool, // 初始化失败后保持稳定回退，避免每帧重复进入驱动。
}

/// 仅供定向画质诊断的 AMD FSR 3×3 调试画面开关；默认关闭且不参与玩家设置持久化。
#[derive(Resource, Clone, Copy, Debug, Default)]
pub(crate) struct Fsr3DebugView(pub(crate) bool); // 插件启动时读取一次环境变量，稳定帧不访问进程环境。

/// 主世界提取到 RenderApp 的同帧 FSR 相机参数。
#[derive(Component, Clone, Debug)]
pub(crate) struct ExtractedFsr3View {
    pub(crate) settings: Fsr3Upscaling, // 质量、锐化和一次性 reset 快照。
    pub(crate) camera_near: f32,        // Bevy 无限反向 Z 按 FSR 契约传入 `INFINITY`。
    pub(crate) camera_far: f32,         // FSR 反向 Z 的 far 参数承载真实近裁面距离。
    pub(crate) camera_fov_y: f32,       // 垂直视野弧度。
    pub(crate) frame_time_ms: f32,      // 同帧真实时间，至少一毫秒以满足上游校验。
}

/// 全局 FSR 计算管线；分辨率无关，只创建一次。
#[derive(Resource)]
pub(crate) struct Fsr3PipelineContext {
    pub(crate) upscaler: FsrContext,    // 固定上游时间域重建管线。
    pub(crate) masks: Fsr3MaskPipeline, // 应用侧自动 Reactive / T&C 计算管线。
} // 逐相机纹理和历史由 Fsr3RenderContext 持有。

/// 每个输出尺寸和质量档独立拥有的时间历史与辅助输出。
#[derive(Component)]
pub(crate) struct Fsr3RenderContext {
    pub(crate) view: Mutex<FsrView>, // Core3d 节点以有限临界区推进这一台相机的历史。
    pub(crate) dispatch_confirmed: AtomicBool, // 每次上下文换代都必须重新确认一次真实HDR输出。
    pub(crate) quality: Fsr3Quality, // 稳定帧比较键。
    pub(crate) output_resolution: UVec2, // 窗口缩放时触发一次重建。
    pub(crate) internal_resolution: UVec2, // MainPassResolutionOverride 与辅助资源共用。
    pub(crate) input_color: Texture, // 紧凑 Rgba16Float；物理尺寸必须等于 FSR MaxRenderSize。
    pub(crate) masks: Fsr3MaskResources, // 不透明快照、两帧历史与两张外部遮罩。
    pub(crate) dilated_depth: Texture, // R32Float，供上游输出膨胀深度。
    pub(crate) dilated_motion_vectors: Texture, // Rg16Float，供上游输出膨胀运动矢量。
    pub(crate) reconstructed_previous_depth: Buffer, // 每像素 u32 深度历史输出。
}

/// 记住同一模式和输出尺寸的创建/dispatch 失败，避免稳定帧重试或刷日志。
#[derive(Component, Clone, Debug)]
pub(crate) struct Fsr3RuntimeFailure {
    pub(crate) quality: Fsr3Quality,     // 切换质量后允许重新尝试。
    pub(crate) output_resolution: UVec2, // 窗口尺寸变化后允许重新尝试。
}

/// FSR 相机上一帧姿态，用于识别历史失效而不扫描场景实体。
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fsr3CameraHistoryPose {
    pub(crate) translation: Vec3, // 单帧大位移视为传送。
    pub(crate) rotation: Quat,    // 单帧大旋转视为切镜。
}

/// 清理 RenderApp 残留时使用的完整私有组件集合说明锚点。
pub(crate) type Fsr3RenderCleanup = (
    ExtractedFsr3View,
    Fsr3RenderContext,
    Fsr3RuntimeFailure,
    MainPassResolutionOverride,
); // 提取阶段发现主世界不再请求时一次移除全部状态。

#[cfg(test)]
mod tests {
    use super::*; // 纯尺寸与偏移规则不需要创建 RenderApp 或 GPU 设备。

    #[test]
    fn fixed_quality_ratios_map_1080p_to_even_internal_sizes() {
        let output = UVec2::new(1920, 1080);
        assert_eq!(
            Fsr3Quality::Quality.render_size(output),
            UVec2::new(1280, 720)
        );
        assert_eq!(
            Fsr3Quality::Balanced.render_size(output),
            UVec2::new(1128, 634)
        );
        assert_eq!(
            Fsr3Quality::Performance.render_size(output),
            UVec2::new(960, 540)
        );
        assert_eq!(
            Fsr3Quality::UltraPerformance.render_size(output),
            UVec2::new(640, 360)
        );
    }

    #[test]
    fn minimum_supported_outputs_keep_valid_even_dimensions_and_negative_mip_bias() {
        for quality in [
            Fsr3Quality::Quality,
            Fsr3Quality::Balanced,
            Fsr3Quality::Performance,
            Fsr3Quality::UltraPerformance,
        ] {
            let size = quality.render_size(UVec2::splat(2));
            assert_eq!(size, UVec2::splat(2));
            assert!(quality.mip_bias().is_finite());
            assert!(quality.mip_bias() < 0.0);
        }
    }

    #[test]
    fn clearing_view_status_preserves_global_initialization_failure() {
        let telemetry = Fsr3RuntimeTelemetry::default();
        telemetry.record_status(Fsr3RuntimeTelemetryFeatureStatus {
            failure: Some("非 Vulkan".into()),
            ..default()
        });
        telemetry.clear_view_status();
        assert_eq!(
            telemetry.snapshot().status.failure.as_deref(),
            Some("非 Vulkan")
        );

        telemetry.record_status(Fsr3RuntimeTelemetryFeatureStatus {
            quality: Some(Fsr3Quality::Quality),
            output_resolution: UVec2::new(1920, 1080),
            internal_resolution: UVec2::new(1280, 720),
            failure: None,
        });
        telemetry.clear_view_status();
        assert_eq!(
            telemetry.snapshot().status,
            Fsr3RuntimeTelemetryFeatureStatus::default()
        );
    }

    #[test]
    fn pipeline_readiness_survives_view_status_cleanup() {
        let telemetry = Fsr3RuntimeTelemetry::default();
        telemetry.record_pipeline_ready();
        telemetry.record_status(Fsr3RuntimeTelemetryFeatureStatus {
            quality: Some(Fsr3Quality::Performance),
            output_resolution: UVec2::new(2560, 1440),
            internal_resolution: UVec2::new(1280, 720),
            failure: None,
        });
        telemetry.clear_view_status();

        let snapshot = telemetry.snapshot();
        assert!(snapshot.pipeline_ready); // 关闭一次会话不应重新编译分辨率无关的共享管线。
        assert!(snapshot.reactive_mask_available); // 共享遮罩能力也不随视图会话清除。
        assert!(snapshot.transparency_and_composition_mask_available);
        assert_eq!(
            snapshot.status,
            Fsr3RuntimeTelemetryFeatureStatus::default()
        );
    }
}
