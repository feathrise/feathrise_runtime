//! 驻留实体的稳定地图代次；关闭后重开使用新代次，迟到对象不能归入新场景。

use bevy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResidentMapToken(pub u64);

/// 对象仍留在主世界；活动资格由Disabled隔离，地图身份不依赖场景枚举是否相同。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
#[component(storage = "SparseSet")]
pub struct ResidentMapEntity(pub ResidentMapToken);

/// 暂停前同步跨地图会话数据；此时源地图尚未加入Disabled。
#[derive(bevy::prelude::Event)]
pub struct ResidentSceneParking(pub ResidentMapToken);

/// 暖恢复完成后的轻量交接，供玩家落点和全局输入继续使用原有功能。
#[derive(bevy::prelude::Event)]
pub struct ResidentSceneActivated(pub ResidentMapToken);
