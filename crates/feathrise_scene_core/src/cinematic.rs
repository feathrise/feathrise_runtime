//! 场景宿主与角色、相机控制器之间的演出接管合同；此层不持有时间轴或游戏进度。

use bevy::prelude::*;

/// 作者分别选择占用相机和玩家操作。它们都不暂停物理、世界时间、对白和跳过按键。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CinematicControl {
    pub camera: bool,
    pub player: bool,
}

/// 由游戏或工程插件给真正的演员根提供稳定身份。player 指向本模型的骨架播放器。
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct CinematicActor {
    pub target: String,
    pub instance: Option<String>,
    pub model: Option<String>,
    pub player: Option<Entity>,
}

/// 对象由演出写位置或朝向时，玩法移动与固定步平台退出该对象的写入。
#[derive(Component)]
pub struct CinematicPoseOwner;

/// 已登记的外部动作走本演员的正式适配。未登记才从身体读取，异步准备必须保留等待语义。
pub enum ActorClip {
    Unregistered,
    Waiting,
    Ready(Handle<AnimationClip>),
}

#[derive(Component, Clone, Copy)]
pub struct CinematicClipProvider(pub fn(&mut World, Entity, &str) -> Result<ActorClip, String>);

/// 只在实际绑定的骨架播放器使用演出剪辑期间抑制该演员本来的动画状态机。
#[derive(Component)]
pub struct CinematicAnimationOwner;

/// 已结算并停在末姿的演员。收到新的移动或施法意图时，玩法可按稳定实体归还原动画控制器。
#[derive(Component)]
pub struct CinematicRestingActor;

#[derive(EntityEvent)]
pub struct ResumeCinematicActor {
    pub entity: Entity,
}
