//! 由组件事件维护的草窗归属索引；定向发布不扫描全部实例。
use bevy::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
/// 事件维护草窗归属，发布时只访问受影响网格的窗口，不扫描全场景草窗。
#[derive(Resource, Default)]
pub(super) struct Followers {
    pub(super) by_root: BTreeMap<Entity, BTreeSet<Entity>>,
    pub(super) by_entity: BTreeMap<Entity, Entity>,
}
impl Followers {
    pub(super) fn remove(&mut self, entity: Entity) {
        if let Some(root) = self.by_entity.remove(&entity)
            && let Some(rows) = self.by_root.get_mut(&root)
        {
            rows.remove(&entity);
            if rows.is_empty() {
                self.by_root.remove(&root);
            }
        }
    }
    pub(super) fn assign(&mut self, entity: Entity, root: Entity) {
        if self.by_entity.get(&entity) == Some(&root) {
            return;
        }
        self.remove(entity);
        self.by_root.entry(root).or_default().insert(entity);
        self.by_entity.insert(entity, root);
    }
}
