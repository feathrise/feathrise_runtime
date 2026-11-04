//! 按MapId拥有完整运行时与实体索引；没有超时或自动卸载，只有关闭文档才回收。

use bevy::{prelude::*, tasks::Task};
use feathrise_map_content::MapId;
use feathrise_scene_core::residency::{ResidentMapToken, ResidentResourceSnapshot};
use feathrise_scene_core::state::ScenePhase;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

pub(super) struct ResidentScene {
    pub token: ResidentMapToken,
    pub scene: ScenePhase,
    pub document_open: bool,
    pub sleeping: bool,
    pub resources: Option<ResidentResourceSnapshot>,
    pub world_epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidentScenePreparation {
    Absent,
    Parking,
    Restoring,
    ColdReady,
    WarmReady,
}

pub(super) enum TransferPhase {
    Parking,
    Parked,
    Restoring,
    Ready,
    Armed,
}

pub(super) struct ResidentTransfer {
    pub source: Option<MapId>,
    pub target: Option<(MapId, ScenePhase)>,
    pub phase: TransferPhase,
    pub cursor: usize,
    pub warm: bool,
    pub deferred_next: Option<NextState<ScenePhase>>,
    pub transition_scene: Option<ScenePhase>, // 真正执行过StateTransition后才解除全图锁。
    pub loading_transaction: Option<feathrise_scene_core::state::LoadingTransactionId>, // Armed时捕获，完成时不能覆盖后来请求的进度。
}

/// 作者宿主只读取准备状态；资源快照和Entity归属由场景域唯一采用。
#[derive(Resource, Default)]
pub struct ResidentScenes {
    pub(super) maps: BTreeMap<MapId, ResidentScene>,
    pub(super) active: Option<MapId>,
    pub(super) transfer: Option<ResidentTransfer>,
    pub(super) restored: Option<MapId>,
    pub(super) serial: u64,
    pub(super) world_epoch: u64,
    pub(super) retired_scenes: VecDeque<ResidentScene>, // 同MapId新世代采用后，旧实体按原预算回收。
    pub(super) saved_schedules: Vec<Schedule>,
    pub(super) closing: VecDeque<MapId>,
    pub(super) retirement: Option<Task<()>>,
    pub(super) retired_resources: VecDeque<Box<dyn Send + Sync>>, // 快照与资产通知表共用唯一后台析构槽。
}

impl ResidentScenes {
    /// 所选高度的旧实例已移除时，最后持久根也归还既有唯一后台析构槽；不能在交换作者身份的主帧递归释放。
    pub(crate) fn retire_sparse_height(&mut self, height: feathrise_map_content::SparseHeightLiveOverride) {
        self.retired_resources.push_back(Box::new(height));
    }

