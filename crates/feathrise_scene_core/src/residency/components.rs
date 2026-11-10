//! 驻留实体的稳定场景代次；关闭后重开使用新代次，迟到对象不能归入新场景。

use bevy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResidentSceneToken(pub u64);

/// 对象仍留在主世界；活动资格由Disabled隔离，场景身份不依赖场景枚举是否相同。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
#[component(storage = "SparseSet")]
pub struct ResidentSceneEntity(pub ResidentSceneToken);

/// 暂停前同步跨场景会话数据；此时源场景尚未加入Disabled。
#[derive(bevy::prelude::Event)]
pub struct ResidentSceneParking(pub ResidentSceneToken);

/// 暖恢复完成后的轻量交接，供玩家落点和全局输入继续使用原有功能。
#[derive(bevy::prelude::Event)]
pub struct ResidentSceneActivated(pub ResidentSceneToken);
