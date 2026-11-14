//! 显式几何验收在启动时冻结的保存目录；不携带作者样本或运行时可写状态。

use feathrise_scene_content::{SceneId, TerrainId};

#[derive(Default)]
pub(in super::super) struct Requirements {
    pub(super) scene: SceneId,
    pub(super) terrains: Vec<SavedTerrain>,
}

pub(super) struct SavedTerrain {
    pub id: TerrainId,
    pub source_path: String,
    pub layers: Vec<SavedLayer>,
}

pub(super) struct SavedLayer {
    pub id: String,
    pub runtime_id: u64,
}
