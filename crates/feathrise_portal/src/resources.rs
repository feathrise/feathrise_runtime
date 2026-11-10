// 传送门 — 资源定义

use bevy::prelude::*; // Resource 等 ECS 常用类型

/// 所有传送点共享的固定视觉资产；场景热更新只创建实体和按作者颜色生成轻量材质。
#[derive(Resource, Clone)]
pub struct PortalVisualMeshes {
    pub(crate) base: Handle<Mesh>,       // 深色地面圆盘沿用原传送范围半径。
    pub(crate) arrow_head: Handle<Mesh>, // 三角箭头头部共享同一二维网格。
    pub(crate) arrow_shaft: Handle<Mesh>, // 矩形箭头杆身共享同一二维网格。
    pub(crate) edge_ring: Handle<Mesh>,  // 地面边缘发光环在草根之间标出完整轮廓。
    pub(crate) edge_beam_wall: Handle<Mesh>, // 无顶盖圆柱只从传送范围边缘垂直发光，中心保持留空。
    pub(crate) label_font: Handle<Font>, // 全部正式标签共享项目中文字体，避免中文名称烘焙成缺字方框。
}

/// 传送目的地资源：应用记录目标坐标和场景身份，公共场景加载消费目标场景。
/// 玩家碰触判定与角色落点解释由游戏适配器拥有，公共库不安装这些策略。
#[derive(Resource, Default, Debug)] // 资源 + 调试 + 默认值支持
pub struct PortalDestination {
    pub spawn_pos: Option<Vec3>, // 目标出生坐标（None 表示使用场景默认位置）
    pub dynamic_scene_id: Option<String>, // DynamicSceneRoot进入前保存目标SceneId，静态场景保持None。
}

/// 应用显式提供标签字体；默认空句柄使用Bevy字体，公共库不读取原游戏字体路径。
#[derive(Resource, Default, Clone)]
pub struct PortalLabelFont(pub Handle<Font>);
