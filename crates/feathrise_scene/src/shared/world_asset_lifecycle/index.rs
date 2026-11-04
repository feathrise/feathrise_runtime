//! `WorldAsset`到显式根的固定容量反向索引、临时Modified判定与有界失效游标。

use std::collections::{HashMap, HashSet, VecDeque};

use bevy::asset::AssetId;
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;

/// 活动批、隐藏候选和F12预览最多各占一张合法8192根地图。
pub(super) const MAX_INDEXED_WORLD_ASSET_ROOTS: usize = 24_576;

/// 根登记失败必须由调用方失败关闭，绝不能遗漏后继续发布Ready。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorldAssetRootRegistration {
    Registered,
    AlreadyRegistered,
    Full,
}

#[derive(Clone, Copy, Debug, Default)]
enum AssetSweepMode {
    /// 先撤销旧信用；积压期已经见到的新Ready可直接追授当前epoch。
    #[default]
    Invalidate,
    /// Bevy没有为这条Modified发WorldReady，说明事件被其debounce抑制；恢复此前Ready信用。
    RestorePreviousReady,
}

#[derive(Clone, Copy, Debug)]
struct ProvisionalModification {
    epoch: u64,
    observed_frame: u32,
    confirmed_by_world_ready: bool,
}

#[derive(Debug, Default)]
struct AssetRootBucket {
    roots: Vec<Entity>, // 交换删除保持登记/移除O(1)，游标按环形槽位容忍换位。
    epoch: u64,         // 每条待确认Modified或确定Removed推进一次。
    sweep_serial: u64,  // 同epoch也可能先失效、后恢复，独立序号避免第二轮被误判已处理。
    sweep_cursor: usize,
    sweep_remaining: usize,
    sweep_mode: AssetSweepMode,
    accepted_backlog_generation: Option<u64>,
    last_ready_backlog_generation: Option<u64>,
    provisional: Option<ProvisionalModification>,
}

#[derive(Clone, Copy, Debug)]
struct IndexedRoot {
    asset_id: AssetId<WorldAsset>,
    bucket_index: usize,
    ready_epoch: Option<u64>, // 只有当前epoch的WorldInstanceReady才能给运行时授信。
    ready_backlog_generation: Option<u64>,
    settled_sweep_serial: u64,
}

/// 一次固定预算根失效步骤；`sweep_complete`允许候选解除全局采用阻断。
#[derive(Clone, Copy, Debug)]
pub(crate) struct WorldAssetRootInvalidation {
    pub(crate) entity: Entity,
    pub(crate) asset_id: AssetId<WorldAsset>,
    pub(crate) epoch: u64,
    /// 已见到新WorldReady，或Bevy明确抑制Modified而恢复旧Scene信用。
    pub(crate) ready_after_event: bool,
    pub(crate) sweep_complete: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum WorldAssetInvalidationStep {
    Root(WorldAssetRootInvalidation),
    Complete {
        asset_id: AssetId<WorldAsset>,
        epoch: u64,
    },
    Skipped,
}

/// 生命周期observer维护的唯一索引；稳定帧只检查固定队首，不枚举`WorldAssetRoot`。
#[derive(Resource)]
pub(crate) struct WorldAssetLifecycleIndex {
    roots: HashMap<Entity, IndexedRoot>,
    assets: HashMap<AssetId<WorldAsset>, AssetRootBucket>,
    pending_assets: VecDeque<AssetId<WorldAsset>>,
    queued_assets: HashSet<AssetId<WorldAsset>>,
    pending_resolutions: VecDeque<AssetId<WorldAsset>>,
    queued_resolutions: HashSet<AssetId<WorldAsset>>,
    events_caught_up: bool,
    next_backlog_generation: u64,
    active_backlog_generation: Option<u64>,
    missed_message_count: usize, // 双缓冲淘汰后无法恢复资产ID，必须永久失败关闭到应用重启。
}

impl Default for WorldAssetLifecycleIndex {
    fn default() -> Self {
        Self {
            roots: HashMap::new(),
            assets: HashMap::new(),
            pending_assets: VecDeque::new(),
            queued_assets: HashSet::new(),
            pending_resolutions: VecDeque::new(),
            queued_resolutions: HashSet::new(),
            events_caught_up: true,
            next_backlog_generation: 0,
            active_backlog_generation: None,
            missed_message_count: 0,
        }
    }
}

impl WorldAssetLifecycleIndex {
    pub(crate) fn register(
        &mut self,
        entity: Entity,
        asset_id: AssetId<WorldAsset>,
    ) -> WorldAssetRootRegistration {
        if let Some(existing) = self.roots.get(&entity).copied() {
            if existing.asset_id == asset_id {
                return WorldAssetRootRegistration::AlreadyRegistered;
            }
            self.unregister(entity);
        }
        if self.roots.len() >= MAX_INDEXED_WORLD_ASSET_ROOTS {
            return WorldAssetRootRegistration::Full;
        }
        let bucket = self.assets.entry(asset_id).or_default();
        let bucket_index = bucket.roots.len();
        bucket.roots.push(entity);
        self.roots.insert(
            entity,
            IndexedRoot {
                asset_id,
                bucket_index,
                ready_epoch: None,
                ready_backlog_generation: None,
                settled_sweep_serial: bucket.sweep_serial,
            },
        );
        WorldAssetRootRegistration::Registered
    }

