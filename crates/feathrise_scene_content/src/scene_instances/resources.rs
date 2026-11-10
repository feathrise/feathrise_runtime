//! 实例图共享完成值与单飞后台事务；加载时继续保留上一完整有效树。

use super::{ExpandedSceneInstance, SceneInstancePlacement, SceneInstanceSource};
use crate::SceneId;
use bevy::{asset::AssetId, prelude::*, tasks::Task};
use std::{collections::{BTreeMap, BTreeSet}, sync::Arc};

/// 场景明确登记当前父世界；SceneId仍表示来源文件，不表示某个子实例。
#[derive(Resource, Clone, Debug, Default)]
pub struct SceneInstanceRootRequest {
    pub scene_id: Option<SceneId>,
}

/// 只有当前父图自己的未保存引用参与预览；其他来源始终读取已保存正式内容。
#[derive(Resource, Default)]
pub struct SceneInstanceAuthorPreview {
    pub scene_id: Option<SceneId>,
    pub revision: u64,
    pub instances: Arc<[SceneInstancePlacement]>,
    pub terrains: Option<Arc<[crate::SceneTerrainObject]>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SceneInstanceGraphPhase {
    #[default]
    Waiting,
    Building,
    Ready,
    Failed,
}

/// 来源链全部完成才发布；内容各域的GPU/碰撞就绪由场景后续门负责。
#[derive(Resource, Default)]
pub struct SceneInstanceGraph {
    pub root_scene_id: Option<SceneId>,
    pub revision: u64,
    pub structural_revision: u64, // 姿态预览不推进结构身份，避免连续拖动使每个后台姿态结果都失效。
    pub instances: Arc<[ExpandedSceneInstance]>,
    pub phase: SceneInstanceGraphPhase,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GraphIdentity {
    pub root: Option<SceneId>,
    pub revision: u64,
    pub author_revision: u64,
}

pub(super) struct GraphCompletion {
    pub identity: GraphIdentity,
    pub sources: BTreeMap<SceneId, Arc<SceneInstanceSource>>,
    pub result: Result<Arc<[ExpandedSceneInstance]>, String>,
    pub pending: BTreeSet<SceneId>,
    pub waiting: bool,
    pub root_references: Arc<[SceneInstancePlacement]>,
}

pub(super) enum SourceUpdate { Ready(Arc<SceneInstanceSource>), Pending, Unavailable }

#[derive(Resource, Default)]
pub(super) struct SceneInstanceGraphWork {
    pub sources: BTreeMap<SceneId, Arc<SceneInstanceSource>>,
    pub handles: BTreeMap<AssetId<crate::SceneManifestAsset>, SceneId>,
    pub known_sources: BTreeSet<SceneId>,
    pub queued: BTreeSet<SceneId>,
    pub updates: BTreeMap<SceneId, SourceUpdate>,
    pub pending: BTreeSet<SceneId>,
    pub catalog_revision: Option<u64>,
    pub cursor: Option<SceneId>,
    pub scanning: bool,
    pub pruning: bool,
    pub prune_cursor: Option<SceneId>,
    pub revision: u64,
    pub desired: Option<GraphIdentity>,
    pub started: Option<GraphIdentity>,
    pub task: Option<Task<GraphCompletion>>,
    pub releasing: Option<Task<()>>, // 完整图的释放也占用同一个有界后台席位。
    pub published_sources_revision: u64,
    pub published_references: Arc<[SceneInstancePlacement]>,
}
