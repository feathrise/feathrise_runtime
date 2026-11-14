// types.rs — Billboard离屏烘焙使用的非ECS运行时数据类型。

use bevy::prelude::*; // Entity用于记录固定相机与当前任务。
use std::sync::{Arc, atomic::AtomicU64}; // 固定槽位以非阻塞回执跨主世界和渲染世界确认提交。

/// 一台固定离屏相机及其长期复用的渲染隔离层。
#[derive(Debug, Clone)] // 回执与RenderApp共享；复用相机时必须同时核对任务代次。
pub(crate) struct BillboardBakeSlot {
    pub(crate) camera_entity: Entity, // 插件启动时创建、整个运行期复用的Camera2d。
    pub(crate) layer: usize,          // 该相机及当前2D源实体共同使用的RenderLayers编号。
    pub(crate) billboard: Option<Entity>, // Some表示槽位正为该Billboard烘焙，None表示空闲。
    pub(crate) generation: u64, // 每次分配递增，零表示尚未分配过任务。
    pub(crate) rendered_generation: Arc<AtomicU64>, // 只有真实绘制和最终输出均提交后才发布对应代次。
}

/// 当前烘焙任务的有界渲染快照；只提取固定四槽，不遍历已完成标签或整场景字形。
pub(crate) struct BillboardBakeRenderJob {
    pub(crate) camera_entity: Entity,
    pub(crate) image: AssetId<Image>,
    pub(crate) generation: u64,
    pub(crate) rendered_generation: Arc<AtomicU64>,
    pub(crate) text_sources: Vec<(Entity, usize)>, // 源实体与预期字形切片数，包含主文字、阴影和描边。
    pub(crate) decorations: Vec<Entity>,
    pub(crate) draw_encoded: bool, // 相机实际执行完整绘制后置位，Render提交后才向主世界回执。
}
