//! 原作者工具的实时投影入口；背景解码、每域单飞和独立回执，不假装为完整场景就绪。

mod placement;
mod vegetation;
mod author_domain;
pub use author_domain::extension::{AuthorDomainExtension,AuthorDomainExtensionStatus,AuthorDomainExtensionScene};
mod surface_trail;

use bevy::prelude::*;
use feathrise_editor_protocol::{Message, Mode, PreviewKind};
use feathrise_scene_content::{ActiveDynamicScene, SceneId};

enum PreparedValue {
    Placement(feathrise_scene_content::embedded_placement_preview::PlacementPreviewCommand),
    Vegetation(vegetation::Prepared),
    AuthorDomain(author_domain::Prepared),
    SurfaceTrail(surface_trail::Prepared),
}
pub(super) struct PreparedPreview {
    pub generation: u64,
    pub sequence: u64,
    pub scene_id: SceneId,
    value: PreparedValue,
}

pub(super) fn prepare(kind: PreviewKind, sequence: u64, generation: u64, map: &str,
    format: &str, bytes: &[u8]) -> Result<PreparedPreview, String>
{
    let map = SceneId::parse(map)?;
    let value = match kind {
        PreviewKind::Placement => PreparedValue::Placement(placement::decode(format, bytes, &map)?),
        PreviewKind::Vegetation => PreparedValue::Vegetation(vegetation::prepare(format, bytes, &map)?),
        PreviewKind::AuthorDomain => PreparedValue::AuthorDomain(author_domain::prepare(format, bytes, &map)?),
        PreviewKind::SurfaceTrail => PreparedValue::SurfaceTrail(surface_trail::prepare(format, bytes, &map)?),
    };
    Ok(PreparedPreview { generation, sequence, scene_id: map, value })
}

#[derive(Default)]
struct DomainClock { waiting: Option<u64>, last: u64 }

pub(super) const KINDS: [PreviewKind; 4] = [PreviewKind::Placement, PreviewKind::Vegetation,
    PreviewKind::AuthorDomain, PreviewKind::SurfaceTrail];
pub(super) const fn index(kind: PreviewKind) -> usize {
    match kind { PreviewKind::Placement => 0, PreviewKind::Vegetation => 1,
        PreviewKind::AuthorDomain => 2, PreviewKind::SurfaceTrail => 3 }
}
pub(super) const fn kind_name(kind: PreviewKind) -> &'static str {
    match kind { PreviewKind::Placement => "placement", PreviewKind::Vegetation => "vegetation",
        PreviewKind::AuthorDomain => "author-domain", PreviewKind::SurfaceTrail => "surface-trail" }
}

#[derive(Default)]
pub(super) struct State {
    placement: placement::State,
    vegetation: vegetation::State,
    author_domain: author_domain::State,
    surface_trail: surface_trail::State,
    // 各域只有一个受控采用事务，可靠手势边沿不会被另一域高频像素覆盖。
    clocks: [DomainClock; 4],
    #[cfg(feature = "validation")]
    pub(super) applied: [u64; 4],
}

pub(super) fn install(app: &mut App) { placement::install(app); author_domain::install(app); }
pub(super) use placement::active_root;
impl State {
    #[cfg(feature = "validation")]
    pub(super) fn evidence(&self, world: &World, kind: PreviewKind) -> serde_json::Value {
        match kind {
            PreviewKind::Placement => self.placement.evidence(world),
            PreviewKind::Vegetation => self.vegetation.evidence(world),
            PreviewKind::AuthorDomain => self.author_domain.evidence(world),
            PreviewKind::SurfaceTrail => self.surface_trail.evidence(world),
        }
    }
    pub(super) fn clear_vegetation(&mut self, world: &mut World) -> Result<bool, String> { self.vegetation.clear_published(world) }
    pub(super) fn clear_water(&mut self, world: &mut World) -> Result<bool, String> { self.author_domain.clear_water(world) }
    pub(super) fn release_placement(&mut self, world: &mut World, entity: Entity) {
        self.placement.release_root(world, entity);
    }
}

pub(super) fn receive(world: &mut World, runtime: &mut super::resources::Runtime,
    preview: PreparedPreview) -> Result<(), String>
{
    if preview.generation != runtime.transport.identity.generation
        || world.resource::<ActiveDynamicScene>().scene_id.as_ref() != Some(&preview.scene_id)
        || world.resource::<super::EmbeddedProjectState>().mode != Mode::Editing
    {
        return Err("实时预览与当前编辑场景或会话代次不一致".into());
    }
    if runtime.pending.is_some() { return Err("完整快照尚未采用，不能插入实时预览".into()); }
    let kind = match &preview.value {
        PreparedValue::Placement(_) => PreviewKind::Placement,
        PreparedValue::Vegetation(_) => PreviewKind::Vegetation,
        PreparedValue::AuthorDomain(_) => PreviewKind::AuthorDomain,
        PreparedValue::SurfaceTrail(_) => PreviewKind::SurfaceTrail,
    };
    let clock = &runtime.previews.clocks[index(kind)];
    if preview.sequence <= clock.last { return Err(format!("{}预览域序号倒退", kind_name(kind))); }
    if clock.waiting.is_some() { return Err(format!("{}预览超过单飞预算", kind_name(kind))); }
    match preview.value {
        PreparedValue::Placement(value) => runtime.previews.placement.receive(world, &preview.scene_id, value)?,
        PreparedValue::Vegetation(value) => runtime.previews.vegetation.receive(value),
        PreparedValue::AuthorDomain(value) => runtime.previews.author_domain.receive(world, value)?,
        PreparedValue::SurfaceTrail(value) => runtime.previews.surface_trail.receive(world, value)?,
    }
    runtime.previews.clocks[index(kind)] = DomainClock { waiting:Some(preview.sequence), last:preview.sequence };
    Ok(())
}

pub(super) fn advance(world: &mut World, runtime: &mut super::resources::Runtime) -> Result<(), String> {
    for kind in KINDS {
        let complete = match kind {
            PreviewKind::Placement => runtime.previews.placement.advance(world)?,
            PreviewKind::Vegetation => runtime.previews.vegetation.advance(world)?,
            PreviewKind::AuthorDomain => runtime.previews.author_domain.advance(world, runtime.sources_settled, runtime.sources_ready)?,
            PreviewKind::SurfaceTrail => runtime.previews.surface_trail.advance(world)?,
        };
        if complete && let Some(sequence) = runtime.previews.clocks[index(kind)].waiting.take() {
            runtime.transport.send(Message::PreviewApplied { kind, sequence })?;
            #[cfg(feature = "validation")]
            { runtime.previews.applied[index(kind)] = sequence; }
        }
    }
    Ok(())
}

/// 完整来源接管以后，旧实时上传不得迟到覆盖新修订。已应用的作者Transform仍由正式采用接回。
pub(super) fn snapshot_started(world: &mut World, runtime: &mut super::resources::Runtime) -> Result<(), String> {
    if runtime.previews.clocks.iter().any(|clock|clock.waiting.is_some()) {
        return Err("完整快照与尚未采用的实时预览重叠".into());
    }
    runtime.previews.vegetation.cancel();
    runtime.previews.placement.snapshot_started(world);
    runtime.previews.author_domain.snapshot_started(world)?;
    runtime.previews.surface_trail.snapshot_started(world)?;
    Ok(())
}
