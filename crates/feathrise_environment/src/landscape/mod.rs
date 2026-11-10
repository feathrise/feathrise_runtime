//! 固定景观技术底座：稳定落点、共享四档LOD、地面吸附、近处碰撞身份与F3诊断资源。
//! 【功能设计】本crate不读取密度图、不运行随机散布、不创建逐帧上传队列；具体资产与清单归单个场景。
//! 【模块所有权】P5.5将实现内收至统一环境入口，公开`environment::landscape`路径与行为保持不变。

mod components; // 景观根、LOD节点与局部碰撞代理ECS身份。
mod resources; // 当前场景灌木和岩石固定预算与资产有效载荷。
mod spawn; // 共享Mesh/Material句柄的单实例四档生成helper。
mod types; // 类别、LOD原型和稳定落点纯值类型。

use bevy::prelude::*; // App、Plugin与资源初始化接口。

pub use components::{LandscapeCollisionProxy, LandscapeInstance, LandscapeLodMesh}; // F3和场景碰撞系统读取公共身份。
pub use resources::{LandscapeKindRuntimeStats, LandscapeRuntimeStats}; // 场景写入固定预算，F3读取轻量快照。
pub use spawn::spawn_landscape_instance; // 场景OnEnter系统复用统一四档生成逻辑。
pub use types::{
    LandscapeKind, LandscapeLodLevel, LandscapeLodPrototype, LandscapePlacement, LandscapePrototype,
}; // 场景装配层取得稳定落点、四档共享资产和类别值。

/// 固定景观公共插件：注册跨场景诊断资源。
pub struct LandscapePlugin; // 无配置字段，具体灌木、岩石资产与落点全部属于单个场景。

impl Plugin for LandscapePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LandscapeRuntimeStats>(); // 非HN-01场景保留全零快照，进入场景后由内容层覆盖。
    } // 完成公共固定景观资源注册。
} // 完成固定景观插件实现。
