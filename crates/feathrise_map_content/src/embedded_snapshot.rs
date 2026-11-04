//! 工程运行端的作者快照合同；只包含正式地图源，不传递作者 ECS、撤销历史或 GPU 句柄。

use crate::*;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

pub const SCENE_SNAPSHOT_FORMAT: &str = "feathrise-scene-v1";

/// 一次完整作者输入。大字节源使用 Arc，捕获方必须在后台序列化，不能在帧内深拷贝地图。
#[derive(Serialize, Deserialize)]
pub struct SceneSnapshot {
    pub entry: MapCatalogEntry,
    pub manifest: MapManifestAsset,
    pub placements: MapPlacementsAsset,
    pub portals: MapPortalsAsset,
    pub boundaries: Option<MapBoundariesAsset>,
    pub surface: Option<MapSurfaceAsset>,
    pub vegetation: Option<MapVegetationAsset>,
    pub water: Option<MapWaterAsset>,
    /// 当前作者画布的稳定对象身份；没有独立地形时为 None。
    pub terrain_id: Option<TerrainId>,
    #[serde(default)]
    pub vegetation_densities: BTreeMap<String, Arc<Vec<u8>>>,
    #[serde(default)]
    pub water_masks: BTreeMap<String, Arc<Vec<u8>>>,
    /// 完整未保存高度页；页内容沿用正式编码和验证函数，缺席表示当前无高度覆盖。
    #[serde(default)]
    pub sparse_height_tiles: Vec<SparseHeightSnapshotTile>,
    /// 作者高度时钟与接收World的来源票据不同；Some(0)是合法初始修订，None表示没有作者来源。
    #[serde(default)]
    pub authoring_height_revision: Option<u64>,
    /// 多个独立画布分别绑定稳定 TerrainId，不能把当前检查器的高度覆盖用于兄弟对象。
    #[serde(default)]
    pub height_overrides: Vec<TerrainHeightSnapshot>,
    /// 复杂地表/GLB等复用生产 loader 的内存源；键是当前工程默认资产根内的相对路径。
    /// 文件仅驻留内存，切换试玩绝不触发作者保存或改写原文件。
    #[serde(default)]
    pub asset_overrides: BTreeMap<String, Arc<Vec<u8>>>,
}

#[derive(Serialize, Deserialize)]
pub struct SparseHeightSnapshotTile {
    pub key: SparseHeightTileKey,
    pub bytes: Arc<Vec<u8>>,
}

#[derive(Serialize, Deserialize)]
pub struct TerrainHeightSnapshot {
    pub terrain_id: TerrainId,
    /// 即使当前没有修改页，也必须携带作者时钟，才能为笔刷绑定同一完整地表。
    #[serde(default)]
    pub authoring_height_revision: Option<u64>,
    pub tiles: Vec<SparseHeightSnapshotTile>,
}
