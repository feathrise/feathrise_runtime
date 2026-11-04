mod components;
#[cfg(any(feature = "dlss_frame_generation", feature = "fsr3_frame_generation"))]
// 任一帧生成构建复用真实呈现计时器。
mod helpers; // 固定大小累计呈现计数的采样逻辑。
mod resource_monitor;
mod resources;
mod systems;
#[cfg(any(feature = "dlss_frame_generation", feature = "fsr3_frame_generation"))]
// 默认构建不引入呈现来源类型。
mod types; // DLSS与FSR计数身份独立，交换链换代不跨来源相减。

use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;

pub use resource_monitor::{
    // F4只依赖共享内容身份和非持久化逐档状态，不取得F3私有页面或系统。
    ResourceMonitorApplicationReport, ResourceMonitorRefresh, ResourceMonitorSet, resource_monitor_overview_refresh,
    ALL_LODS_ENABLED,
    LOD_LEVEL_COUNT,
    LodControlContent,
    LodControlState,
    LodControlPresetBindings, LodControlApplySet,
}; // 完成F3/F4共同使用的公开LOD调试API。
pub use resources::FpsDisplayFont; // 字体选择属于消费应用，共享诊断只使用明确提供的句柄。

pub struct FpsDisplayPlugin;

impl Plugin for FpsDisplayPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(FrameTimeDiagnosticsPlugin::default());
        app.init_resource::<FpsDisplayFont>(); // 保留宿主预先提供的字体；不隐式读取原游戏字体目录。
        app.insert_resource(resources::FpsRefreshTimer(Timer::from_seconds(
            0.5,
            TimerMode::Repeating,
        )));
        resource_monitor::install_resource_monitor(app); // 资源面板由自身功能域安装计时器、状态和系统，避免私有查询类型泄漏到crate根。
        app.add_systems(Startup, systems::spawn_fps_text);
        app.add_systems(Last, systems::update_fps_text); // 在PostUpdate发布DLSS/FSR快照后读取，避免采样上一主世界帧的计数。
    }
}
