//! 通用静态网格LOD的单飞捕获、后台席位、上传信用与冷缓存。
use super::{capture::Capture, types::*};
use bevy::{asset::AssetId, prelude::*, tasks::Task};
use feathrise_map_terrain::StaticMeshLod;
use std::collections::{HashMap, HashSet, VecDeque};

pub(super) const TARGET_BUDGET: usize = 32;
pub(super) const COPY_BUDGET: usize = 128 * 1024;
pub(super) const SOURCE_BYTES: usize = 32 * 1024 * 1024;
pub(super) const UPLOAD_BUDGET: usize = 16 * 1024 * 1024;
pub(super) const CACHE_BYTES: usize = 64 * 1024 * 1024;

pub(super) struct Upload {
    pub key: SourceKey,
    pub pending: VecDeque<StaticMeshLod>,
}

#[derive(Resource, Default)]
pub(super) struct MeshLodRuntime {
    pub pending: VecDeque<Entity>,
    pub queued: HashSet<Entity>,
    pub discovery: VecDeque<(Entity, usize)>,
    pub targets: HashMap<Entity, VisualTarget>,
    pub coarse: HashSet<Entity>,
    pub cursor: VecDeque<Entity>,
    pub revisions: HashMap<AssetId<Mesh>, u64>,
    pub revision_cursor: VecDeque<AssetId<Mesh>>,
    pub revision_nonce: u64,
    pub asset_references: HashMap<AssetId<Mesh>, usize>,
    pub event_epoch: u64,
    pub cache: HashMap<SourceKey, VisualCache>,
    pub cache_cursor: VecDeque<SourceKey>,
    pub cache_queued: HashSet<SourceKey>,
    pub references: HashMap<SourceKey, usize>,
    pub credit_revision: u64,
    /// 活动缓存、待退役资产和正在后台析构的派生数组共同占用信用。
    pub cache_bytes: usize,
    pub requests: VecDeque<SourceKey>,
    pub requested: HashSet<SourceKey>,
    pub capture: Option<Capture>,
    pub task: Option<(SourceKey, Task<Result<Vec<StaticMeshLod>, String>>)>,
    pub upload: Option<Upload>,
    pub release: Option<Task<()>>,
    pub release_credit: usize,
    pub retired: VecDeque<(Handle<Mesh>, usize)>,
}

impl MeshLodRuntime {
    pub fn enqueue(&mut self, entity: Entity) {
        if self.queued.insert(entity) {
            self.pending.push_back(entity);
        }
    }
    pub fn key(&self, id: AssetId<Mesh>) -> SourceKey {
        SourceKey {
            id,
            revision: self.revisions.get(&id).copied().unwrap_or(0),
            epoch: self.event_epoch,
        }
    }
    pub fn current(&self, key: SourceKey) -> bool {
        self.key(key.id) == key
    }
    pub fn request(&mut self, key: SourceKey) {
        if self
            .cache
            .get(&key)
            .and_then(|cache| cache.retry_credit)
            .is_some_and(|revision| revision != self.credit_revision)
        {
            self.cache.remove(&key); // 只有信用真实归还才允许压力回退重试，稳定帧不反复减面。
        }
        if !self.cache.contains_key(&key) && self.requested.insert(key) {
            self.requests.push_back(key);
        }
    }
    pub fn queue_cache(&mut self, key: SourceKey) {
        if self.cache_queued.insert(key) {
            self.cache_cursor.push_back(key);
        }
    }
    pub fn pop_cache(&mut self) -> Option<SourceKey> {
        let key = self.cache_cursor.pop_front()?;
        self.cache_queued.remove(&key);
        Some(key)
    }
    pub fn release_reference(&mut self, key: SourceKey) {
        if let Some(count) = self.asset_references.get_mut(&key.id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.asset_references.remove(&key.id);
            }
        }
        if let Some(count) = self.references.get_mut(&key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.references.remove(&key);
            }
        }
    }
    pub fn retain_reference(&mut self, key: SourceKey) {
        *self.references.entry(key).or_default() += 1;
        *self.asset_references.entry(key.id).or_default() += 1;
    }
    pub fn watch(&mut self, id: AssetId<Mesh>) {
        if !self.revisions.contains_key(&id) {
            self.revision_nonce = self.revision_nonce.wrapping_add(1);
            self.revisions.insert(id, self.revision_nonce);
            self.revision_cursor.push_back(id);
        }
    }
}

/// 状态只说明实际能力与工作阶段，不把保原回退报告为简化成功。
#[derive(Resource)]
pub struct MeshLodRuntimeStatus {
    pub registered_meshes: usize,
    pub prepared_sources: usize,
    /// 作者世界拾取只在全部显示/物理恢复原始拓扑后开放，避免LOD三角序号写入L0。
    pub authoring_ready: bool,
    pub detail: String,
}

impl Default for MeshLodRuntimeStatus {
    fn default() -> Self {
        Self {
            registered_meshes: 0,
            prepared_sources: 0,
            authoring_ready: true,
            detail: String::new(),
        }
    }
}
