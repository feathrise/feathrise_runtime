// 游戏设置 — 运行时渲染组件。

use bevy::{
    core_pipeline::tonemapping::Tonemapping, // Bloom 作者状态使用的色调映射类型。
    pbr::{ContactShadows, ScreenSpaceAmbientOcclusion}, // 屏幕空间遮蔽作者组件需要完整备份并精确恢复。
    prelude::*,                                         // ECS 组件与数学常用类型。
    render::{extract_component::ExtractComponent, render_resource::ShaderType}, // 渲染比例需同步到 RenderApp 并上传逐视图 uniform。
};

#[cfg(feature = "dlss")] // 普通便携构建不得引用 NVIDIA SDK 相机组件。
use crate::DlssMode;
#[cfg(any(feature = "dlss", feature = "fsr3_upscaling"))]
// 任一时间重建器都通过同一租约和抖动快照恢复作者状态。
use crate::graphics::GraphicsRuntimeLeaseId;
#[cfg(any(feature = "dlss", feature = "fsr3_upscaling"))]
use bevy::render::camera::{MipBias, TemporalJitter}; // 保存时间重建 required 关系会覆盖的有值附件。
#[cfg(feature = "dlss")]
use bevy::{
    anti_alias::dlss::{Dlss, DlssSuperResolutionFeature}, // 保存接管前可能存在的作者 SR 组件。
}; // 记录稳定租约与最后一次明确模式，避免稳定帧重建上下文。

/// 窗口 3D 视图本帧采用的主通道与完整输出物理尺寸。
///
/// 该组件只由画质插件拥有；存在即表示主场景先低分辨率绘制，再在原生 UI 之前放大。
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, ExtractComponent, ShaderType)]
#[extract_app(bevy::render::RenderApp)]
pub(crate) struct RenderScaleView {
    pub(crate) render_size: UVec2, // 主通道、阴影接收和体积效果实际覆盖的低分辨率区域。
    pub(crate) output_size: UVec2, // 后处理、UI 与最终窗口输出使用的完整物理尺寸。
}

/// 记录灯光由场景作者声明的阴影资格，临时关闭画质阴影时不丢失原始意图。
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AuthoredShadowMapsEnabled(pub bool); // true 表示重新开启阴影后应恢复投影。

/// 记录窗口Bloom或显式目标HDR租约首次接管前的HDR/色调映射；非窗口仅在明确租约下使用，不传播窗口Bloom。
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AuthoredWindowPostProcessing {
    pub(crate) hdr: bool,                        // 相机原本是否启用 HDR 中间目标。
    pub(crate) tonemapping: Option<Tonemapping>, // 相机原本的显式色调映射组件。
}

/// 记录窗口 3D 相机首次启用玩家曝光补偿前的作者全局曝光。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub(crate) struct AuthoredWindowExposure(pub(crate) f32); // 归零或离开窗口目标时精确恢复该 EV 基线。

/// 记录窗口 3D 相机首次启用全局 CAS 前的作者锐化状态，关闭时精确恢复。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub(crate) struct AuthoredWindowSharpening {
    pub(crate) present: bool,            // false 表示作者原本没有 CAS 组件。
    pub(crate) enabled: bool,            // 作者 CAS 原本是否启用。
    pub(crate) sharpening_strength: f32, // 作者 CAS 原本的锐化强度。
    pub(crate) denoise: bool,            // 作者 CAS 原本是否开启噪点保护。
}

/// 记录玩法相机首次启用全局 SSAO 前的作者组件与预通道资格，关闭或离开玩法相机时恢复。
#[derive(Component, Debug, Clone, PartialEq)]
pub(crate) struct AuthoredGameplayAmbientOcclusion {
    pub(crate) ambient_occlusion: Option<ScreenSpaceAmbientOcclusion>, // 作者原本完整的 SSAO 参数。
    pub(crate) depth_prepass: bool, // true 表示深度预通道在 SSAO 接管前已由作者或其他功能持有。
    pub(crate) normal_prepass: bool, // true 表示法线预通道在 SSAO 接管前已存在。
}

/// 记录玩法相机首次启用全局接触阴影前的作者组件与深度预通道资格。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub(crate) struct AuthoredGameplayContactShadows {
    pub(crate) present: bool,       // false 表示作者原本没有接触阴影组件。
    pub(crate) linear_steps: u32,   // 作者原本的固定步进数量。
    pub(crate) thickness: f32,      // 作者原本用于深度相交的表面厚度估计。
    pub(crate) length: f32,         // 作者原本的世界空间射线长度。
    pub(crate) depth_prepass: bool, // true 表示深度预通道在本功能接管前已有独立所有者。
}

