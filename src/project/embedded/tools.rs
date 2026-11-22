//! 工程按工具语义生成三维预览；输入、作者文档和撤销事务仍留在编辑器。

use super::{EmbeddedProjectState, resources::Runtime};
use bevy::{gizmos::config::GizmoConfigStore, prelude::*};
use feathrise_editor_protocol::{AuthorTool, AuthorToolPreview, Mode, TransformPreview};
use feathrise_scene::author_view::tools as render_tools;

#[derive(Default, Reflect, GizmoConfigGroup)]
pub(super) struct AuthorToolsGizmos;

#[derive(Default, Reflect, GizmoConfigGroup)]
pub(super) struct AuthorSpatialToolsGizmos;

#[derive(Default, Reflect, GizmoConfigGroup)]
pub(super) struct AuthorTransformGizmos;

pub(super) fn install(app: &mut App) {
    app.init_gizmo_group::<AuthorToolsGizmos>()
        .init_gizmo_group::<AuthorSpatialToolsGizmos>()
        .init_gizmo_group::<AuthorTransformGizmos>()
        .add_systems(Startup, configure)
        .add_systems(PostUpdate, draw.after(super::author_view::sync));
}

fn configure(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<AuthorToolsGizmos>();
    render_tools::configure(config, false);
    let (config, _) = store.config_mut::<AuthorSpatialToolsGizmos>();
    render_tools::configure(config, true);
    let (config, _) = store.config_mut::<AuthorTransformGizmos>();
    render_tools::configure(config, false);
    config.depth_bias = -1.0; // 普通变换手柄贴近裁剪面绘制，保留原世界坐标和拾取姿态。
}

pub(super) fn visible<'a>(state: &EmbeddedProjectState, runtime: &'a Runtime) -> Option<&'a AuthorToolPreview> {
    let (revision, preview) = runtime.author_tools.as_ref()?;
    allowed(state, runtime.pending.is_some(), runtime.viewport.visible, *revision).then_some(preview)
}

fn allowed(state: &EmbeddedProjectState, pending: bool, visible: bool, revision: u64) -> bool {
    state.mode == Mode::Editing && state.revision != 0 && !pending && visible && revision == state.revision
}

fn draw(runtime: NonSend<Runtime>, state: Res<EmbeddedProjectState>,
    mut gizmos: Gizmos<AuthorToolsGizmos>, mut spatial: Gizmos<AuthorSpatialToolsGizmos>,
    mut transforms: Gizmos<AuthorTransformGizmos>) {
    if let Some(preview) = visible(&state, &runtime) {
        for (index, item) in preview.items.iter().enumerate() {
            match item {
                AuthorTool::Brush(brush) if brush.spatial => render_tools::draw_brush(&mut spatial, brush),
                AuthorTool::Brush(brush) => render_tools::draw_brush(&mut gizmos, brush),
                AuthorTool::Transform(handle @ TransformPreview::Move { spatial: true, .. }) => {
                    render_tools::draw_transform(&mut spatial, handle);
                }
                AuthorTool::Transform(handle) => render_tools::draw_transform(&mut transforms, handle),
                // 相机已提供的内置对象轮廓无需再画选框，其余选择沿用原深度。
                AuthorTool::Selection(selection) if Some(index) != runtime.outlined_selection => {
                    render_tools::draw_selection(&mut gizmos, selection);
                }
                AuthorTool::Selection(_) => {}
            }
        }
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
