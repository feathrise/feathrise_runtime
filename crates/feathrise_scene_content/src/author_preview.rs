//! 单个活动作者文档的临时运行时输入；磁盘资产句柄和目录权威保持独立。

use crate::{SceneBoundariesAsset, SceneId, ScenePlacementsAsset, ScenePortalsAsset};
use bevy::prelude::*;

/// 【功能设计】切回未保存场景时，正式加载链使用完整的内存快照重新建立世界。
/// 非活动标签只持有CPU作者源；保存成功或关闭标签后撤销这些临时句柄。
#[derive(Clone)]
pub struct SceneAuthorPreviewHandles {
    pub scene_id: SceneId,
    pub placements: Handle<ScenePlacementsAsset>,
    pub portals: Handle<ScenePortalsAsset>,
    pub boundaries: Option<Handle<SceneBoundariesAsset>>,
}

#[derive(Resource, Default)]
pub struct SceneAuthorPreview {
    pub active: Option<SceneAuthorPreviewHandles>,
}

impl SceneAuthorPreview {
    pub fn for_scene(&self, map: &SceneId) -> Option<&SceneAuthorPreviewHandles> {
        self.active.as_ref().filter(|input| &input.scene_id == map)
    }
}
