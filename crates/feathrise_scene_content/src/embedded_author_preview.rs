//! 原作者已发布局部结果的纯值合同；不传输入、ECS地址、GPU句柄或历史。

use crate::{
    SceneId, SceneInstanceId, SceneInstanceTransform, ScenePlacementAssetTarget, SceneWaterAsset,
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
    pub target: ScenePlacementAssetTarget,
    pub local: [f32; 16],
}

#[derive(Serialize, Deserialize)]
pub enum AuthorPreview {
    /// 游戏领域自有作者扩展；SDK只验证归属与预算，领域自行完成真实显示后回执。
    Extension { scene_id:SceneId, revision:u64, domain:String, payload:Arc<Vec<u8>> },
    /// 整体引用沿原实例图预览路径；None 是撤回本桥信用，不能重放旧作者姿态。
    SubscenePose {
        scene_id: SceneId,
        revision: u64,
        id: SceneInstanceId,
        pose: Option<SceneInstanceTransform>,
        active: bool,
    },
    TerrainPose {
        scene_id: SceneId,
        revision: u64,
        id: TerrainId,
        pose: Option<TerrainObjectTransform>,
        active: bool,
    },
    Mesh {
        scene_id: SceneId,
        revision: u64,
        value: Option<MeshPreview>,
        committed: bool,
    },
    Height {
        scene_id: SceneId,
        terrain_id: Option<TerrainId>,
        revision: u64,
        reset: bool,
        tiles: Vec<SparseHeightSnapshotTile>,
    },
    Water {
        scene_id: SceneId,
        terrain_id: Option<TerrainId>,
        root: bool,
        revision: u64,
        definition: Option<SceneWaterAsset>,
        masks: BTreeMap<String, Arc<[u8]>>,
    },
}

impl AuthorPreview {
    pub fn scene_id(&self) -> &SceneId {
        match self {
            Self::Extension {scene_id,..} => scene_id,
            Self::SubscenePose { scene_id, .. }
            | Self::TerrainPose { scene_id, .. }
            | Self::Mesh { scene_id, .. }
            | Self::Height { scene_id, .. }
            | Self::Water { scene_id, .. } => scene_id,
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
        AuthorPreview::SubscenePose { scene_id, revision, id, pose, active } => {
            SceneId::parse(scene_id.as_str())?;
            SceneInstanceId::parse(id.as_str())?;
            if *revision == 0 || (*active && pose.is_none()) { return Err("子场景预览缺少有效版本或当前姿态".into()); }
            if let Some(pose) = pose { pose.validate()?; }
        }
        AuthorPreview::TerrainPose { scene_id, revision, id, pose, active } => {
            SceneId::parse(scene_id.as_str())?;
            TerrainId::parse(id.as_str())?;
            if *revision == 0 || (*active && pose.is_none()) { return Err("地形预览缺少有效版本或当前姿态".into()); }
            if let Some(pose) = pose { pose.validate()?; }
        }
        _ => {}
    }
    Ok(())
}