    pub(crate) fn unregister(&mut self, entity: Entity) {
        let Some(removed) = self.roots.remove(&entity) else {
            return;
        };
        let mut remove_bucket = false;
        if let Some(bucket) = self.assets.get_mut(&removed.asset_id) {
            if removed.settled_sweep_serial != bucket.sweep_serial {
                bucket.sweep_remaining = bucket.sweep_remaining.saturating_sub(1);
            }
            let removed_entity = bucket.roots.swap_remove(removed.bucket_index);
            debug_assert_eq!(removed_entity, entity);
            if removed.bucket_index < bucket.roots.len() {
                let moved = bucket.roots[removed.bucket_index];
                if let Some(entry) = self.roots.get_mut(&moved) {
                    entry.bucket_index = removed.bucket_index;
                }
            }
            if bucket.roots.is_empty() {
                remove_bucket = bucket.provisional.is_none();
            } else {
                bucket.sweep_cursor %= bucket.roots.len();
            }
        }
        if remove_bucket {
            self.assets.remove(&removed.asset_id);
        }
    }

    pub(crate) fn contains_asset(&self, asset_id: AssetId<WorldAsset>) -> bool {
        self.assets
            .get(&asset_id)
            .is_some_and(|bucket| !bucket.roots.is_empty())
    }

    pub(crate) fn begin_event_collection(&mut self, unread: usize, budget: usize) -> Option<u64> {
        if unread > budget && self.active_backlog_generation.is_none() {
            self.next_backlog_generation = self.next_backlog_generation.wrapping_add(1).max(1);
            self.active_backlog_generation = Some(self.next_backlog_generation);
        }
        self.events_caught_up = unread <= budget && self.missed_message_count == 0;
        self.active_backlog_generation
    }

    pub(crate) fn finish_event_collection(&mut self, caught_up: bool) {
        self.events_caught_up = caught_up && self.missed_message_count == 0;
        if self.events_caught_up {
            self.active_backlog_generation = None;
        }
    }

    pub(crate) fn events_caught_up(&self) -> bool {
        self.events_caught_up
    }

    /// `Messages`双缓冲已经淘汰未读项时，未知AssetId不允许静默恢复为caught-up。
    pub(crate) fn record_missed_messages(&mut self, count: usize) -> bool {
        let first = self.missed_message_count == 0;
        self.missed_message_count = self.missed_message_count.saturating_add(count);
        self.events_caught_up = false;
        first
    }

    #[cfg(test)]
    pub(crate) fn missed_message_count(&self) -> usize {
        self.missed_message_count
    }

