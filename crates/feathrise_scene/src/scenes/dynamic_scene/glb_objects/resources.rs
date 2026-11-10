//! 全场景碰撞的分帧捕获和单飞任务；所有GLB共用CPU捕获预算。
use avian3d::prelude::Collider;
use bevy::{prelude::*, tasks::Task};
use std::collections::BTreeSet;
use super::capture::Capture;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Identity { pub root: Entity, pub epoch: u64, pub revision: u64, pub scale: [u32;3] }
#[derive(Resource, Default)]
pub(in crate::scenes::dynamic_scene) struct GlbObjectRuntime {
    pub(super) revision: u64,
    pub(super) desired: Option<Identity>,
    pub(super) applied: Option<Identity>,
    pub(super) failed: Option<Identity>,
    pub(super) capture: Option<Capture>,
    pub(super) task: Option<Task<(Identity, Result<Collider, String>)>>,
    pub(super) release: Option<Task<()>>,
    pub(super) sources: BTreeSet<AssetId<Mesh>>,
}
impl GlbObjectRuntime {
    pub(in crate::scenes::dynamic_scene) fn busy(&self) -> bool { self.capture.is_some() || self.task.is_some() || self.release.is_some() }
    pub(in crate::scenes::dynamic_scene) fn finished(&self) -> bool {
        self.task.as_ref().is_none_or(Task::is_finished) && self.release.as_ref().is_none_or(Task::is_finished)
    }
}

#[derive(Resource, Default)]
pub(super) struct CaptureBudget { stamp: u64, pub bytes: usize, pub nodes: usize }
impl CaptureBudget {
    pub(super) fn begin(&mut self, stamp: u64) {
        if self.stamp != stamp { self.stamp = stamp; self.bytes = 128 * 1024; self.nodes = 64; }
    }
}
