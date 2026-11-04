//! glTF显式遮雨盒生命周期；只读取小extras和变换，不读取任何Mesh顶点。

mod components; // 区分extras派生盒和玩法直接插入的盒。
mod helpers; // 有界JSON作者参数解析。
mod ownership; // placement采用门和场景根解析。
mod resources; // 作者实体记录和地图身份。
mod systems; // 变更、删除及整批发布。
#[cfg(test)] // 直接运行正式ECS桥验证生命周期。
mod tests;
mod types; // 小代理根身份与有界祖先缓存。 // 无碰撞代理、热更新和预算拒绝。

use bevy::prelude::*; // 插件和有序调度。
use bevy::transform::TransformSystems; // 父级移动必须先传播到代理。
use feathrise_environment::rain_shelter::{RainShelterState, RainShelterUpload}; // 共享正式受雨批次。

#[derive(Default)] // 无配置公共场景插件。
pub(crate) struct RainShelterPlugin; // 安装节点作者桥，不接管天气历史。

impl Plugin for RainShelterPlugin {
    // 主世界系统保持小数据变化驱动。
    fn build(&self, app: &mut App) {
        // 环境插件拥有缓冲，场景插件只负责作者来源。
        app.init_resource::<resources::RainShelterRegistry>() // 默认无作者源。
            .init_resource::<RainShelterState>() // 支持场景插件单独进行无窗口测试。
            .add_systems(Update, systems::sync_extras) // extras只在发生真实变更时解析。
            .add_systems(
                PostUpdate,
                systems::publish
                    .after(TransformSystems::Propagate)
                    .before(RainShelterUpload),
            ); // 当前父变换与删除先形成完整批次，再上传。
    } // 完成安装。
} // 完成公共插件。