    /// 每条Modified先保守推进epoch；下一Update以是否真实收到WorldReady精确判定Bevy debounce。
    pub(crate) fn begin_modified_invalidation(
        &mut self,
        asset_id: AssetId<WorldAsset>,
        accepted_backlog_generation: Option<u64>,
        frame: u32,
    ) -> (u64, bool) {
        let (epoch, has_roots, needs_resolution) = {
            let Some(bucket) = self.assets.get_mut(&asset_id) else {
                return (0, false); // 没有索引根时只需让placement Scene加载器重查依赖。
            };
            if let Some(provisional) = bucket.provisional.as_mut() {
                if accepted_backlog_generation.is_some()
                    && bucket.last_ready_backlog_generation == accepted_backlog_generation
                {
                    provisional.confirmed_by_world_ready = true;
                }
                return (provisional.epoch, !bucket.roots.is_empty());
            }
            bucket.epoch = bucket.epoch.wrapping_add(1).max(1);
            let epoch = bucket.epoch;
            let confirmed_by_world_ready = accepted_backlog_generation.is_some()
                && bucket.last_ready_backlog_generation == accepted_backlog_generation;
            bucket.provisional = (!confirmed_by_world_ready).then_some(ProvisionalModification {
                epoch,
                observed_frame: frame,
                confirmed_by_world_ready: false,
            });
            bucket.accepted_backlog_generation = accepted_backlog_generation;
            Self::start_sweep(bucket, AssetSweepMode::Invalidate);
            (
                epoch,
                !bucket.roots.is_empty(),
                bucket.provisional.is_some(),
            )
        };
        if has_roots {
            self.requeue_asset(asset_id);
        }
        if needs_resolution {
            self.requeue_resolution(asset_id);
        }
        (epoch, has_roots)
    }

    /// Removed没有后续Ready，直接永久撤销旧Scene信用。
    pub(crate) fn invalidate_removed_asset(
        &mut self,
        asset_id: AssetId<WorldAsset>,
    ) -> (u64, bool) {
        let Some(bucket) = self.assets.get_mut(&asset_id) else {
            return (0, false);
        };
        bucket.epoch = bucket.epoch.wrapping_add(1).max(1);
        bucket.provisional = None;
        bucket.accepted_backlog_generation = None;
        Self::start_sweep(bucket, AssetSweepMode::Invalidate);
        let epoch = bucket.epoch;
        let has_roots = !bucket.roots.is_empty();
        if has_roots {
            self.requeue_asset(asset_id);
        }
        (epoch, has_roots)
    }

    /// 下一帧最多判定固定数量资产；没收到WorldReady的Modified按Bevy真实抑制结果恢复旧信用。
    pub(crate) fn resolve_provisional_modifications(&mut self, frame: u32, budget: usize) {
        for _ in 0..budget {
            let Some(asset_id) = self.pending_resolutions.pop_front() else {
                return;
            };
            self.queued_resolutions.remove(&asset_id);
            let Some(bucket) = self.assets.get_mut(&asset_id) else {
                continue;
            };
            let Some(provisional) = bucket.provisional else {
                continue;
            };
            if frame.wrapping_sub(provisional.observed_frame) < 1 {
                self.requeue_resolution(asset_id);
                continue;
            }
            bucket.provisional = None;
            if !provisional.confirmed_by_world_ready {
                bucket.accepted_backlog_generation = None;
                Self::start_sweep(bucket, AssetSweepMode::RestorePreviousReady);
            }
            self.requeue_asset(asset_id);
        }
    }

    /// `WorldInstanceReady`即使复用相同InstanceId，也会把根授信推进到当前资产epoch。
    pub(crate) fn mark_world_ready(
        &mut self,
        entity: Entity,
        asset_id: AssetId<WorldAsset>,
    ) -> Result<u64, WorldAssetRootRegistration> {
        match self.register(entity, asset_id) {
            WorldAssetRootRegistration::Full => return Err(WorldAssetRootRegistration::Full),
            WorldAssetRootRegistration::Registered
            | WorldAssetRootRegistration::AlreadyRegistered => {}
        }
        let ready_backlog_generation = self.active_backlog_generation;
        let entry = self.roots.get_mut(&entity).expect("根登记成功后必须存在");
        let bucket = self
            .assets
            .get_mut(&asset_id)
            .expect("根登记成功后资产桶必须存在");
        if entry.settled_sweep_serial != bucket.sweep_serial {
            entry.settled_sweep_serial = bucket.sweep_serial;
            bucket.sweep_remaining = bucket.sweep_remaining.saturating_sub(1);
        }
        entry.ready_epoch = Some(bucket.epoch);
        entry.ready_backlog_generation = ready_backlog_generation;
        if let Some(generation) = ready_backlog_generation {
            bucket.last_ready_backlog_generation = Some(generation);
        }
        if let Some(provisional) = bucket.provisional.as_mut()
            && provisional.epoch == bucket.epoch
        {
            provisional.confirmed_by_world_ready = true;
        }
        Ok(bucket.epoch)
    }

