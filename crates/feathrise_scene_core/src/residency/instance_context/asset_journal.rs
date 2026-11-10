//! 有界资产通知日志：子场景轮转调度不会受Bevy两帧消息保留期影响，也不伪造GPU资产事件。

use bevy::{asset::AssetId, prelude::*};
use std::collections::{BTreeMap, HashMap};

const CAPTURE_BUDGET: usize = 128;
const MAX_ASSET_NOTICES: usize = 4096;
// 首载和GC通知只做常数元数据分类，不应挤占真正失效通知的写入预算；分类本身仍有固定最坏边界。
const CLASSIFY_BUDGET: usize = MAX_ASSET_NOTICES;

/// 日志溢出后宿主必须重建完整候选，不能继续声称未收到通知的旧缓存有效。
#[derive(Resource, Default)]
pub struct InstanceAssetJournalEpoch(pub u64);

#[derive(Resource)]
pub struct InstanceAssetJournal<A: Asset> {
    pub(crate) revision: u64,
    pub(crate) records: BTreeMap<u64, AssetEvent<A>>,
    ids: HashMap<AssetId<A>, u64>,
    // Bevy最后强句柄释放时连续发布Unused、Removed；只跨帧保留这一个相邻配对，不能永久屏蔽同ID。
    gc_removed: Option<AssetId<A>>,
}
impl<A: Asset> Default for InstanceAssetJournal<A> {
    fn default() -> Self { Self { revision: 0, records: BTreeMap::new(), ids: HashMap::new(), gc_removed: None } }
}
impl<A: Asset> InstanceAssetJournal<A> {
    fn forget(&mut self, id: AssetId<A>) {
        if let Some(old) = self.ids.remove(&id) { self.records.remove(&old); }
    }
}

fn capture<A: Asset>(mut events: MessageReader<AssetEvent<A>>, mut journal: ResMut<InstanceAssetJournal<A>>,
    mut epoch: ResMut<InstanceAssetJournalEpoch>) {
    let mut captured = 0;
    let mut lost_invalidation = false;
    for event in events.read().take(CLASSIFY_BUDGET) {
        let gc_removed = journal.gc_removed.take();
        if let AssetEvent::Unused { id } = *event {
            // 资产已无强引用，旧派生实例也不可能继续依赖它；退役记录不能永久占满日志后反向重建整图。
            journal.forget(id);
            journal.gc_removed = Some(id);
            continue;
        }
        if matches!(*event, AssetEvent::Removed { id } if Some(id) == gc_removed) { continue; }
        // 首载由域自己的Waiting/dirty推进；只持久保留能使完整缓存失效的边沿。
        let id = match *event { AssetEvent::Modified { id } | AssetEvent::Removed { id } => id, _ => continue };
        if captured == CAPTURE_BUDGET { lost_invalidation = true; break; }
        captured += 1;
        journal.forget(id);
        journal.revision = journal.revision.wrapping_add(1);
        let revision = journal.revision;
        journal.records.insert(revision, *event);
        journal.ids.insert(id, revision);
        if journal.records.len() > MAX_ASSET_NOTICES {
            if let Some((_, removed)) = journal.records.pop_first() {
                let id = match removed { AssetEvent::Modified { id } | AssetEvent::Removed { id } => id, _ => unreachable!() };
                journal.ids.remove(&id);
            }
            lost_invalidation = true;
        }
    }
    if !events.is_empty() {
        // 未分类尾部可能含真实热更，仍必须保守恢复；不能用无界filter().take()隐藏帧内扫描成本。
        events.clear(); journal.gc_removed = None; lost_invalidation = true;
    }
    if lost_invalidation { epoch.0 = epoch.0.wrapping_add(1); }
}

#[cfg(test)]
mod tests;

pub fn install_instance_asset_journal<A: Asset>(app: &mut App) {
    if app.world().contains_resource::<InstanceAssetJournal<A>>() { return; }
    app.init_resource::<InstanceAssetJournalEpoch>().init_resource::<InstanceAssetJournal<A>>()
        .add_systems(PreUpdate, capture::<A>);
}
