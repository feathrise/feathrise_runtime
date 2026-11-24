//! 静水本体按来源分离游戏参与与作者显隐，复用所属水网完整采用。

use super::super::instances::TerrainObjectInstance;
use bevy::{ecs::system::SystemParam, prelude::*};
use feathrise_scene_content::{SceneId, SceneManifestAsset, SceneStillWaterPreviewState, TerrainId};
use feathrise_scene_core::residency::InstanceFrame;
use std::{collections::{BTreeMap, BTreeSet}, sync::Arc};

#[derive(SystemParam)]
pub(crate) struct StillWaterSources<'w, 's> {
    pub(super) frame: InstanceFrame<'w>,
    terrains: Query<'w, 's, &'static TerrainObjectInstance>,
    preview: Option<Res<'w, SceneStillWaterPreviewState>>,
}

#[derive(Default)]
pub(super) struct StillWaterRuntime {
    disabled: Option<Arc<BTreeSet<String>>>,
    pub(super) participation_revision: u64,
    preview_revision: Option<u32>,
    preview: Option<StillWaterAuthorVisibility>,
}

struct StillWaterAuthorVisibility {
    terrain: Option<TerrainId>,
    disabled: Option<Arc<BTreeSet<String>>>,
    eyes: Arc<BTreeMap<Option<TerrainId>, BTreeSet<String>>>,
}

impl StillWaterAuthorVisibility {
    fn hidden(&self, id: &str) -> bool {
        self.disabled.as_ref().is_some_and(|ids| ids.contains(id))
            || self.eyes.get(&self.terrain).is_some_and(|ids| ids.contains(id))
    }
}

fn same_disabled_set(
    current: Option<&Arc<BTreeSet<String>>>,
    next: Option<&Arc<BTreeSet<String>>>,
) -> bool {
    match (current, next) {
        (Some(current), Some(next)) => Arc::ptr_eq(current, next),
        (None, None) => true,
        _ => false,
    }
}

pub(super) struct StillWaterDisplay {
    pub(super) id: String,
    pub(super) root: Entity,
}

impl StillWaterSources<'_, '_> {
    pub(super) fn update(
        &self,
        state: &mut StillWaterRuntime,
        scene: Option<&SceneId>,
        manifest: Option<&SceneManifestAsset>,
    ) -> (bool, bool) {
        let terrain = self.frame.instance().and_then(|entity| self.terrains.get(entity).ok())
            .map(|instance| &instance.definition.id);
        let author = self.preview.as_ref().filter(|preview| Some(preview.scene_id()) == scene);
        // 编辑器构建所有作者层，确保关闭参与的草稿仍有保存回执。正式游戏只准备参与层。
        let disabled = if author.is_none() {
            manifest.and_then(|manifest| manifest.still_water_disabled_layers(terrain))
        } else {
            None
        };
        let participation_changed = state.update_participation(disabled);
        let visibility_changed = state.update_preview(author, terrain);
        (participation_changed, visibility_changed)
    }
}

impl StillWaterRuntime {
    fn update_participation(&mut self, disabled: Option<&Arc<BTreeSet<String>>>) -> bool {
        if same_disabled_set(self.disabled.as_ref(), disabled) {
            return false;
        }
        let changed = self.disabled.as_deref() != disabled.map(Arc::as_ref);
        self.disabled = disabled.cloned();
        if changed { self.participation_revision = self.participation_revision.wrapping_add(1); }
        changed
    }

    fn update_preview(
        &mut self,
        preview: Option<&Res<SceneStillWaterPreviewState>>,
        terrain: Option<&TerrainId>,
    ) -> bool {
        let Some(preview) = preview else {
            self.preview_revision = None;
            return self.preview.take().is_some();
        };
        let revision = Some(preview.last_changed().get());
        let same_scope = self.preview.as_ref().is_some_and(|old| old.terrain.as_ref() == terrain);
        if self.preview_revision == revision && same_scope {
            return false;
        }
        self.preview_revision = revision;
        let (disabled, eyes) = preview.visibility_snapshot(terrain);
        // 读共享输入身份即可。锁定位改变和稳定帧都不复制或合并隐藏集合。
        if same_scope && self.preview.as_ref().is_some_and(|old| {
            same_disabled_set(old.disabled.as_ref(), disabled) && Arc::ptr_eq(&old.eyes, eyes)
        }) {
            return false;
        }
        self.preview = Some(StillWaterAuthorVisibility {
            terrain: terrain.cloned(),
            disabled: disabled.cloned(),
            eyes: eyes.clone(),
        });
        true
    }

    pub(super) fn enabled(&self, id: &str) -> bool {
        self.disabled.as_ref().is_none_or(|ids| !ids.contains(id))
    }

    pub(super) fn visibility(&self, id: &str) -> Visibility {
        if self.enabled(id) && self.preview.as_ref().is_none_or(|preview| !preview.hidden(id)) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        }
    }
}
