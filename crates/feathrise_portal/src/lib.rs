// 传送门 crate (feathrise_portal)
// 提供传送目的地数据、作者标签与固定几何表现。
// 玩家进入、角色会话同步和场景切换策略由应用显式安装。
//
// 【拆分】2026-05-12 抽离为独立 crate。

mod components; // 组件：Portal
mod constants;
mod helpers; // 辅助函数：spawn_portal_entity
mod mesh; // 共享边缘直光幕程序网格与顶点渐隐。
mod resources; // 资源：PortalDestination
mod systems; // 固定几何资产初始化，不安装玩家或角色会话系统。

pub use components::{Portal, PortalLabel}; // 导出传送碰撞与标签锚点身份，供F12相机定向呈现。
pub use constants::{PORTAL_EDGE_BEAM_HEIGHT, PORTAL_VISUAL_RADIUS}; // F12预览与正式实体共享几何规格。
pub use helpers::{
    PortalSpawnSpec, PortalSpawnedEntities, portal_arrow_layout, portal_destination_label,
    portal_display_label, portal_label_billboard, spawn_portal_entity,
}; // 导出正式生成函数、共享标签规格、热更清理实体表与高内聚配置。
pub use mesh::build_portal_edge_beam_mesh; // 编辑器预览复用同一固定半径和顶部渐隐网格实现。
pub use resources::{PortalDestination, PortalLabelFont, PortalVisualMeshes}; // 字体由应用提供，不隐式加载游戏资源。

use bevy::prelude::*; // Plugin 等

pub struct PortalPlugin;

impl Plugin for PortalPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PortalDestination>()
            .init_resource::<PortalLabelFont>()
            .add_systems(Startup, systems::setup_portal_visual_meshes);
    }
}
