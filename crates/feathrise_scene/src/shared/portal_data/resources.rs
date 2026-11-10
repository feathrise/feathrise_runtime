//! 数据驱动传送图私有事务状态与公开诊断。

use bevy::prelude::*; // Entity、Resource与场景枚举。
use feathrise_scene_content::SceneId; // 诊断使用稳定场景ID而非Rust枚举名。
use feathrise_scene_core::state::ScenePhase; // 记录当前实体所属场景，切换场景时拒收旧图结果。

/// 当前场景传送数据所处阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScenePortalRuntimePhase {
    /// 目录或目标场景私有文件仍在异步加载。
    #[default]
    Waiting,
    /// 完整传送图已校验并一次性生成。
    Ready,
    /// 本代数据损坏；继续保留上一份完整实体并等待修复热更。
    Failed,
}

/// 可供F3和F12编辑器显示的传送图只读证据。
#[derive(Resource, Clone, Debug, Default)]
pub struct ScenePortalRuntimeStatus {
    /// 当前阶段。
    pub phase: ScenePortalRuntimePhase,
    /// 当前稳定场景ID；菜单和加载阶段为空。
    pub scene_id: Option<SceneId>,
    /// 最近一次成功原子应用的修订。
    pub revision: u64,
    /// 当前成功生成的逻辑传送点数量。
    pub portal_count: usize,
    /// 等待或失败的明确原因。
    pub detail: String,
}

/// 传送图唯一写侧保存的脏标记和旧实体表。
#[derive(Resource, Debug)]
pub(crate) struct ScenePortalRuntimeState {
    pub(crate) observed_terrain_revision:Option<(SceneId,u64)>,
    pub(crate) observed_preview: Option<(
        SceneId,
        AssetId<feathrise_scene_content::ScenePortalsAsset>,
        Option<AssetId<feathrise_scene_content::SceneBoundariesAsset>>,
    )>,
    /// 任一目录、清单或传送图事件到达后请求重新解析。
    pub(crate) dirty: bool,
    /// 上次尝试处理的玩法场景。
    pub(crate) active_scene: Option<ScenePhase>,
    /// 上次尝试处理的真实场景身份；多个动态场景共用同一场景枚举，不能只比较`ScenePhase`。
    pub(crate) active_scene_id: Option<SceneId>,
    /// 上次观察到的场景目录修订。
    pub(crate) catalog_revision: u64,
    /// 最近一次成功生成的关系根与独立文字锚点集合；热更成功后统一销毁。
    pub(crate) spawned_entities: Vec<Entity>,
}

impl Default for ScenePortalRuntimeState {
    fn default() -> Self {
        Self {
            observed_terrain_revision:None,
            observed_preview: None,
            dirty: true,
            active_scene: None,
            active_scene_id: None,
            catalog_revision: 0,
            spawned_entities: Vec::new(),
        }
    }
}
