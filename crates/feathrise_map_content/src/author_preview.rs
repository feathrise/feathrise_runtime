//! 单个活动作者文档的临时运行时输入；磁盘资产句柄和目录权威保持独立。

use crate::{MapBoundariesAsset, MapId, MapPlacementsAsset, MapPortalsAsset};
use bevy::prelude::*;

/// 【功能设计】切回未保存地图时，正式加载链使用完整的内存快照重新建立世界。
/// 非活动标签只持有CPU作者源；保存成功或关闭标签后撤销这些临时句柄。
#[derive(Clone)]
pub struct MapAuthorPreviewHandles {
    pub map_id: MapId,
    pub placements: Handle<MapPlacementsAsset>,
    pub portals: Handle<MapPortalsAsset>,
    pub boundaries: Option<Handle<MapBoundariesAsset>>,
}

#[derive(Resource, Default)]
pub struct MapAuthorPreview {
    pub active: Option<MapAuthorPreviewHandles>,
}

impl MapAuthorPreview {
    pub fn for_map(&self, map: &MapId) -> Option<&MapAuthorPreviewHandles> {
        self.active.as_ref().filter(|input| &input.map_id == map)
    }
}
