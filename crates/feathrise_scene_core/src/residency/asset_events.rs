//! 场景运行时专用的热更新回放；不向Bevy资产/GPU上传和WorldSpawner伪造再次修改。

use bevy::{
    ecs::{message::MessageCursor, system::SystemParam},
    prelude::*,
};

/// 原资产身份和事件内容不变；独立通道让CPU派生缓存补收后台通知，GPU资产继续复用原分配。
#[derive(Message)]
pub struct ResidentAssetEvent<A: Asset>(pub AssetEvent<A>);

/// 未装配驻留的独立插件/测试仍只读取原资产事件，可选回放通道不增加安装前提。
#[derive(SystemParam)]
pub struct ResidentAssetEventReader<'w, 's, A: Asset> {
    original: MessageReader<'w, 's, AssetEvent<A>>,
    replayed: Option<Res<'w, Messages<ResidentAssetEvent<A>>>>,
    cursor: Local<'s, MessageCursor<ResidentAssetEvent<A>>>,
    journal: Option<Res<'w, crate::residency::InstanceAssetJournal<A>>>,
    journal_cursor: Local<'s, u64>,
    scope: Option<Res<'w, crate::residency::MapRuntimeScope>>,
}

impl<A: Asset> ResidentAssetEventReader<'_, '_, A> {
    pub fn read(&mut self) -> impl Iterator<Item = &AssetEvent<A>> {
        let child = self.scope.as_deref().is_some_and(|scope| scope.instance.is_some());
        let journal = self.journal.as_deref().filter(|_| child);
        let old_cursor = *self.journal_cursor;
        let notices = journal.into_iter().flat_map(move |journal|
            journal.records.range((std::ops::Bound::Excluded(old_cursor), std::ops::Bound::Unbounded)))
            .map(|(revision, event)| { *self.journal_cursor = *revision; event });
        let replayed = self
            .replayed
            .as_deref()
            .map(|messages| self.cursor.read(messages));
        self.original
            .read()
            .chain(replayed.into_iter().flatten().map(|event| &event.0))
            .chain(notices)
    }

    /// 未读数量包括有界日志；轮转的子图不能因为原通道已过期跳过失效通知。
    pub fn len(&self) -> usize {
        self.original.len()
            + self
                .replayed
                .as_deref()
                .map_or(0, |messages| self.cursor.len(messages))
            + self.journal.as_deref().filter(|_| self.scope.as_deref()
                .is_some_and(|scope| scope.instance.is_some())).map_or(0, |journal|
                journal.records.range((std::ops::Bound::Excluded(*self.journal_cursor),
                    std::ops::Bound::Unbounded)).count())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&mut self) {
        self.original.clear();
        if let Some(messages) = self.replayed.as_deref() {
            self.cursor.clear(messages);
        }
        if let Some(journal) = self.journal.as_deref() { *self.journal_cursor = journal.revision; }
    }
}