impl AuthoredGameplayContactShadows {
    /// 从作者当前组件和已排除其他后处理临时所有权的深度状态建立备份。
    pub(crate) fn from_current(value: Option<&ContactShadows>, depth_prepass: bool) -> Self {
        Self {
            present: value.is_some(), // 记录组件是否原本存在。
            linear_steps: value.map_or(0, |settings| settings.linear_steps), // 无组件时使用不参与恢复的占位零值。
            thickness: value.map_or(0.0, |settings| settings.thickness), // 无组件时使用不参与恢复的占位零值。
            length: value.map_or(0.0, |settings| settings.length), // 无组件时使用不参与恢复的占位零值。
            depth_prepass, // 保存真实作者深度资格，排除 TAA 与 SSAO 临时组件。
        }
    }

    /// 作者原本存在组件时重建全部参数，否则返回 None 表示应移除全局组件。
    pub(crate) fn contact_shadows(self) -> Option<ContactShadows> {
        self.present.then_some(ContactShadows {
            linear_steps: self.linear_steps, // 精确恢复作者固定步进数量。
            thickness: self.thickness,       // 精确恢复作者厚度估计。
            length: self.length,             // 精确恢复作者射线长度。
        })
    }
}

/// 记录启用 TAA 前窗口相机已有的支撑组件，关闭后只恢复或移除本功能实际接管的状态。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub(crate) struct TaaSupportOwnership {
    pub(crate) temporal_jitter_offset: Option<Vec2>, // Some 表示作者原本已有投影抖动，并保存其原始偏移。
    pub(crate) mip_bias: Option<f32>, // Some 表示作者原本已有纹理 mip 偏移，并保存其原始值。
    pub(crate) depth_prepass: bool,   // true 表示深度预通道并非由本次 TAA 临时引入。
    pub(crate) motion_vector_prepass: bool, // true 表示运动矢量预通道并非由本次 TAA 临时引入。
}

/// 保存 TAA 窗口相机上一帧姿态，用于识别回城复活等同实体瞬移并清空历史。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub(crate) struct TaaCameraHistoryPose {
    pub(crate) translation: Vec3, // 上一帧完成全局变换传播后的相机世界坐标。
    pub(crate) rotation: Quat,    // 上一帧相机世界旋转，用于识别大角度镜头切换。
}

/// 标记当前真正由 DLSS SR 或 RR 接管、可使用专用后处理锐化的相机。
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DlssPostProcessTarget; // 跨 crate 标记不携带模式，权威状态仍由 DlssRuntimeStatus 提供。

/// 标记当前由FSR 3上下文真实接管、普通CAS必须暂停的窗口相机。
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fsr3PostProcessTarget; // 锐化强度由FSR dispatch内部RCAS处理。

#[cfg(feature = "fsr3_upscaling")]
use feathrise_fsr3::Fsr3Upscaling; // 保存接管前可能存在的作者FSR组件。

/// FSR候选首次接管相机前的作者附件与稳定租约所有权。
#[cfg(feature = "fsr3_upscaling")]
#[derive(Component, Clone)]
pub(crate) struct Fsr3UpscalingOwnership {
    pub(crate) lease: GraphicsRuntimeLeaseId, // 关闭、失败或销毁时归还同一票据。
    pub(crate) authored_fsr: Option<Fsr3Upscaling>, // 作者原有组件完整快照或缺失状态。
    pub(crate) temporal_jitter: Option<TemporalJitter>, // 接管前作者抖动值。
    pub(crate) mip_bias: Option<MipBias>,     // 接管前作者纹理LOD偏移。
    pub(crate) applied_mode: crate::FsrMode,  // 最近一次写入运行时的明确模式。
    pub(crate) output_resolution: UVec2,      // 窗口缩放时只请求一次历史reset。
}

/// 独立 DLSS SR 首次接管窗口相机前的作者附件及稳定租约所有权。
#[cfg(feature = "dlss")] // 无 NVIDIA SDK 的普通构建不产生该组件或依赖。
#[derive(Component, Clone)]
pub(crate) struct DlssSuperResolutionOwnership {
    pub(crate) lease: GraphicsRuntimeLeaseId, // 释放 SR 时必须归还同一份画质租约。
    pub(crate) authored_dlss: Option<Dlss<DlssSuperResolutionFeature>>, // 作者原本的完整 SR 参数或缺失状态。
    pub(crate) temporal_jitter: Option<TemporalJitter>,                 // 接管前的完整投影抖动值。
    pub(crate) mip_bias: Option<MipBias>, // 接管前的完整纹理 Mip 偏移。
    pub(crate) applied_mode: DlssMode,    // 最近一次传给 SDK 的明确模式，永不保存 Auto。
    pub(crate) output_resolution: UVec2,  // 最近一次创建上下文时的最终物理输出尺寸。
}
