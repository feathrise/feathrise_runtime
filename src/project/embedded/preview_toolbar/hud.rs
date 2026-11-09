//! 悬浮HUD位置只属于当前进程；原生Pointer拖拽抓手，不改相机视口、作者文档或工程偏好。

use feathrise_ui_widgets::UiInteraction;
use super::{components::*, resources::ToolbarPointerInput, ui};
use crate::project::embedded::EmbeddedProjectState;
use bevy::{
    picking::{events::{PointerDrag, PointerDragEnd, PointerDragStart}, pointer::{PointerButton, PointerId}},
    prelude::*, window::PrimaryWindow,
};
use feathrise_editor_protocol::Mode;

#[derive(Resource)]
pub(super) struct ToolbarHudState {
    pub position: Vec2,
    manually_positioned: bool,
    dragging: Option<(PointerId, Vec2)>,
}

impl Default for ToolbarHudState {
    fn default() -> Self { Self { position: Vec2::splat(12.0), manually_positioned: false, dragging: None } }
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<ToolbarHudState>()
        .add_systems(PostUpdate, synchronize.after(bevy::camera::CameraUpdateSystems)
            .before(bevy::ui::UiSystems::Prepare));
}

pub(super) fn drag_start(
    event: On<PointerDragStart>,
    project: Res<EmbeddedProjectState>,
    handles: Query<(), With<ToolbarDragHandle>>,
    mut hud: ResMut<ToolbarHudState>,
) {
    if project.mode == Mode::Playing && event.button == PointerButton::Primary
        && handles.contains(event.entity) {
        hud.manually_positioned = true;
        hud.dragging = Some((event.pointer.id, hud.position));
    }
}

pub(super) fn drag(
    event: On<PointerDrag>,
    project: Res<EmbeddedProjectState>,
    scale: Option<Res<UiScale>>,
    mut hud: ResMut<ToolbarHudState>,
) {
    if project.mode != Mode::Playing || event.button != PointerButton::Primary || !event.distance.is_finite() { return; }
    if let Some((_, origin)) = hud.dragging.filter(|(pointer, _)| *pointer == event.pointer.id) {
        // Pointer位置来自Window.cursor_position，已是窗口逻辑像素；仅除UI缩放，不能再次除显示器DPI。
        let scale = scale.as_deref().map_or(1.0, |scale| scale.0).max(0.01);
        hud.position = origin + event.distance / scale;
    }
}

pub(super) fn drag_end(event: On<PointerDragEnd>, mut hud: ResMut<ToolbarHudState>) {
    if event.button == PointerButton::Primary
        && hud.dragging.is_some_and(|(pointer, _)| pointer == event.pointer.id) {
        hud.dragging = None;
    }
}

fn bounded(position: Vec2, panel: Vec2, viewport: Vec2) -> Vec2 {
    // 正常尺寸完整留在客户区；极小窗口仍从左上保住抓手，避免拖到屏幕外后无法找回。
    let room = (viewport - panel).max(Vec2::ZERO);
    let margin = room.min(Vec2::splat(6.0));
    position.clamp(margin, (room - margin).max(margin))
}

pub(super) fn synchronize(
    project: Res<EmbeddedProjectState>,
    scale: Option<Res<UiScale>>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<&Camera>,
    mut hud: ResMut<ToolbarHudState>,
    pointer: Option<ResMut<ToolbarPointerInput>>,
    mut roots: Query<(&mut Node, &ComputedNode, &UiTargetCamera, &mut UiTransform), With<ToolbarRoot>>,
    mut grips: Query<(&UiInteraction, &mut BackgroundColor), With<ToolbarDragHandle>>,
) {
    let focused = windows.single().map_or(true, |window| window.focused);
    if project.mode != Mode::Playing || !focused {
        hud.dragging = None;
        if let Some(mut pointer) = pointer { *pointer = default(); }
    } else if keys.as_deref().is_some_and(|keys| keys.just_pressed(KeyCode::Escape)) {
        // Esc停止位置变化；原工具条指针门仍持有按住状态，直到真正松开，不能把拖出变成玩法输入。
        hud.dragging = None;
    }
    let scale = scale.as_deref().map_or(1.0, |scale| scale.0).max(0.01);
    for (mut node, computed, target, mut transform) in &mut roots {
        let panel = computed.size() * computed.inverse_scale_factor;
        // 首次布局前保留UI自身的百分比居中，避免未知宽度造成第一帧从右侧跳回。
        if panel.x <= 0. { continue; }
        let viewport = cameras.get(target.0).ok().and_then(Camera::logical_viewport_size)
            .or_else(|| windows.single().ok().map(|window| window.resolution.size()));
        let mut centered = false;
        if let Some(viewport) = viewport.filter(|size| size.is_finite() && size.min_element() > 0.) {
            let viewport = viewport / scale;
            // 未拖动时随窗口保持顶部居中；第一次拖动后改用用户位置，预览往返不复位。
            centered = !hud.manually_positioned && panel.x <= viewport.x;
            let desired = if hud.manually_positioned { hud.position }
                else { Vec2::new(((viewport.x - panel.x) * 0.5).max(0.), 12.) };
            let mut position = bounded(desired, panel, viewport);
            if centered { position.x = desired.x; }
            if hud.position != position { hud.position = position; }
        }
        // 空闲时不持续标脏Node，否则没有移动HUD也会每帧唤醒整棵UI布局。
        let (left, translation) = if centered { (percent(50), Val2::percent(-50, 0)) }
            else { (px(hud.position.x), Val2::ZERO) };
        let top = px(hud.position.y);
        if node.left != left { node.left = left; }
        if node.top != top { node.top = top; }
        if transform.translation != translation { transform.translation = translation; }
    }
    for (interaction, mut background) in &mut grips {
        let color = if hud.dragging.is_some() { ui::SELECTED }
            else if *interaction != UiInteraction::None { ui::HOVERED } else { Color::NONE };
        background.set_if_neq(BackgroundColor(color));
    }
}

#[cfg(test)]
mod tests;
