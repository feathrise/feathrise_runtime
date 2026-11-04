//! 场景加载执行器：等待应用提交的关键资产，交给既有碰撞、场景和应用启动门。
//! 游戏菜单负责新局与PAK，编辑器和普通工程负责自己的关键资产；此层不选择内容或构建界面。

mod assets;
mod resources;
mod systems;

pub use assets::has_failed_asset;
pub use resources::{CriticalSceneAssets, SceneLoadingPrerequisites};

use bevy::prelude::*;
use feathrise_scene_core::state::ScenePhase;

/// 应用准备先完整提交资产清单，再由唯一执行器推进状态，避免并发系统绕过前置门。
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SceneLoadingSet {
    Initialize,
    Prepare,
    Poll,
    Finish,
}

pub struct SceneLoadingPlugin;
impl Plugin for SceneLoadingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SceneLoadingPrerequisites>()
            .configure_sets(
                Update,
                (SceneLoadingSet::Prepare, SceneLoadingSet::Poll).chain(),
            )
            .add_systems(
                OnEnter(ScenePhase::Loading),
                systems::begin_scene_loading.in_set(SceneLoadingSet::Initialize),
            )
            .add_systems(OnEnter(ScenePhase::Idle), systems::clear_scene_loading)
            .add_systems(
                Update,
                systems::poll_scene_loading
                    .in_set(SceneLoadingSet::Poll)
                    .run_if(in_state(ScenePhase::Loading)),
            )
            // Complete由scene的真实碰撞/资产/应用门产生；Update界面看完最终值再统一回收。
            .add_systems(
                Last,
                systems::finish_scene_loading.in_set(SceneLoadingSet::Finish),
            );
    }
}

/// 无原游戏菜单的工程与作者World显式选装，既不读取旧PAK，也不枚举默认敌人/NPC。
pub struct ProjectSceneAssetsPlugin;
impl Plugin for ProjectSceneAssetsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            systems::prepare_project_scene_assets
                .in_set(SceneLoadingSet::Prepare)
                .run_if(in_state(ScenePhase::Loading)),
        );
    }
}

#[cfg(test)]
mod tests;
