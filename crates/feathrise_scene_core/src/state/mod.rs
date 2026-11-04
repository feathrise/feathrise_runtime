//! 公共场景生命周期与完整加载事务；不包含游戏菜单、玩家出生或新局规则。

use bevy::prelude::*;
pub use crate::residency::SceneResidencyActivity;

mod resources;
mod types;

pub use resources::{
    EditorStartup, LoadingProgress, PendingSceneTransition, SceneFallBoundary,
    SceneStartupGate,
};
pub use types::{LoadingStage, LoadingTransactionId, ScenePhase, SceneTransitionPresentation};

/// 运行条件：当前是否处于有效场景（非空闲、非加载）
pub fn in_active_scene() -> impl FnMut(
    Option<Res<State<ScenePhase>>>,
    Option<Res<crate::residency::SceneResidencyActivity>>,
) -> bool
+ Clone {
    move |state: Option<Res<State<ScenePhase>>>,
          residency: Option<Res<crate::residency::SceneResidencyActivity>>| {
        !residency.is_some_and(|state| state.switching)
            && state
                .is_some_and(|s| !matches!(s.get(), ScenePhase::Idle | ScenePhase::Loading))
    }
}

/// 运行条件：当前场景的地形碰撞是否已经可用于生成应用对象。
pub fn scene_collision_ready(
    gate: Option<Res<SceneStartupGate>>,
    state: Option<Res<State<ScenePhase>>>,
    residency: Option<Res<crate::residency::SceneResidencyActivity>>,
) -> bool {
    if residency.is_some_and(|state| state.switching) {
        return false;
    }
    let Some(gate) = gate else {
        return true;
    };
    let Some(state) = state else {
        return true;
    };
    if *state.get() != gate.target {
        return true;
    }
    gate.collision_ready
}

/// 运行条件：当前场景是否已经完成启动，可推进世界模拟。
pub fn scene_startup_ready(
    gate: Option<Res<SceneStartupGate>>,
    state: Option<Res<State<ScenePhase>>>,
    loading: Option<Res<LoadingProgress>>,
    residency: Option<Res<crate::residency::SceneResidencyActivity>>,
) -> bool {
    if residency.is_some_and(|state| state.switching) {
        return false;
    }
    if loading.is_some() {
        return false;
    }

    let Some(gate) = gate else {
        return true;
    };
    let Some(state) = state else {
        return true;
    };
    if *state.get() != gate.target {
        return true;
    }
    gate.ready
}
