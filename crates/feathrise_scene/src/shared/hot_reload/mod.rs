//! 全局地图 GLB 热重建功能域，复用 Bevy 原生场景热更新并协调碰撞和玩家物理生命周期。

use feathrise_scene_core::residency::SceneResidencyAppExt;

mod systems; // 收拢资产事件监听、跨帧场景重建和碰撞完成恢复系统。

use bevy::prelude::*; // 引入 App、Plugin 和 Update 调度类型。
use feathrise_scene_core::input::{GamePauseSystemSet, WorldPauseBlockers}; // 场景只发布暂停原因，时间写侧仍在游戏组合根。

use systems::{
    // 导入本功能域注册到 Bevy App 的系统与 observer。
    begin_modified_map_reloads, // 处理全部 Scene 修改事件并安全移除匹配地图的旧碰撞。
    finish_map_reload_collision, // Avian碰撞完成后只清理等待身份，统一暂停门随后自然解除。
    respawn_pending_maps,       // 下一帧基于 Bevy 已更新的场景层级重新插入碰撞配置。
    sync_map_reload_pause_blocker, // 把Pending/Awaiting集合投影到公共世界暂停门。
}; // 完成热重载系统导入。

/// 全局地图热重载插件，只处理显式添加 HotReloadableMapScene 的地图根。
pub(crate) struct MapHotReloadPlugin; // GameScenePlugin统一安装该插件，无需每张地图重复注册系统。

impl Plugin for MapHotReloadPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldPauseBlockers>() // 独立场景测试未安装游戏组合根时仍有稳定公共状态。
            .add_resident_systems(
                Update,
                (
                    begin_modified_map_reloads,
                    respawn_pending_maps,
                    sync_map_reload_pause_blocker,
                )
                    .chain() // 命令边界后立即发布本帧最终Pending/Awaiting集合。
                    .in_set(GamePauseSystemSet::CollectBlockers), // 统一Apply同帧暂停，不允许碰撞空窗推进世界。
            )
            .add_observer(finish_map_reload_collision); // 统一接收所有已登记地图的 Avian 碰撞完成事件。
    } // 结束地图热重载插件构建。
} // 结束 MapHotReloadPlugin 实现。
