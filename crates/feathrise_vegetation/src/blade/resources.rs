//! GPU 草地跨主世界与渲染世界共享的资源入口。 // 对外维持稳定路径，具体职责下沉到独立子模块。

use bevy::prelude::{Handle, Image, Resource};
use bevy::render::extract_resource::ExtractResource;
use bevy::render::render_resource::{
    BindGroupLayoutDescriptor, // 延迟到真实RenderDevice上解析的计算绑定组布局描述。
    // 保存计算管线缓存句柄和声明式绑定组布局。
    CachedComputePipelineId, // Bevy异步编译后的计算管线缓存编号。
};

use crate::types::GpuBladeComputeJob; // RenderGraph逐帧消费准备系统生成的草窗计算作业。

mod diagnostics;
mod frame_counters;
mod interaction;
mod profiles;

#[cfg(feature = "diagnostics")]
pub(crate) use diagnostics::GpuBladeTimingCompatibility; // 在 RenderStartup 前传递 Bevy 全局渲染诊断是否独占管线统计。
pub(crate) use diagnostics::GpuBladeTimingQueries;
#[cfg(feature = "diagnostics")]
pub use diagnostics::{
    GpuBladeGpuReadbackStats, GpuBladeGpuTimingStats, GpuBladeVegetationReadbackStats,
    GpuBladeVegetationTimingStats,
};
pub use diagnostics::{GpuBladeStreamingPhase, GpuBladeStreamingStats};
pub(crate) use frame_counters::GpuBladeRenderFrameCounters;
pub use frame_counters::GpuBladeRenderStats;
pub use interaction::{
    GPU_GRASS_DEFAULT_WIND_TIP_STRENGTH, GPU_GRASS_INTERACTION_SOURCE_CAPACITY,
    GPU_GRASS_MAX_WIND_TIP_STRENGTH, GpuBladeInteraction, GpuBladeInteractionSource,
};
pub use profiles::{GpuBladeProfile, GpuBladeProfileBank, GpuBladeProfileBanks, GpuBladeProfiles}; // 四槽兼容表与任意数量动态批次同时保持公开。

/// 消费者提供给草叶绘制管线的共享资产。
///
/// 【功能设计】`None` 明确表示使用 Bevy 的一乘一白色回退纹理；`Some` 表示消费者要求
/// 使用指定综合色图集，此时图片尚未上传 GPU 时安全跳过绘制，禁止先闪现错误纹理。
#[derive(Resource, ExtractResource, Clone, Debug, Default, PartialEq)]
#[extract_app(bevy::render::RenderApp)]
pub struct GpuBladeRenderAssets {
    pub blade_base_color: Option<Handle<Image>>, // 四个 profile 槽共享一张可由 atlas_uv 切片的综合色纹理。
}

impl GpuBladeRenderAssets {
    /// 使用消费者已经加载的综合色纹理构造草地渲染资产配置。
    pub fn with_blade_base_color(blade_base_color: Handle<Image>) -> Self {
        Self {
            blade_base_color: Some(blade_base_color), // 强句柄保证消费者配置存在期间图片保持可达。
        }
    }

    /// 切换共享综合色纹理；传入 `None` 可恢复无外部资产的白色回退模式。
    pub fn set_blade_base_color(&mut self, blade_base_color: Option<Handle<Image>>) {
        self.blade_base_color = blade_base_color; // RenderApp 下一次提取后会重建绑定组并安全等待新图片。
    }
}

/// GPU草地分类、压缩与间接参数收尾管线。
#[derive(Resource)]
pub(crate) struct GpuBladeComputePipeline {
    pub(crate) bind_group_layout: BindGroupLayoutDescriptor, // uniform、两张地表纹理、实例输出和状态缓冲的统一group0布局。
    pub(crate) cull_tiles_pipeline: CachedComputePipelineId, // 每瓦片一次执行保守距离环带与玩法视锥粗剔除。
    pub(crate) finalize_tiles_pipeline: CachedComputePipelineId, // 把GPU活跃瓦片计数写成候选阶段间接派发参数。
    pub(crate) classify_pipeline: CachedComputePipelineId, // 每候选一次执行距离、密度、高度和组内压缩。
    pub(crate) finalize_pipeline: CachedComputePipelineId, // 每草窗一个线程钳制间接实例数并记录溢出。
}

/// 当前渲染帧已经准备完成的草窗计算作业列表。
#[derive(Resource, Default)]
pub(crate) struct GpuBladeComputeJobs {
    pub(crate) jobs: Vec<GpuBladeComputeJob>, // 通常只有近中远三项，节点只遍历不再查询或分配ECS数据。
}

#[cfg(test)]
// 资源实现已经接近职责上限，测试单独放入同crate文件避免生产模块继续膨胀。
#[path = "resources_tests.rs"]
mod tests; // 编译时仍作为resources私有子模块，可直接验证内部uniform字段与原子计数器。
