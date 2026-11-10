//! 按全局素材预设把共享 F3/F4 LOD 状态应用到通用 Mesh 节点。
//! 新节点在首次可见性传播前继承状态，稳定帧不扫描已有节点。

use bevy::prelude::*;
use feathrise_environment::vegetation::VegetationMeshRenderIdentity;
use feathrise_scene_content::VegetationPresetRegistry;
use super::LodControlPresetBindings;

use super::apply::write_lod_visibility;
use super::state::{LodControlContent, LodControlState};
use crate::resource_monitor::mesh::preset_prototype;

pub(in crate::resource_monitor) fn install_managed_mesh_lod_controls(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (apply_changed, apply_new)
            .chain()
            .in_set(super::LodControlApplySet)
            .before(TransformSystems::Propagate)
            .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
    );
}

fn content(
    identity: &VegetationMeshRenderIdentity,
    presets: Option<&VegetationPresetRegistry>,
    bindings: Option<&LodControlPresetBindings>,
) -> Option<LodControlContent> {
    bindings?.iter()
    .find_map(|(content, id)| {
        (preset_prototype(presets, id) == Some(identity.prototype)).then_some(content)
    })
}

fn synchronize(
    state: &LodControlState,
    presets: Option<&VegetationPresetRegistry>,
    bindings: Option<&LodControlPresetBindings>,
    identity: &VegetationMeshRenderIdentity,
    visibility: &mut Visibility,
) {
    let Some(content) = content(identity, presets, bindings) else {
        return;
    };
    let level = usize::from(identity.level.0);
    if level < 4 {
        write_lod_visibility(state.is_enabled(content, level), visibility);
    }
}

fn apply_changed(
    state: Res<LodControlState>,
    presets: Option<Res<VegetationPresetRegistry>>,
    bindings: Option<Res<LodControlPresetBindings>>,
    mut previous_revision: Local<Option<u64>>,
    mut previous_targets: Local<super::targets::Targets>,
    mut had_bindings: Local<bool>,
    mut nodes: Query<(&VegetationMeshRenderIdentity, &mut Visibility)>,
) {
    let revision = presets.as_deref().map(|presets| presets.revision);
    let removed = *had_bindings && bindings.is_none();
    *had_bindings = bindings.is_some();
    if !removed && !state.is_changed() && *previous_revision == revision
        && bindings.as_ref().is_none_or(|bindings| !bindings.is_changed()) {
        return;
    }
    *previous_revision = revision;
    let targets = super::targets::Targets::resolve(presets.as_deref(), bindings.as_deref());
    for (identity, mut visibility) in &mut nodes {
        if usize::from(identity.level.0) < super::LOD_LEVEL_COUNT
            && previous_targets.mesh(identity.prototype.0).is_some() && targets.mesh(identity.prototype.0).is_none() {
            write_lod_visibility(true, &mut visibility);
        } // 旧预设解绑/目录移除后归还原型，不让不可见分类的旧掩码继续隐藏画面。
        synchronize(&state, presets.as_deref(), bindings.as_deref(), identity, &mut visibility);
    }
    *previous_targets = targets;
}

fn apply_new(
    state: Res<LodControlState>,
    presets: Option<Res<VegetationPresetRegistry>>,
    bindings: Option<Res<LodControlPresetBindings>>,
    mut nodes: Query<
        (&VegetationMeshRenderIdentity, &mut Visibility),
        Added<VegetationMeshRenderIdentity>,
    >,
) {
    for (identity, mut visibility) in &mut nodes {
        synchronize(&state, presets.as_deref(), bindings.as_deref(), identity, &mut visibility);
    }
}
