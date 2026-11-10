//! F3 植物与景观物四档 LOD 性能隔离功能域。
//! 【功能设计】调试选择只在本次进程内生效；关闭档位不扩大其他档位范围，重启游戏恢复全部开启。

mod apply; // 把九类内容的调试状态同步到GPU提取标记和Bevy可见性。
mod mesh; // 接管后按各自诊断层与原型把树木/灌木/岩石开关同步到通用Mesh托管节点。
mod preset_bindings; // 工程显式提供现有诊断页的预设身份。
mod targets; // 只缓存九槽数值身份，资源移除后仍可恢复旧节点。
pub use preset_bindings::LodControlPresetBindings;
mod state; // 保存九类内容互相独立的非持久化四档状态。
mod ui; // 创建并驱动可读的档位开关、单独查看和全部开启按钮。
pub(super) use state::content_for_page;

/// 作者工具在公共诊断恢复旧目标之后采用自己的当前层；同帧不互相覆盖可见性。
#[derive(bevy::prelude::SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LodControlApplySet;

pub(super) use apply::{apply_changed_lod_controls, apply_lod_controls_to_new_nodes}; // 调度层只调用全量变化与新增节点两条同步路径。
pub(super) use mesh::install_managed_mesh_lod_controls; // 通用节点通过独立PostUpdate安装边界保证首次生成帧即应用F3状态。
pub use state::{
    // F4动态场景监控复用同一状态，避免两个面板对同一渲染节点互相覆盖。
    ALL_LODS_ENABLED,
    LOD_LEVEL_COUNT,
    LodControlContent,
    LodControlState,
}; // 完成跨监控面板共享的最小公开调试接口。
#[cfg(test)] // 根标记只供外层无窗口UI结构测试直接查询。
pub(super) use ui::ResourceMonitorLodControls; // 验证总览页让完整LOD控件退出布局。
pub(super) use ui::{handle_lod_control_interactions, spawn_lod_controls, update_lod_control_ui}; // 面板创建、输入和视觉同步通过单一功能域入口暴露给兄弟模块。
