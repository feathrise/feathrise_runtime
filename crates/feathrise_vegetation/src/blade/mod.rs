//! Bevy 0.19 的通用 GPU 叶片种植后端。
//!
//! 提供多环带GPU分类压缩、间接实例绘制、世界稳定草格、动态风摆、固定容量局部交互，
//! 以及可选RGBA16高度和RGBA8密度输入；消费者负责草种profile、原型网格和纹理内容。

pub(crate) mod components; // 主世界轻量叶片批次与渲染世界持久GPU缓冲组件。
pub(crate) mod constants; // 计算压缩、间接状态与着色器路径共享常量。
#[cfg(feature = "diagnostics")]
pub(crate) mod diagnostics; // 可选F3与严格烟雾共享的无阻塞GPU回读采样配置。
pub(crate) mod gpu_compute; // RenderGraph计算分类、紧凑输出和间接参数准备功能域。
pub(crate) mod gpu_timing; // 固定GPU时间戳、近中远管线统计和异步三缓冲回读功能域。
pub(crate) mod mesh; // 近、中、远与超远四级叶片GLB/复用网格加载入口。
pub(crate) mod render; // 自定义渲染管线、排队与间接实例绘制命令。
pub(crate) mod resources; // 上一完整渲染帧的F3快照与RenderApp内部无锁计数器。
pub(crate) mod shaders; // crate 内置 Compute 与 Draw WGSL 的固定句柄和注册入口。
pub(crate) mod types; // GPU数据布局镜像与每帧计算作业类型。
mod volume_source; // 有界作者源编码与密度空间索引，供宿主后台计算复用。

use bevy::prelude::SystemSet; // 根RenderGraph调度用稳定集合表达跨功能域的GPU命令顺序。

/// 根RenderGraph中相机前的一次性植被GPU命令阶段。
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, SystemSet)]
pub(crate) enum VegetationRenderGraphSet {
    CandidateMip,       // 加载或热更新时先生成候选密度完整Mip链并录入回读复制。
    BladeCompute,       // 随后分类草窗、压缩实例并发布间接绘制参数。
    DiagnosticReadback, // 最后复制少量诊断状态，相机才可消费同帧间接参数。
}

pub use components::{
    GpuBladeClipmapStitches,clipmap_stitch_shader_source,
    GPU_BLADE_CLIPMAP_MAX_BANDS,
    GpuBladeClipmapBand,
    GpuBladeClipmapHeight, // 分页地形的共享高度与可变环带快照。
    GpuBladeDensityMap,
    GpuBladeSourceRevision,
    GpuBladeVolumeSource,
    GpuBladeHeightMap,
    GpuBladeLodDebugDisabled,
    GpuBladePatch,
    GpuBladeProfileBankId,
    GpuBladeProfileBankAllocation, // 多地图/多网格根分别持有绘制槽，退役不清空共享目录。
    GpuBladeView,
}; // 对场景层开放密度、高度、草块参数和玩法相机标记，并允许F3临时隔离单档GPU负载。
pub use constants::{
    GPU_GRASS_DEPTH_BUCKET_COUNT, // 对F3开放每个LOD环带固定的径向间接绘制桶数。
    GPU_GRASS_PATCH_AABB_HORIZONTAL_MARGIN, // 地图和独立示例必须共用能覆盖草簇重映射与顶点形变的水平AABB余量。
};
#[cfg(feature = "diagnostics")] // 公共默认构建不暴露未安装的异步回读配置。
pub use diagnostics::GpuBladeReadbackConfig; // 自动运动烟雾可切换逐帧异步采样，正式默认仍为每四帧。
#[cfg(feature = "diagnostics")]
pub use gpu_compute::pipeline_diagnostic::gpu_blade_compute_pipeline_states;
pub use mesh::ensure_vertical_blade_uv; // 通用库只提供叶片网格准备工具，具体资产路径与预算由消费者维护。
#[cfg(feature = "diagnostics")] // 只有启用诊断 feature 的消费者才能安装 GPU 查询插件。
pub use render::GpuBladeDiagnosticsPlugin;
pub use render::GpuBladePlugin; // 消费者安装一次通用 GPU 草地核心渲染后端。
pub use resources::{
    GPU_GRASS_DEFAULT_WIND_TIP_STRENGTH, // 测试设置默认值与正式渲染默认风摆保持一致。
    GPU_GRASS_INTERACTION_SOURCE_CAPACITY, // 地图足迹采样严格遵守绘制shader固定八槽上限。
    GPU_GRASS_MAX_WIND_TIP_STRENGTH,     // F1风力滑块使用渲染端同一安全上限。
    GpuBladeInteraction,                 // 对地图层和F3开放一份全局风场与固定容量足迹uniform。
    GpuBladeInteractionSource,           // 对地图层开放米制世界空间局部压草源。
    GpuBladeProfile, // 对内容层开放固定叶片profile结构，便于测试面板或后续地图显式覆盖。
    GpuBladeProfileBank, // 一个绘制绑定组内的四个局部外观和共享纹理页。
    GpuBladeProfileBanks, // 数据驱动内容可登记任意数量四槽绘制批次。
    GpuBladeProfiles, // 兼容场景继续使用的批次零固定四槽外观表。
    GpuBladeRenderAssets, // 消费者可配置共享综合色图集，None时使用内置白色回退。
    GpuBladeRenderStats, // 对F3开放上一完整CPU渲染流水线快照。
    GpuBladeStreamingPhase, // 对地图层与F3开放阶段5驻留生命周期枚举。
    GpuBladeStreamingStats, // 对地图层与F3开放预测、淡入和资产驻留快照。
};
#[cfg(feature = "diagnostics")] // 诊断 feature 统一控制异步样本和分阶段 GPU 查询快照。
pub use resources::{
    GpuBladeGpuReadbackStats, GpuBladeGpuTimingStats, GpuBladeVegetationReadbackStats,
    GpuBladeVegetationTimingStats,
};
pub use types::{
    GPU_GRASS_LOD_CAPACITY, GPU_GRASS_PROFILE_CAPACITY, GpuBladeConfig, GpuBladeProfileSlot,
    GpuBladeUltraLodBudget, GpuBladeViewLodConfig,
}; // 消费地图显式提供内容身份、保守包围、投影LOD和常驻窗口规格。

#[cfg(any(test, feature = "diagnostics"))] // 仅诊断/测试构建暴露拓扑镜像。
pub use gpu_compute::clipmap_validation::sample_clipmap_surface_for_validation; // 场景真实 Mesh oracle 的独立 GPU 算法镜像。
