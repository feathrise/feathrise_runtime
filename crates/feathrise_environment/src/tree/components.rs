//! 树木运行时 ECS 组件。
//! 本文件只定义树根、LOD 渲染节点和近处碰撞代理的数据身份。
//! 【模块所有权】组件归树木子域所有，不向统一环境crate根命名空间平铺。

use bevy::prelude::*; // 引入 Component 与 Entity 等 ECS 基础类型。

use super::types::{TreeLodLevel, TreeMeshPart}; // LOD 与树干/树冠身份由非 ECS 值类型统一声明。

/// 单棵世界稳定树木的根实体。
#[derive(Component, Clone, Copy, Debug, PartialEq)] // 根组件允许诊断读取稳定编号和当前碰撞代理。
pub struct TreeInstance {
    pub id: u16, // 地图内容表中的稳定编号，不随进出地图或相机移动变化。
    pub collision_proxy: Option<Entity>, // 只保存当前近处碰撞子实体，不复制渲染实例数据。
} // 完成树根运行状态定义。

/// 单个树木 LOD 的树干、树冠或公告板渲染节点。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash)] // F3 可按 LOD 和部件统计当前可见节点。
pub struct TreeLodMesh {
    pub level: TreeLodLevel, // 指明高、中、低或公告板档。
    pub part: TreeMeshPart,  // 指明当前节点承载树干、树冠或完整公告板。
} // 完成树木渲染节点身份定义。

/// 玩家附近才存在的树干碰撞代理。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)] // 代理可由距离系统安全创建、回收并供F3计数。
pub struct TreeCollisionProxy {
    pub owner: Entity, // 记录所属树根，便于诊断和异常孤儿清理。
} // 完成局部碰撞代理身份定义。