    pub(crate) fn current_ready_epoch(
        &self,
        entity: Entity,
        asset_id: AssetId<WorldAsset>,
    ) -> Option<u64> {
        let entry = self.roots.get(&entity)?;
        let bucket = self.assets.get(&asset_id)?;
        (entry.asset_id == asset_id && entry.ready_epoch == Some(bucket.epoch))
            .then_some(bucket.epoch)
    }

    pub(crate) fn request_epoch_is_current(
        &self,
        entity: Entity,
        asset_id: AssetId<WorldAsset>,
        ready_epoch: u64,
    ) -> bool {
        self.events_caught_up && self.current_ready_epoch(entity, asset_id) == Some(ready_epoch)
    }

    /// 每次只检查一个槽位；交换删除或同帧Ready最多产生一次有界空步。
    pub(crate) fn next_invalidation(&mut self) -> Option<WorldAssetInvalidationStep> {
        let asset_id = self.pending_assets.pop_front()?;
        self.queued_assets.remove(&asset_id);
        let Some(bucket) = self.assets.get_mut(&asset_id) else {
            return Some(WorldAssetInvalidationStep::Skipped);
        };
        let epoch = bucket.epoch;
        if bucket.sweep_remaining == 0 || bucket.roots.is_empty() {
            bucket.sweep_remaining = 0;
            if bucket.provisional.is_some() {
                return Some(WorldAssetInvalidationStep::Skipped);
            }
            return Some(WorldAssetInvalidationStep::Complete { asset_id, epoch });
        }
        let index = bucket.sweep_cursor % bucket.roots.len();
        bucket.sweep_cursor = (index + 1) % bucket.roots.len();
        let entity = bucket.roots[index];
        let sweep_serial = bucket.sweep_serial;
        let mode = bucket.sweep_mode;
        let accepted_generation = bucket.accepted_backlog_generation;
        let provisional = bucket.provisional.is_some();
        let (step, should_requeue) = match self.roots.get_mut(&entity) {
            None => (WorldAssetInvalidationStep::Skipped, true),
            Some(entry) if entry.settled_sweep_serial == sweep_serial => {
                (WorldAssetInvalidationStep::Skipped, true)
            }
            Some(entry) => {
                entry.settled_sweep_serial = sweep_serial;
                let ready_after_event = match mode {
                    AssetSweepMode::Invalidate => {
                        accepted_generation.is_some()
                            && entry.ready_backlog_generation == accepted_generation
                    }
                    AssetSweepMode::RestorePreviousReady => entry.ready_epoch.is_some(),
                };
                if ready_after_event {
                    entry.ready_epoch = Some(epoch);
                } else if !provisional {
                    entry.ready_epoch = None;
                }
                bucket.sweep_remaining = bucket.sweep_remaining.saturating_sub(1);
                let sweep_complete = bucket.sweep_remaining == 0 && bucket.provisional.is_none();
                (
                    WorldAssetInvalidationStep::Root(WorldAssetRootInvalidation {
                        entity,
                        asset_id,
                        epoch,
                        ready_after_event,
                        sweep_complete,
                    }),
                    bucket.sweep_remaining != 0,
                )
            }
        };
        if should_requeue {
            self.requeue_asset(asset_id);
        }
        Some(step)
    }

    fn start_sweep(bucket: &mut AssetRootBucket, mode: AssetSweepMode) {
        bucket.sweep_serial = bucket.sweep_serial.wrapping_add(1).max(1);
        bucket.sweep_cursor = 0;
        bucket.sweep_remaining = bucket.roots.len();
        bucket.sweep_mode = mode;
    }

    fn requeue_asset(&mut self, asset_id: AssetId<WorldAsset>) {
        if self.queued_assets.insert(asset_id) {
            self.pending_assets.push_back(asset_id);
        }
    }

    fn requeue_resolution(&mut self, asset_id: AssetId<WorldAsset>) {
        if self.queued_resolutions.insert(asset_id) {
            self.pending_resolutions.push_back(asset_id);
        }
    }
}
