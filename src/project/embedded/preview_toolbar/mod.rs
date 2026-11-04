//! 工程预览专用工具条：真实游戏窗口拥有按钮，返回复用模式事务，调试显示只租用当前预览。

mod components;
mod resources;
mod systems;
mod ui;
mod hud;
#[cfg(feature = "validation")]
mod validation;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod gpu_tests;

use bevy::prelude::*;

pub(super) fn install(app: &mut App) {
    hud::install(app);
    app.init_resource::<resources::PreviewToolbarState>()
        .init_resource::<resources::ToolbarPointerInput>()
        .add_systems(Startup, ui::spawn)
        .add_systems(PreUpdate, systems::restore_pointer.before(bevy::input::InputSystems))
        // UI 已消费点击之后、原生自由相机和玩法读取之前，阻止工具条鼠标穿透。
        .add_systems(PreUpdate, systems::capture_pointer.after(bevy::ui::UiSystems::Focus))
        .add_systems(Update, systems::interact.before(super::systems::drive))
        .add_systems(Update, systems::present.after(super::systems::drive))
        .add_systems(PostUpdate, systems::synchronize_mesh_wire.after(super::author_view::sync)
            .before(feathrise_settings::GraphicsRuntimeLeaseSystems::RestoreReleased)
            .before(bevy::transform::TransformSystems::Propagate));
}

pub(super) use systems::{mode_changed, take_return_request};
#[cfg(feature = "validation")]
pub(super) use validation::{RuntimeEvidence, RuntimeEvidenceQuery, runtime_evidence};

/// 显式验收只读取已完成的真实布局，不根据预设按钮尺寸伪造点击坐标。
#[cfg(feature = "validation")]
pub(super) fn evidence(world: &mut World) -> serde_json::Value {
    use components::*;
    let state = world.resource::<resources::PreviewToolbarState>();
    let grid = state.grid;
    let collision = state.collision;
    let active = state.baseline.is_some();
    let runtime = runtime_evidence(world);
    let physical_rect = runtime.hud_rect;
    let root_visible = world.query_filtered::<(&Node, &InheritedVisibility), With<ToolbarRoot>>()
        .iter(world).any(|(node, visible)| node.display != Display::None && visible.get());
    let target_camera = world.query_filtered::<&UiTargetCamera, With<ToolbarRoot>>()
        .single(world).ok().map(|camera| camera.0);
    let camera_active = target_camera.and_then(|entity| world.get::<Camera>(entity))
        .is_some_and(|camera| camera.is_active);
    let buttons: Vec<_> = world.query::<(&ToolbarAction, &ComputedNode, &UiGlobalTransform, &InheritedVisibility)>()
        .iter(world).take(3).map(|(action, node, transform, visible)| {
            let label = match action { ToolbarAction::Return => "返回", ToolbarAction::Grid => "显示网格",
                ToolbarAction::Collision => "显示碰撞网格" };
            serde_json::json!({"label":label,"physical_center":transform.translation.to_array(),
                "physical_size":node.size().to_array(),"visible":visible.get() && !node.is_empty()})
        }).collect();
    serde_json::json!({"source":"production-Bevy-UI-ComputedNode","active":active,
        "root_visible":root_visible,"camera_active":camera_active,"target_camera":runtime.target_camera,
        "fallback_camera_active":runtime.fallback_camera_active,"grid":grid,
        "collision":collision,"buttons":buttons,"runtime":runtime,"physical_rect":physical_rect,"f12_bound":false})
}
