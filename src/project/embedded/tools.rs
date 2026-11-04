//! 工程按工具语义生成三维预览；输入、作者文档和撤销事务仍留在编辑器。

use super::{EmbeddedProjectState, resources::Runtime};
use bevy::{gizmos::config::GizmoConfigStore, prelude::*};
use feathrise_editor_protocol::{AuthorToolPreview, Mode};

#[derive(Default, Reflect, GizmoConfigGroup)]
pub(super) struct AuthorToolsGizmos;

#[derive(Default, Reflect, GizmoConfigGroup)]
pub(super) struct AuthorSpatialToolsGizmos;

pub(super) fn install(app: &mut App) {
    app.init_gizmo_group::<AuthorToolsGizmos>()
        .init_gizmo_group::<AuthorSpatialToolsGizmos>()
        .add_systems(Startup, configure)
        .add_systems(PostUpdate, draw);
}

fn configure(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<AuthorToolsGizmos>();
    feathrise_scene::author_view::tools::configure(config, false);
    let (config, _) = store.config_mut::<AuthorSpatialToolsGizmos>();
    feathrise_scene::author_view::tools::configure(config, true);
}

pub(super) fn visible<'a>(state: &EmbeddedProjectState, runtime: &'a Runtime) -> Option<&'a AuthorToolPreview> {
    let (revision, preview) = runtime.author_tools.as_ref()?;
    allowed(state, runtime.pending.is_some(), runtime.viewport.visible, *revision).then_some(preview)
}

fn allowed(state: &EmbeddedProjectState, pending: bool, visible: bool, revision: u64) -> bool {
    state.mode == Mode::Editing && state.revision != 0 && !pending && visible && revision == state.revision
}

fn draw(runtime: NonSend<Runtime>, state: Res<EmbeddedProjectState>,
    mut gizmos: Gizmos<AuthorToolsGizmos>, mut spatial: Gizmos<AuthorSpatialToolsGizmos>) {
    if let Some(preview) = visible(&state, &runtime) {
        feathrise_scene::author_view::tools::draw_layer(&mut gizmos, preview, false);
        feathrise_scene::author_view::tools::draw_layer(&mut spatial, preview, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_hidden_view_and_replacing_scene_never_draw_old_tools() {
        let mut state = EmbeddedProjectState { mode: Mode::Editing, revision: 7, world_id: 11, transitioning: false };
        assert!(allowed(&state, false, true, 7));
        for (pending, visible, revision) in [(true, true, 7), (false, false, 7), (false, true, 6), (false, true, 8)] {
            assert!(!allowed(&state, pending, visible, revision));
        }
        state.mode = Mode::Playing;
        assert!(!allowed(&state, false, true, 7));
        state.mode = Mode::Editing;
        assert!(allowed(&state, false, true, 7), "返回编辑恢复原工具状态");
        state.revision = 0;
        assert!(!allowed(&state, false, true, 0));
    }
}
