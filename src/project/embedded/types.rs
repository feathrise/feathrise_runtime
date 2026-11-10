//! 常驻工程 World 的模式事实和项目玩法运行条件。

pub use feathrise_scene_content::embedded_snapshot::*;

/// 工程插件可以用此资源或 project_simulation_active 条件安排自己的玩法系统。
/// 同一进程只创建一次 World；编辑/试玩切换只交接时间、输入和相机。
#[derive(bevy::prelude::Resource, Clone, Copy, Debug)]
pub struct EmbeddedProjectState {
    pub mode: feathrise_editor_protocol::Mode,
    pub world_id: u64,
    pub revision: u64,
    /// SDK在原生窗口交接开始时关闭运行许可，实际成功或旧承载恢复后才清除；mode仍保留真实已确认事实。
    pub transitioning: bool,
}
impl EmbeddedProjectState {
    /// 领域作者显示只在真实编辑态揭示；工程插件不必依赖内部IPC协议类型。
    pub fn editing(&self) -> bool {
        self.mode == feathrise_editor_protocol::Mode::Editing
    }
}

/// 普通独立游戏默认允许运行；编辑与原生交接期间均禁止主动玩法，直接使用此helper的工程也遵守同一门。
pub fn project_simulation_active(state: Option<bevy::prelude::Res<EmbeddedProjectState>>) -> bool {
    state.is_none_or(|state| state.mode == feathrise_editor_protocol::Mode::Playing && !state.transitioning)
}
