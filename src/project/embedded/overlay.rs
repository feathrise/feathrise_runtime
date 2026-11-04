//! 作者选框和操作手柄只传递有界线段，由真实项目相机绘制，跨修订或进入试玩即隐藏。

use super::{EmbeddedProjectState, resources::Runtime};
use bevy::{gizmos::config::GizmoConfigStore, prelude::*};
use feathrise_editor_protocol::Mode;

/// 使用独立配置，不能改变工程原有调试 Gizmo 的线宽或深度。
#[derive(Default, Reflect, GizmoConfigGroup)]
struct AuthorOverlay;

pub(super) fn install(app: &mut App) {
    app.init_gizmo_group::<AuthorOverlay>()
        .add_systems(Startup, configure)
        .add_systems(PostUpdate, draw);
}

fn configure(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<AuthorOverlay>();
    config.line.width = 3.0;
    config.line.perspective = false;
    config.depth_bias = -1.0; // 手柄与选框须越过场景深度；仅影响显示，不改作者拾取坐标。
}

fn draw(
    runtime: NonSend<Runtime>,
    state: Res<EmbeddedProjectState>,
    mut gizmos: Gizmos<AuthorOverlay>,
) {
    if state.mode != Mode::Editing || state.revision == 0 || runtime.pending.is_some() {
        return;
    }
    let Some((revision, lines)) = &runtime.overlay else {
        return;
    };
    if *revision != state.revision {
        return;
    }
    for line in lines {
        gizmos.line_gradient(
            Vec3::from_array(line.start),
            Vec3::from_array(line.end),
            Color::linear_rgba(
                line.start_color[0],
                line.start_color[1],
                line.start_color[2],
                line.start_color[3],
            ),
            Color::linear_rgba(
                line.end_color[0],
                line.end_color[1],
                line.end_color[2],
                line.end_color[3],
            ),
        );
    }
}
