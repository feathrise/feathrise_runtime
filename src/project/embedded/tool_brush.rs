//! 工程笔刷只按稳定作者来源绑定本World已采用的Clipmap；作者修订与本地GPU票据绝不混用。

use super::{EmbeddedProjectState, resources::Runtime};
use bevy::prelude::*;
use feathrise_editor_protocol::{AuthorTool, BrushPreview};
use feathrise_map_content::{ActiveDynamicMap, MapId, TerrainId};
use feathrise_scene::{ClipmapOverlaySet, DynamicClipmapVegetationOverlay,
    TerrainAuthoringApply, TerrainObjectAuthoringTarget, TerrainObjectClipmapGround};
use std::collections::BTreeMap;

struct HeightBinding {
    path: String,
    author_revision: Option<u64>,
    ticket: u64,
}

/// 每个地形只保留当前完整快照及最新局部高度的一份映射，不积累历史或扫描运行实体。
#[derive(Resource, Default)]
pub(super) struct HeightSourceBindings {
    map: Option<MapId>,
    snapshot_revision: u64,
    values: BTreeMap<Option<TerrainId>, HeightBinding>,
}

impl HeightSourceBindings {
    pub(super) fn new(map: MapId, snapshot_revision: u64) -> Self {
        Self { map: Some(map), snapshot_revision, values: BTreeMap::new() }
    }

    /// 完整清单先声明所有合法稀疏源；空live稍后首次发布0修订时也能绑定，但声明本身不授信。
    pub(super) fn declare(&mut self, terrain: Option<TerrainId>, path: String) -> Result<(), String> {
        if self.values.contains_key(&terrain) { return Err("高度清单包含重复作者来源声明".into()); }
        self.values.insert(terrain, HeightBinding { path, author_revision: None, ticket: 0 });
        Ok(())
    }

    /// 仅后台完整快照准备调用；路径由同一清单解析，不能由笔刷消息补写来源信用。
    pub(super) fn insert(&mut self, terrain: Option<TerrainId>, path: String, author_revision: u64, ticket: u64) -> Result<(), String> {
        if let Some(binding) = self.values.get_mut(&terrain) {
            if binding.path != path || binding.author_revision.is_some() {
                return Err("高度快照包含重复或错配的作者来源映射".into());
            }
            binding.author_revision = Some(author_revision);
            binding.ticket = ticket;
        } else {
            self.values.insert(terrain, HeightBinding { path, author_revision: Some(author_revision), ticket });
        }
        Ok(())
    }

    /// 只有真实高度发布成功后更新一个键；外层快照变化会拒绝迟到的局部工作。
    pub(super) fn published_preview(&mut self, map: &MapId, terrain: Option<&TerrainId>, snapshot_revision: u64,
        author_revision: u64, ticket: u64) -> bool {
        if self.map.as_ref() != Some(map) || self.snapshot_revision != snapshot_revision { return false; }
        let Some(binding) = self.values.get_mut(&terrain.cloned()) else { return false; };
        binding.author_revision = Some(author_revision);
        binding.ticket = ticket;
        true
    }

    pub(super) fn resolve(&self, map: &MapId, terrain: Option<&TerrainId>, path: &str, snapshot_revision: u64, author_revision: u64) -> Option<u64> {
        if self.map.as_ref() != Some(map) || self.snapshot_revision != snapshot_revision { return None; }
        let binding = self.values.get(&terrain.cloned())?;
        (binding.path == path && binding.author_revision == Some(author_revision)).then_some(binding.ticket)
    }
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<HeightSourceBindings>()
        .add_systems(Update, select_target.after(super::systems::heights).before(TerrainAuthoringApply))
        .add_systems(PostUpdate, publish_brush.in_set(ClipmapOverlaySet::Prepare));
}

fn brush<'a>(state: &EmbeddedProjectState, runtime: &'a Runtime) -> Option<&'a BrushPreview> {
    if runtime.failed || runtime.pending_mode.is_some() { return None; }
    super::tools::visible(state, runtime)?.items.iter().find_map(|item| match item {
        AuthorTool::Brush(brush) if brush.clipmap.is_some() => Some(brush), _ => None,
    })
}

fn binding(state: &EmbeddedProjectState, active: &ActiveDynamicMap, bindings: &HeightSourceBindings,
    brush: &BrushPreview) -> Option<(MapId, Option<TerrainId>, u64)> {
    let surface = brush.clipmap.as_ref()?;
    let map = MapId::parse(&surface.map_id).ok()?;
    if active.map_id.as_ref() != Some(&map) { return None; }
    let terrain = surface.terrain_id.as_deref().map(TerrainId::parse).transpose().ok()?;
    let ticket = bindings.resolve(&map, terrain.as_ref(), &surface.height_path, state.revision, surface.authoring_height_revision)?;
    Some((map, terrain, ticket))
}

/// 在原作者资源交接前设置稳定对象；实例查询由原TerrainAuthoringApply负责，不能猜Entity。
fn select_target(state: Res<EmbeddedProjectState>, runtime: NonSend<Runtime>, active: Res<ActiveDynamicMap>,
    bindings: Res<HeightSourceBindings>, mut target: ResMut<TerrainObjectAuthoringTarget>) {
    let Some(brush) = brush(&state, &runtime) else { return; };
    let Some((map, terrain, _)) = binding(&state, &active, &bindings, brush) else { return; };
    if target.map.as_ref() != Some(&map) || target.terrain != terrain {
        target.map = Some(map);
        target.terrain = terrain;
    }
}

/// 每帧先撤回，再授予精确本地页信用；视口隐藏、模式交接、坏来源和GPU迟到都不能留下旧圈。
fn publish_brush(state: Res<EmbeddedProjectState>, runtime: NonSend<Runtime>, active: Res<ActiveDynamicMap>,
    bindings: Res<HeightSourceBindings>, ground: TerrainObjectClipmapGround,
    mut overlay: ResMut<DynamicClipmapVegetationOverlay>) {
    overlay.brush = None;
    let Some(brush) = brush(&state, &runtime) else { return; };
    let Some((map, terrain, ticket)) = binding(&state, &active, &bindings, brush) else { return; };
    let surface = brush.clipmap.as_ref().expect("已核对Clipmap来源");
    let Some(snapshot) = ground.authoring_snapshot(&map, terrain.as_ref(), &surface.height_path, ticket) else { return; };
    // 不比较或复制作者World的root Entity bits、环带修订和缺页信用；全部取本World本次真实采用。
    overlay.brush = local_brush(brush, snapshot.revision, snapshot.missing_pages);
}

fn local_brush(brush: &BrushPreview, revision: [u64; 3], missing_pages: [UVec4; 16])
    -> Option<feathrise_scene::ClipmapVegetationBrush> {
    feathrise_scene::author_view::tools::clipmap_brush(brush, revision, missing_pages)
}

#[cfg(test)]
mod tests;
