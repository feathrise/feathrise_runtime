//! 驻留事务的公共活动门；不携带游戏地图、玩法状态或资源生命周期策略。

use crate::residency::ResidentMapToken;
use bevy::prelude::*;

#[derive(Resource, Default)]
pub struct SceneResidencyActivity {
    pub enabled: bool,
    pub switching: bool,
    pub active: Option<ResidentMapToken>,
    /// 只在完整实体与资源恢复后发布；作者缓存以此决定能否继续使用原Entity与修订。
    pub restored: Option<ResidentMapToken>,
}

#[derive(SystemSet, Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct SceneResidencyRuntimeSet;
