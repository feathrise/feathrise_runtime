//! 唯一时间写侧只消费应用许可与作者/碰撞暂停事实，不认识背包、交易或死亡。

use avian3d::{prelude::Physics, schedule::PhysicsTime};
use bevy::prelude::*;
use feathrise_scene_core::input::{
    EditorInputState, GameCursorSystemSet, GamePauseSystemSet, InputGateState, WorldPauseBlockers,
};

pub struct WorldInteractionPlugin;
impl Plugin for WorldInteractionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InputGateState>()
            .init_resource::<EditorInputState>()
            .init_resource::<WorldPauseBlockers>()
            .configure_sets(Update, (
                GamePauseSystemSet::CollectBlockers,
                GamePauseSystemSet::PublishGateFacts,
                GamePauseSystemSet::Apply,
                GameCursorSystemSet::Apply,
            ).chain())
            .add_systems(Update, apply_simulation_pause
                .in_set(GamePauseSystemSet::Apply)
                .run_if(feathrise_scene_core::state::in_active_scene()));
    }
}

fn apply_simulation_pause(
    input: Res<InputGateState>,
    editor: Res<EditorInputState>,
    blockers: Res<WorldPauseBlockers>,
    mut physics: ResMut<Time<Physics>>,
    mut virtual_time: ResMut<Time<Virtual>>,
) {
    // 输入许可与模拟时间是不同事实；对话禁输入但不暂停的原应用语义由发布者决定。
    if input.time_paused || editor.scene_editor_open || blockers.blocks_world() {
        physics.pause();
        virtual_time.pause();
    } else {
        physics.unpause();
        virtual_time.unpause();
    }
}

#[cfg(test)]
#[path = "interaction_tests.rs"]
mod tests;
