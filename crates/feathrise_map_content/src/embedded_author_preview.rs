//! 原作者已发布局部结果的纯值合同；不传输入、ECS地址、GPU句柄或历史。

use crate::{
    MapId, MapInstanceId, MapInstanceTransform, MapPlacementAssetTarget, MapWaterAsset,
    TerrainId, TerrainObjectTransform,
    embedded_snapshot::SparseHeightSnapshotTile,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

pub const AUTHOR_PREVIEW_FORMAT: &str = "feathrise-author-preview-v1";
pub const MAX_AUTHOR_PREVIEW_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshPreview {
    pub placement_id: String,
    pub target: MapPlacementAssetTarget,
    pub local: [f32; 16],
}

#[derive(Serialize, Deserialize)]
pub enum AuthorPreview {
    /// 游戏领域自有作者扩展；SDK只验证归属与预算，领域自行完成真实显示后回执。
    Extension { map_id:MapId, revision:u64, domain:String, payload:Arc<Vec<u8>> },
    /// 整体引用沿原实例图预览路径；None 是撤回本桥信用，不能重放旧作者姿态。
    SubMapPose {
        map_id: MapId,
        revision: u64,
        id: MapInstanceId,
        pose: Option<MapInstanceTransform>,
        active: bool,
    },
    TerrainPose {
        map_id: MapId,
        revision: u64,
        id: TerrainId,
        pose: Option<TerrainObjectTransform>,
        active: bool,
    },
    Mesh {
        map_id: MapId,
        revision: u64,
        value: Option<MeshPreview>,
        committed: bool,
    },
    Height {
        map_id: MapId,
        terrain_id: Option<TerrainId>,
        revision: u64,
        reset: bool,
        tiles: Vec<SparseHeightSnapshotTile>,
    },
    Water {
        map_id: MapId,
        terrain_id: Option<TerrainId>,
        root: bool,
        revision: u64,
        definition: Option<MapWaterAsset>,
        masks: BTreeMap<String, Arc<[u8]>>,
    },
}

impl AuthorPreview {
    pub fn map_id(&self) -> &MapId {
        match self {
            Self::Extension {map_id,..} => map_id,
            Self::SubMapPose { map_id, .. }
            | Self::TerrainPose { map_id, .. }
            | Self::Mesh { map_id, .. }
            | Self::Height { map_id, .. }
            | Self::Water { map_id, .. } => map_id,
        }
    }
}

pub fn encode(value: &AuthorPreview) -> Result<Vec<u8>, String> {
    validate_pose(value)?;
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_AUTHOR_PREVIEW_BYTES {
        return Err("作者局部预览超过64MiB传输预算".into());
    }
    Ok(bytes)
}

pub fn decode(bytes: &[u8]) -> Result<AuthorPreview, String> {
    if bytes.len() > MAX_AUTHOR_PREVIEW_BYTES {
        return Err("作者局部预览超过64MiB传输预算".into());
    }
    let value = serde_json::from_slice(bytes).map_err(|error| format!("作者局部预览格式无效：{error}"))?;
    validate_pose(&value)?;
    Ok(value)
}

fn validate_pose(value: &AuthorPreview) -> Result<(), String> {
    match value {
        AuthorPreview::SubMapPose { map_id, revision, id, pose, active } => {
            MapId::parse(map_id.as_str())?;
            MapInstanceId::parse(id.as_str())?;
            if *revision == 0 || (*active && pose.is_none()) { return Err("子地图预览缺少有效版本或当前姿态".into()); }
            if let Some(pose) = pose { pose.validate()?; }
        }
        AuthorPreview::TerrainPose { map_id, revision, id, pose, active } => {
            MapId::parse(map_id.as_str())?;
            TerrainId::parse(id.as_str())?;
            if *revision == 0 || (*active && pose.is_none()) { return Err("地形预览缺少有效版本或当前姿态".into()); }
            if let Some(pose) = pose { pose.validate()?; }
        }
        _ => {}
    }
    Ok(())
}
