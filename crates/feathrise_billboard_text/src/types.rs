// types.rs — Billboard离屏烘焙使用的非ECS运行时数据类型。

use bevy::prelude::*; // Entity用于记录固定相机与当前任务。

/// 一台固定离屏相机及其长期复用的渲染隔离层。
#[derive(Debug, Clone, Copy)] // 槽位只保存轻量实体标识和层编号。
pub(crate) struct BillboardBakeSlot {
    pub(crate) camera_entity: Entity, // 插件启动时创建、整个运行期复用的Camera2d。
    pub(crate) layer: usize,          // 该相机及当前2D源实体共同使用的RenderLayers编号。
    pub(crate) billboard: Option<Entity>, // Some表示槽位正为该Billboard烘焙，None表示空闲。
}
