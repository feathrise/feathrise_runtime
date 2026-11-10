//! 共享输入与模拟门：消费应用许可、作者占用和场景就绪事实，不包含游戏UI规则。

mod resources;
mod types;

pub use resources::*;
pub use types::*;

use bevy::prelude::*;
use crate::state::{ScenePhase, LoadingProgress, SceneStartupGate, scene_startup_ready};

/// 玩家主动输入门；场景启动、驻留切换和碰撞空窗均保持原有安全门。
pub fn in_game_mode(
    gate: Res<InputGateState>,
    editor: Option<Res<EditorInputState>>,
    world_pause: Option<Res<WorldPauseBlockers>>,
    startup_gate: Option<Res<SceneStartupGate>>,
    scene_state: Option<Res<State<ScenePhase>>>,
    loading_progress: Option<Res<LoadingProgress>>,
    residency: Option<Res<crate::state::SceneResidencyActivity>>,
) -> bool {
    gate.input_allowed
        && !editor.is_some_and(|editor| editor.scene_editor_open)
        && !world_pause.is_some_and(|blocks| blocks.blocks_world())
        && scene_startup_ready(startup_gate, scene_state, loading_progress, residency)
}

/// 世界系统运行门，与时间是否暂停分开；应用显式决定零时间步系统是否仍可运行。
pub fn world_active(
    gate: Res<InputGateState>,
    editor: Option<Res<EditorInputState>>,
    world_pause: Option<Res<WorldPauseBlockers>>,
    startup_gate: Option<Res<SceneStartupGate>>,
    scene_state: Option<Res<State<ScenePhase>>>,
    loading_progress: Option<Res<LoadingProgress>>,
    residency: Option<Res<crate::state::SceneResidencyActivity>>,
) -> bool {
    gate.world_active
        && !editor.is_some_and(|editor| editor.scene_editor_open)
        && !world_pause.is_some_and(|blocks| blocks.blocks_world())
        && scene_startup_ready(startup_gate, scene_state, loading_progress, residency)
}

#[cfg(test)]
mod tests;
