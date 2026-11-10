//! 父世界的完整子场景事务；候选、活动与退役各保留一份，节点推进共用固定预算。

use bevy::prelude::*;
use feathrise_scene_content::{ExpandedSceneInstance, SceneId, SceneInstancePath};
use std::{collections::{BTreeMap, BTreeSet, VecDeque}, sync::Arc};

#[derive(Resource, Default)]
pub struct SubsceneRuntimeStatus {
    pub root_scene_id: Option<SceneId>,
    pub ready: bool,
    /// 旧有效批次可保持ready；此版本单独证明请求中的全部对象姿态已经采用。
    pub applied_revision: u64,
    pub total: usize,
    pub prepared: usize,
    /// 当前准备席位的真实等待项，防止工作台长期只显示“加载0/1”。
    pub loading_detail: Option<String>,
    pub last_error: Option<String>,
}

pub(super) struct Context {
    pub entity: Entity,
    pub path: SceneInstancePath,
    pub schedule: Schedule,
    pub follow: Schedule,
    pub initialized: bool,
    pub ready: bool,
    pub failed: bool,
    pub fall_death_y: Option<f32>,
}

pub(super) struct Batch {
    pub root: Entity,
    pub graph: Arc<[ExpandedSceneInstance]>,
    pub revision: u64,
    pub next: usize,
    pub contexts: Vec<Context>,
    pub index: BTreeMap<SceneInstancePath, Entity>,
    pub leases: Vec<usize>, // 所有子图共用两个准备席位，不随实例数增加CPU/IO并发。
    pub cursor: usize,
    pub prepared: usize,
    pub fall_thresholds: BTreeSet<(u32, Entity)>, // 浮点排序键支持单实例O(log N)更新全图最低合法高度。
    pub cleanup: Vec<Entity>,
    pub retirement_cursor: usize,
    pub cleanup_started: bool,
}

/// 子调度不能借用driver正在移出的Runtime；每批次保留独立只读地址，候选和活动绝不混用。
#[derive(Resource, Default)]
pub(in crate::scenes::dynamic_scene) struct TerrainInstanceLookup {
    pub(super) batches: BTreeMap<Entity, TerrainLookupBatch>,
}

#[derive(Default)]
pub(super) struct TerrainLookupBatch {
    pub(super) entities: BTreeMap<SceneInstancePath, Entity>,
    pub(super) contexts_complete: bool,
}

impl TerrainInstanceLookup {
    #[cfg(test)]
    pub(in crate::scenes::dynamic_scene) fn test_batch(&mut self,batch:Entity,entities:BTreeMap<SceneInstancePath,Entity>) {
        self.batches.insert(batch,TerrainLookupBatch{entities,contexts_complete:true});
    }
    pub(in crate::scenes::dynamic_scene) fn instance(&self, batch: Entity, path: &SceneInstancePath) -> Option<Entity> {
        self.batches.get(&batch)?.entities.get(path).copied()
    }

    /// 只有整批上下文已创建，缺失地址才能从暂时等待升级为确定失效。
    pub(in crate::scenes::dynamic_scene) fn contexts_complete(&self, batch: Entity) -> bool {
        self.batches.get(&batch).is_some_and(|row| row.contexts_complete)
    }
}

#[derive(Resource, Default)]
pub(in crate::scenes::dynamic_scene) struct SubsceneRuntime {
    pub(super) root_scene_id: Option<SceneId>,
    pub(super) observed_revision: u64,
    pub(super) generation: u64,
    pub(super) candidate: Option<Batch>,
    pub(super) active: Option<Batch>,
    pub(super) retired: VecDeque<Batch>,
    pub(super) releasing: Option<bevy::tasks::Task<()>>,
    pub(super) journal_epoch: u64,
    pub(super) comparison: Option<bevy::tasks::Task<super::planning::Comparison>>,
    pub(super) transform_update: Option<super::planning::Comparison>,
}

impl SubsceneRuntime {
    /// 只公开已原子采用批次的身份索引，查询方不接触候选或退役事务。
    pub(in crate::scenes::dynamic_scene) fn active_entity(&self, path: &SceneInstancePath) -> Option<Entity> {
        self.active.as_ref()?.index.get(path).copied()
    }

    /// 无窗口查询回归只安装小型已采用地址表，不构造正式加载/GPU后台事务。
    #[cfg(any(test, feature = "runtime_test_support"))]
    pub(in crate::scenes::dynamic_scene) fn test_active(root: Entity, index: BTreeMap<SceneInstancePath, Entity>) -> Self {
        Self { active: Some(Batch {root, index, graph: Arc::from([]), revision:1, next:0,
            contexts:Vec::new(), leases:Vec::new(), cursor:0, prepared:0, fall_thresholds:Default::default(),
            cleanup:Vec::new(), retirement_cursor:0, cleanup_started:false}), ..Default::default() }
    }
}
