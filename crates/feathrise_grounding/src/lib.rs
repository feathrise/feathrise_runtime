//! 通用地面吸附：按声明偏移和源实例过滤执行 Avian 射线；实体分类与场景就绪资格由安装方适配。

use bevy::prelude::*; // 引入 App、Plugin、Update 和系统集派生宏。

mod components; // ECS 组件层：地面偏移、待解算状态和最后高度。
mod constants; // 常量层：统一世界射线起点与检测距离。
mod systems; // 系统层：批量排除动态实体并向下查找真实地表。
mod types; // 实例限定只包含中性实体身份，不借用角色来源或游戏状态。

pub use components::{GroundSnap, GroundSnapFilter}; // 生成请求与调用方显式发布的射线输入。
pub use types::GroundSnapSource;
pub use systems::{ActiveGroundColliders, GroundSurfaceFilter}; // 共享隐藏批次过滤，射线不能把禁用刚体的子Collider当成地面。

/// 地面吸附调度集，供游戏聚合层约束其在行为移动前执行。
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)] // 可在跨 crate 插件聚合层安全配置先后顺序。
pub enum GroundingSet {
    Prepare, // 应用先更新生成资格、查询掩码和来源限定；命令在Snap前完整采用。
    Snap, // 在 AI/玩家移动意图之前完成生成位置的地面高度修正。
} // 结束地面吸附系统集定义。

/// 注册全局一次性地面吸附系统。
pub struct GroundingPlugin; // 无资源配置的零大小插件类型。

impl Plugin for GroundingPlugin {
    // 把地面吸附功能接入 Bevy App 调度。
    fn build(&self, app: &mut App) {
        // 叶子能力不假定任何GameScene；安装方必须按自己的碰撞就绪状态配置Snap运行条件。
        app.configure_sets(Update, (GroundingSet::Prepare, GroundingSet::Snap).chain());
        app.add_systems(
            // 将统一射线系统加入主 Update 调度。
            Update, // 生成实体通过延迟 Commands 出现后，最迟下一帧完成吸附。
            systems::snap_pending_entities_to_ground // 执行所有待处理实体的一次性射线。
                .in_set(GroundingSet::Snap), // 应用在这一阶段统一施加场景资格，不把游戏状态带回叶子包。
        ); // 完成地面吸附系统注册。
    } // 结束插件构建。
} // 结束 GroundingPlugin 实现。
