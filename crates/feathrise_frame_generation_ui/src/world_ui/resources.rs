//! 世界 UI 只保留固定上限的精确实体映射，避免每帧遍历完整透明场景。
use bevy::{prelude::*, render::sync_world::MainEntity}; // 队列查找使用真实 render/main 成对身份。

pub const MAX_WORLD_UI_ITEMS: usize = 4096; // 超出固定预算时整帧保留普通透明绘制，绝不丢弃 UI。
pub const MAX_WORLD_UI_BYTES: u64 = 128 * 1024 * 1024; // RGBA16F颜色与Depth32Float输出遮挡纹理合计的固定显存上限。

#[derive(Resource, Default)] // 主世界提取与渲染队列共享本帧有限清单。
pub struct WorldUiEntities {
    // 不按材质类型猜测水、粒子或其他透明效果。
    pub items: Vec<(Entity, MainEntity)>, // 只包含显式 FrameGenerationUi 实体，例如伤害数字、头顶文字和血条。
    pub over_budget: bool,                // 实体过多时不进行任何部分分流。
} // WorldUiEntities 结束。

#[derive(Resource, Default)] // Prepare 不能把仍留在场景中的标签误报为已经分离。
pub struct WorldUiFrameGate {
    // 只记录当前唯一主视图的固定大小事实。
    pub frame: u32,             // 消费方必须匹配同帧。
    pub camera: Option<Entity>, // 消费方必须匹配唯一主世界相机。
    pub required: bool,         // 主透明 phase 中确实有指定 UI，或者标记数量超过安全预算。
    pub split: bool,            // 只有所有检查完成且全部指定项已转移才为真。
} // WorldUiFrameGate 结束。

#[derive(Resource)] // 单个主视图复用一条深度映射管线与参数缓冲。
pub(super) struct WorldUiDepth {
    // 输出像素必须读取内部viewport的对应深度，不能直接借用附件坐标。
    pub layout: bevy::render::render_resource::BindGroupLayoutDescriptor, // 深度源和坐标映射参数。
    pub pipeline: bevy::render::render_resource::CachedRenderPipelineId,  // 异步编译，不等待GPU。
    pub parameters: bevy::render::render_resource::UniformBuffer<Vec4>, // xy缩放，zw为内部像素抖动位移。
    pub binding: Option<(
        bevy::render::render_resource::TextureViewId,
        bevy::render::render_resource::BindGroup,
    )>, // 仅源附件变化时重建绑定。
} // WorldUiDepth结束。

#[derive(Resource, Default)] // 最多256种源管线，避免热更新无限积累自定义派生管线。
pub(super) struct WorldUiPipelines(
    pub  std::collections::HashMap<
        bevy::render::render_resource::CachedRenderPipelineId,
        bevy::render::render_resource::CachedRenderPipelineId,
    >,
); // 原管线到无抖动独立管线的映射。
