//! 固定景观运行时ECS组件。
//! 【功能设计】根实体保存稳定内容身份，四档子节点只保存LOD身份，近处碰撞代理独立驻留。
//! 【模块所有权】组件归固定景观子域所有，不向统一环境crate根命名空间平铺。

use bevy::prelude::*; // Component与Entity等ECS基础类型。

use super::{LandscapeKind, LandscapeLodLevel}; // 公共内容类别与四档身份保持唯一来源。

/// 单个世界稳定灌木或岩石的根实体。
#[derive(Component, Clone, Copy, Debug, PartialEq)] // F3与碰撞系统读取同一轻量运行状态。
pub struct LandscapeInstance {
    pub id: u16,                         // 当前类别内的稳定清单编号。
    pub kind: LandscapeKind,             // 区分灌木与岩石，禁止按名称字符串判断。
    pub collision_proxy: Option<Entity>, // 只保存当前近处简化碰撞实体，不复制渲染数据。
    pub collision_enabled: bool,         // 离线内容决定当前实例是否属于中大型可阻挡岩石。
    pub uniform_scale: f32,              // 碰撞代理按与可见岩石相同的稳定体量缩放。
} // 完成景观根运行状态定义。

/// 单个景观LOD渲染节点。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash)] // F3可按类别和高、中、低、超低统计节点。
pub struct LandscapeLodMesh {
    pub kind: LandscapeKind,      // 继承所属景观类别，查询不必沿父子层级回溯。
    pub level: LandscapeLodLevel, // 指明当前节点属于高、中、低或超低档。
} // 完成景观LOD节点身份定义。

/// 玩家附近才存在的简化景观碰撞代理。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)] // 代理身份用于F3和异常孤儿审计。
pub struct LandscapeCollisionProxy {
    pub owner: Entity,       // 记录所属稳定景观根实体。
    pub kind: LandscapeKind, // 当前实现只允许Rock，字段仍防止未来误混其他类别。
} // 完成景观碰撞代理身份定义。
