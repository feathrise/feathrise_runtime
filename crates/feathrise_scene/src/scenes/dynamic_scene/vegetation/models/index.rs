//! 摆放根的定向身份索引；不在稳定帧扫描全部模型，也不让同源子场景共享实体姿态。
use crate::ScenePlacementInstance;
use bevy::prelude::*;
use feathrise_scene_core::residency::SceneResidencyAppExt;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Resource, Default)]
pub(super) struct ModelSourceIndex {
    pub revision: u64,
    pub by_id: BTreeMap<String, BTreeSet<Entity>>,
    reverse: BTreeMap<Entity, String>,
}
impl ModelSourceIndex {
    fn remove(&mut self, entity: Entity) {
        if let Some(id) = self.reverse.remove(&entity) {
            if let Some(entities) = self.by_id.get_mut(&id) {
                entities.remove(&entity);
                if entities.is_empty() {
                    self.by_id.remove(&id);
                }
            }
            self.revision = self.revision.wrapping_add(1);
        }
    }
}
fn inserted(
    event: On<Insert<ScenePlacementInstance>>,
    roots: Query<&ScenePlacementInstance>,
    mut index: ResMut<ModelSourceIndex>,
) {
    let Ok(root) = roots.get(event.entity) else {
        return;
    };
    index.remove(event.entity);
    index
        .by_id
        .entry(root.id().into())
        .or_default()
        .insert(event.entity);
    index.reverse.insert(event.entity, root.id().into());
    index.revision = index.revision.wrapping_add(1);
}
fn removed(event: On<Remove<ScenePlacementInstance>>, mut index: ResMut<ModelSourceIndex>) {
    index.remove(event.entity);
}
pub(super) fn install(app: &mut App) {
    app.init_resident_resource::<ModelSourceIndex>()
        .add_observer(inserted)
        .add_observer(removed);
}
