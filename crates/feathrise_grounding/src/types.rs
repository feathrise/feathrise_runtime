//! 贴地来源限定；只表达本次射线允许使用的场景实例，不解释角色、场景文件或游戏状态。

use bevy::prelude::Entity;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroundSnapSource {
    /// 无额外来源限定，仍检查实体自身的SceneInstanceOwner和已采用碰撞覆盖。
    #[default]
    Any,
    /// 只允许此场景实例或归属于它的地形实例承托，Entity自带代次防止旧根复用。
    Instance(Entity),
    /// 调用方知道实体属于显式来源，但该来源尚未发布；保留请求，不能借用父图。
    Unavailable,
}
