//! 共享输入许可、作者占用与安全事务事实；这里不认识任何具体游戏面板。

use bevy::prelude::*;

/// 应用在收集完本帧输入来源后发布最终许可；共享系统不判断具体业务原因。
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputGateState {
    pub input_allowed: bool,
    pub world_active: bool,
    /// 时间暂停与系统运行许可分开，保留应用对零时间步更新的原有约定。
    pub time_paused: bool,
    pub camera_control_allowed: bool,
    pub editor_entry_allowed: bool,
    /// 活动场景是否允许把焦点交给作者；空工作台不需要玩法焦点。
    pub scene_focus_available: bool,
    pub system_cursor_required: bool,
    /// 存在独占交互或更高优先级提示时，应用可单独占用世界交互键。
    pub interaction_allowed: bool,
}

impl Default for InputGateState {
    fn default() -> Self {
        Self {
            input_allowed: true,
            world_active: true,
            time_paused: false,
            camera_control_allowed: true,
            editor_entry_allowed: true,
            scene_focus_available: true,
            system_cursor_required: false,
            interaction_allowed: true,
        }
    }
}

/// 作者自己的占用事实；由作者模块写入，应用适配不得覆盖作者现场。
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EditorInputState {
    pub map_editor_open: bool,
    /// 只在确实显示跟随笔刷时隐藏系统鼠标；绝不据此锁定指针。
    pub map_editor_custom_cursor_visible: bool,
}

/// 非UI安全事务的暂停原因；生产者登记状态，唯一暂停应用系统负责时间。
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct WorldPauseBlockers {
    /// 地图正在替换碰撞，完成前不得推进模拟或物理。
    pub map_collision_reload: bool,
}

impl WorldPauseBlockers {
    pub fn blocks_world(&self) -> bool {
        self.map_collision_reload
    }
}
