//! 单静水体的清单元数据和作者视口覆盖；同 ID 的场景根与私有地形分别存储。

use bevy::prelude::{Resource, World};
use serde::{Deserialize, Serialize};
use std::{collections::{BTreeMap, BTreeSet}, sync::Arc};

use crate::{SceneId, SceneManifestAsset, TerrainId};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct SourceSettings {
    // 独立 Arc 保留既有参与过滤身份，单改锁定位不重建正式水网。
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    disabled: Arc<BTreeSet<String>>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    locked: Arc<BTreeSet<String>>,
}

impl SourceSettings {
    fn is_empty(&self) -> bool { self.disabled.is_empty() && self.locked.is_empty() }

    fn set(&mut self, id: &str, enabled: bool, locked: bool) {
        fn set_member(set: &mut Arc<BTreeSet<String>>, id: &str, present: bool) {
            match (present, set.contains(id)) {
                (true, false) => { Arc::make_mut(set).insert(id.to_owned()); }
                (false, true) => { Arc::make_mut(set).remove(id); }
                _ => {}
            }
        }
        set_member(&mut self.disabled, id, !enabled);
        set_member(&mut self.locked, id, locked);
    }
}

/// 只保存与默认参与、解锁状态有差异的稳定水体身份。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneStillWaterSettings {
    #[serde(skip_serializing_if = "SourceSettings::is_empty")]
    root: SourceSettings,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    terrains: BTreeMap<TerrainId, SourceSettings>,
}

impl SceneStillWaterSettings {
    pub(crate) fn is_empty(&self) -> bool { self.root.is_empty() && self.terrains.is_empty() }

    fn source(&self, terrain: Option<&TerrainId>) -> Option<&SourceSettings> {
        terrain.map_or(Some(&self.root), |terrain| self.terrains.get(terrain))
    }
}

/// 编辑器只为当前场景替换正式参与状态；眼睛不进入清单。
#[derive(Resource)]
pub struct SceneStillWaterPreviewState {
    scene: SceneId,
    settings: Arc<SceneStillWaterSettings>,
    hidden: Arc<BTreeMap<Option<TerrainId>, BTreeSet<String>>>,
}

impl SceneStillWaterPreviewState {
    /// 预览只接管这个场景的水域。
    pub fn scene_id(&self) -> &SceneId {
        &self.scene
    }

    /// 借出当前来源的停用集与眼睛快照，运行时只需交换 Arc。
    pub fn visibility_snapshot(
        &self,
        terrain: Option<&TerrainId>,
    ) -> (Option<&Arc<BTreeSet<String>>>, &Arc<BTreeMap<Option<TerrainId>, BTreeSet<String>>>) {
        let disabled = self.settings.source(terrain)
            .and_then(|source| (!source.disabled.is_empty()).then_some(&source.disabled));
        (disabled, &self.hidden)
    }

    /// 返回草稿停用集和会话隐藏集；空集仍表示作者视口掌管当前场景的呈现。
    pub fn hidden(&self, scene: &SceneId, terrain: Option<&TerrainId>) -> Option<(Option<&BTreeSet<String>>, Option<&BTreeSet<String>>)> {
        if &self.scene != scene { return None; }
        let (disabled, hidden) = self.visibility_snapshot(terrain);
        Some((disabled.map(Arc::as_ref), hidden.get(&terrain.cloned())))
    }
}

impl SceneManifestAsset {
    /// 公开编辑器可检测旧 SDK 是否能保存逐静水体状态。
    pub const fn supports_still_water_layers(&self) -> bool { true }

    /// 返回指定来源中的（参与，锁定），未登记的旧水体使用默认值。
    pub fn still_water_layer_state(&self, terrain: Option<&TerrainId>, id: &str) -> (bool, bool) {
        self.scene_layers.still_water.source(terrain).map_or((true, false), |source|
            (!source.disabled.contains(id), source.locked.contains(id)))
    }

    /// 运行时可以按来源借用独立参与集，兄弟来源和锁定位改变时沿用原身份。
    pub fn still_water_disabled_layers(&self, terrain: Option<&TerrainId>) -> Option<&Arc<BTreeSet<String>>> {
        let disabled = &self.scene_layers.still_water.source(terrain)?.disabled;
        (!disabled.is_empty()).then_some(disabled)
    }

    /// 修改指定来源的一项，恢复默认时自动删去稀疏记录。
    pub fn set_still_water_layer_state(&mut self, terrain: Option<&TerrainId>, id: &str, enabled: bool, locked: bool) {
        if self.still_water_layer_state(terrain, id) == (enabled, locked) { return; }
        let settings = Arc::make_mut(&mut self.scene_layers.still_water);
        if let Some(terrain) = terrain {
            let source = settings.terrains.entry(terrain.clone()).or_default();
            source.set(id, enabled, locked);
            if source.is_empty() { settings.terrains.remove(terrain); }
        } else {
            settings.root.set(id, enabled, locked);
        }
    }

    /// 通过共享快照发布作者显隐；关闭或交接时归还正式清单。
    pub fn set_still_water_preview_state(
        world: &mut World,
        draft: Option<&SceneManifestAsset>,
        hidden: Arc<BTreeMap<Option<TerrainId>, BTreeSet<String>>>,
    ) {
        let Some(draft) = draft else { world.remove_resource::<SceneStillWaterPreviewState>(); return; };
        let settings = &draft.scene_layers.still_water;
        if world.get_resource::<SceneStillWaterPreviewState>().is_some_and(|old|
            old.scene == draft.id && Arc::ptr_eq(&old.settings, settings) && Arc::ptr_eq(&old.hidden, &hidden)) { return; }
        world.insert_resource(SceneStillWaterPreviewState { scene: draft.id.clone(), settings: settings.clone(), hidden });
    }
}
