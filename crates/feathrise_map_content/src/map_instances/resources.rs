//! 实例图共享完成值与单飞后台事务；加载时继续保留上一完整有效树。

use super::{ExpandedMapInstance, MapInstancePlacement, MapInstanceSource};
use crate::MapId;
use bevy::{asset::AssetId, prelude::*, tasks::Task};
use std::{collections::{BTreeMap, BTreeSet}, sync::Arc};

/// 场景明确登记当前父世界；MapId仍表示来源文件，不表示某个子实例。
#[derive(Resource, Clone, Debug, Default)]
pub struct MapInstanceRootRequest {
    pub map_id: Option<MapId>,
}

/// 只有当前父图自己的未保存引用参与预览；其他来源始终读取已保存正式内容。
#[derive(Resource, Default)]
pub struct MapInstanceAuthorPreview {
    pub map_id: Option<MapId>,
    pub revision: u64,
    pub instances: Arc<[MapInstancePlacement]>,
    pub terrains: Option<Arc<[crate::MapTerrainObject]>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MapInstanceGraphPhase {
    #[default]
    Waiting,
    Building,
    Ready,
    Failed,
}

/// 来源链全部完成才发布；内容各域的GPU/碰撞就绪由场景后续门负责。
#[derive(Resource, Default)]
pub struct MapInstanceGraph {
    pub root_map_id: Option<MapId>,
    pub revision: u64,
    pub structural_revision: u64, // 姿态预览不推进结构身份，避免连续拖动使每个后台姿态结果都失效。
    pub instances: Arc<[ExpandedMapInstance]>,
    pub phase: MapInstanceGraphPhase,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GraphIdentity {
    pub root: Option<MapId>,
    pub revision: u64,
    pub author_revision: u64,
}

pub(super) struct GraphCompletion {
    pub identity: GraphIdentity,
    pub sources: BTreeMap<MapId, Arc<MapInstanceSource>>,
    pub result: Result<Arc<[ExpandedMapInstance]>, String>,
    pub pending: BTreeSet<MapId>,
    pub waiting: bool,
    pub root_references: Arc<[MapInstancePlacement]>,
}

pub(super) enum SourceUpdate { Ready(Arc<MapInstanceSource>), Pending, Unavailable }

#[derive(Resource, Default)]
pub(super) struct MapInstanceGraphWork {
    pub sources: BTreeMap<MapId, Arc<MapInstanceSource>>,
    pub handles: BTreeMap<AssetId<crate::MapManifestAsset>, MapId>,
    pub known_sources: BTreeSet<MapId>,
    pub queued: BTreeSet<MapId>,
    pub updates: BTreeMap<MapId, SourceUpdate>,
    pub pending: BTreeSet<MapId>,
    pub catalog_revision: Option<u64>,
    pub cursor: Option<MapId>,
    pub scanning: bool,
    pub pruning: bool,
    pub prune_cursor: Option<MapId>,
    pub revision: u64,
    pub desired: Option<GraphIdentity>,
    pub started: Option<GraphIdentity>,
    pub task: Option<Task<GraphCompletion>>,
    pub releasing: Option<Task<()>>, // 完整图的释放也占用同一个有界后台席位。
    pub published_sources_revision: u64,
    pub published_references: Arc<[MapInstancePlacement]>,
}