    pub fn contains(&self, map: &MapId) -> bool {
        self.maps.contains_key(map)
    }
    pub fn sleeping(&self, map: &MapId) -> bool {
        self.maps.get(map).is_some_and(|entry| {
            entry.world_epoch == self.world_epoch && entry.sleeping && entry.resources.is_some()
        })
    }
    pub fn active_is(&self, map: &MapId) -> bool {
        self.active.as_ref() == Some(map) && self.transfer.is_none()
    }
    pub fn restored(&self, map: &MapId) -> bool {
        self.restored.as_ref() == Some(map)
            && self.active.as_ref() == Some(map)
            && self.transfer.is_none()
    }
    /// 恢复期间快照已移出后台槽，但仍是同一次暖恢复，作者历史不能撤销其预览句柄。
    pub fn restoring(&self, map: &MapId) -> bool {
        self.restored(map)
            || self.transfer.as_ref().is_some_and(|transfer| {
                transfer.warm
                    && transfer
                        .target
                        .as_ref()
                        .is_some_and(|(target, _)| target == map)
            })
    }
    pub fn source_ready(&self) -> bool {
        self.transfer
            .as_ref()
            .is_none_or(|t| !matches!(t.phase, TransferPhase::Parking))
    }
    pub fn preparation(&self, target: &MapId) -> ResidentScenePreparation {
        let Some(transfer) = self.transfer.as_ref() else {
            return ResidentScenePreparation::Absent;
        };
        if transfer.target.as_ref().is_none_or(|(id, _)| id != target) {
            return ResidentScenePreparation::Absent;
        }
        match transfer.phase {
            TransferPhase::Parking => ResidentScenePreparation::Parking,
            TransferPhase::Restoring => ResidentScenePreparation::Restoring,
            TransferPhase::Parked if transfer.warm => ResidentScenePreparation::Restoring,
            TransferPhase::Parked | TransferPhase::Ready | TransferPhase::Armed => {
                if transfer.warm {
                    ResidentScenePreparation::WarmReady
                } else {
                    ResidentScenePreparation::ColdReady
                }
            }
        }
    }
    pub(super) fn ensure(&mut self, map: MapId, scene: ScenePhase) -> ResidentMapToken {
        if let Some(entry) = self.maps.get(&map)
            && entry.scene == scene
            && entry.world_epoch == self.world_epoch
        {
            return entry.token;
        }
        let document_open = self.maps.get(&map).is_some_and(|entry| entry.document_open);
        if let Some(old) = self.maps.remove(&map) {
            self.retired_scenes.push_back(old);
        }
        self.serial = self.serial.wrapping_add(1).max(1);
        let token = ResidentMapToken(self.serial);
        self.maps.insert(
            map,
            ResidentScene {
                token,
                scene,
                document_open,
                sleeping: false,
                resources: None,
                world_epoch: self.world_epoch,
            },
        );
        token
    }
}

pub(super) struct DescendantCursor {
    pub parent: Entity,
    pub next: usize,
}

/// 新对象通过关系/归属观察器增量登记；切换不扫描整个World或复制整份Entity集合。
#[derive(Resource, Default)]
pub struct ResidentEntityIndex {
    pub(super) maps: HashMap<ResidentMapToken, Vec<Entity>>,
    pub(super) positions: HashMap<Entity, (ResidentMapToken, usize)>,
    pub(super) descendants: VecDeque<DescendantCursor>,
    pub(super) cameras: HashMap<ResidentMapToken, HashMap<Entity, bool>>, // 每图固定的玩法/F12视图在采用点一起开启。
    pub(super) parked: HashSet<Entity>,
    pub(super) parked_counts: HashMap<ResidentMapToken, usize>, // 删除/热更改变索引时仍能O(1)证明整图暂停或恢复完整。
    revisions: HashMap<ResidentMapToken, u64>, // 分帧只读遍历必须识别swap_remove/新增，不能采用半份旧索引。
}

impl ResidentEntityIndex {
    /// 后代登记尚未耗尽时，跨帧消费者不能把根已就绪误当完整内容已登记。
    pub fn registration_pending(&self) -> bool {
        !self.descendants.is_empty()
    }

    /// 借用指定地图的已登记实体和结构修订；调用方按固定预算推进游标，禁止整表复制。
    pub fn snapshot(&self, token: ResidentMapToken) -> (u64, &[Entity]) {
        (self.revisions.get(&token).copied().unwrap_or(0), self.maps.get(&token).map_or(&[], Vec::as_slice))
    }

    /// 所有权登记入口；只由归属观察器更新，编辑器等消费者仅借用snapshot。
    pub fn insert(&mut self, entity: Entity, token: ResidentMapToken) {
        if self
            .positions
            .get(&entity)
            .is_some_and(|(old, _)| *old == token)
        {
            return;
        }
        self.remove(entity);
        *self.revisions.entry(token).or_default() += 1;
        let values = self.maps.entry(token).or_default();
        self.positions.insert(entity, (token, values.len()));
        values.push(entity);
        self.descendants.push_back(DescendantCursor {
            parent: entity,
            next: 0,
        });
    }
    pub fn remove(&mut self, entity: Entity) {
        let Some((token, index)) = self.positions.remove(&entity) else {
            return;
        };
        *self.revisions.entry(token).or_default() += 1;
        if self.parked.remove(&entity) {
            *self.parked_counts.entry(token).or_default() -= 1;
        }
        if let Some(cameras) = self.cameras.get_mut(&token) {
            cameras.remove(&entity);
        }
        let values = self.maps.get_mut(&token).unwrap();
        values.swap_remove(index);
        if let Some(swapped) = values.get(index) {
            self.positions.get_mut(swapped).unwrap().1 = index;
        }
    }
    pub fn mark_parked(&mut self, entity: Entity, parked: bool) {
        let Some(&(token, _)) = self.positions.get(&entity) else {
            return;
        };
        if parked {
            if self.parked.insert(entity) {
                *self.parked_counts.entry(token).or_default() += 1;
            }
        } else if self.parked.remove(&entity) {
            *self.parked_counts.entry(token).or_default() -= 1;
        }
    }
}
