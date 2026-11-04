//! 地形背面偏好运行时投影与材质目标索引；不保存地图作者数据。

use super::materials::BackfaceMaterial;
use bevy::prelude::*;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// 编辑器与玩法视图共享显示意图；仅关闭开关时分帧恢复各源材质显示。
#[derive(Resource, Default)]
pub struct TerrainBackfacesPreview {
    pub enabled: bool,
    pub(super) active_targets: usize,
}

impl TerrainBackfacesPreview {
    pub fn is_restoring(&self) -> bool {
        !self.enabled && self.active_targets != 0
    }
}

pub(super) struct Target<M: BackfaceMaterial> {
    pub source: Handle<M>,
    pub overridden: bool,
}

pub(super) struct Source<M: BackfaceMaterial> {
    pub users: usize,
    pub active: usize,
    pub preview: Option<Handle<M>>,
}

#[derive(Resource)]
pub(super) struct MaterialTargets<M: BackfaceMaterial> {
    pub targets: BTreeMap<Entity, Target<M>>,
    pub sources: HashMap<AssetId<M>, Source<M>>,
    pub pending: VecDeque<Entity>,
    pub queued: HashSet<Entity>,
    pub changed_sources: VecDeque<AssetId<M>>,
    pub queued_sources: HashSet<AssetId<M>>,
    /// 外层None表示无扫描；内层游标每帧最多推进固定数量，不复制完整目标集合。
    pub sweep: Option<Option<Entity>>,
    pub enabled: bool,
    pub active: usize,
}

impl<M: BackfaceMaterial> Default for MaterialTargets<M> {
    fn default() -> Self {
        Self {
            targets: default(),
            sources: default(),
            pending: default(),
            queued: default(),
            changed_sources: default(),
            queued_sources: default(),
            sweep: None,
            enabled: false,
            active: 0,
        }
    }
}

impl<M: BackfaceMaterial> MaterialTargets<M> {
    pub fn enqueue(&mut self, entity: Entity) {
        if self.queued.insert(entity) {
            self.pending.push_back(entity);
        }
    }

    pub fn remove(&mut self, entity: Entity) {
        let Some(target) = self.targets.remove(&entity) else {
            return;
        };
        if target.overridden {
            self.active -= 1;
        }
        if let Some(source) = self.sources.get_mut(&target.source.id()) {
            source.users -= 1;
            if target.overridden {
                source.active -= 1;
            }
            if source.users == 0 {
                self.sources.remove(&target.source.id());
            }
        }
        self.queued.remove(&entity);
    }
}
