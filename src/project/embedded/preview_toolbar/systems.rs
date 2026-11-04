//! 三个按钮的局部状态与显示租约；复用模型三角线框、Avian调试组和原模式请求。

use super::{components::*, resources::*, ui};
use crate::project::embedded::EmbeddedProjectState;
use bevy::{input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseButtonInput,
    MouseMotion, MouseWheel}, prelude::*};
use feathrise_editor_protocol::Mode;
use feathrise_scene::author_view::{ShadingView, apply_shading};
use feathrise_scene_core::view::PrimarySceneView;

fn display_state(world: &World) -> DisplayBaseline {
    DisplayBaseline {
        collision: world.get_resource::<bevy::gizmos::config::GizmoConfigStore>()
            .and_then(|store| store.get_config::<avian3d::debug_render::PhysicsGizmos>()
                .map(|(config, _)| config.enabled)),
    }
}

fn apply_display(world: &mut World, value: DisplayBaseline) {
    if let Some(enabled) = value.collision
        && let Some(mut store) = world.get_resource_mut::<bevy::gizmos::config::GizmoConfigStore>()
        && store.get_config::<avian3d::debug_render::PhysicsGizmos>()
            .is_some_and(|(config, _)| config.enabled != enabled) {
        store.config_mut::<avian3d::debug_render::PhysicsGizmos>().0.enabled = enabled;
    }
}

pub(in crate::project::embedded) fn mode_changed(world: &mut World, mode: Mode) {
    let Some(mut state) = world.remove_resource::<PreviewToolbarState>() else { return; };
    match mode {
        Mode::Playing if state.baseline.is_none() => {
            let baseline = display_state(world);
            // 每次预览默认关闭模型线框；无限参考网格属于编辑视图，不由本按钮安装。
            state.grid = false;
            state.collision = baseline.collision.unwrap_or(false);
            state.baseline = Some(baseline);
        }
        Mode::Editing => {
            release_mesh_wire(world, &mut state);
            if let Some(baseline) = state.baseline.take() { apply_display(world, baseline); }
            state.return_requested = false;
            if let Some(mut pointer) = world.get_resource_mut::<ToolbarPointerInput>() { *pointer = default(); }
        }
        _ => {}
    }
    world.insert_resource(state);
}

pub(in crate::project::embedded) fn take_return_request(world: &mut World) -> bool {
    world.get_resource_mut::<PreviewToolbarState>()
        .is_some_and(|mut state| std::mem::take(&mut state.return_requested))
}

pub(super) fn interact(world: &mut World,
    mut buttons: Local<Option<QueryState<(&ToolbarAction, &Interaction), Changed<Interaction>>>>,
) {
    if world.resource::<EmbeddedProjectState>().mode != Mode::Playing { return; }
    let actions: Vec<_> = buttons.get_or_insert_with(||world.query_filtered::<
        (&ToolbarAction, &Interaction), Changed<Interaction>>()).iter(world)
        .filter_map(|(action, interaction)| (*interaction == Interaction::Pressed).then_some(*action))
        .take(3).collect();
    let Some(mut state) = world.remove_resource::<PreviewToolbarState>() else { return; };
    if state.baseline.is_some() {
        for action in actions {
            match action {
                ToolbarAction::Return => state.return_requested = true,
                ToolbarAction::Grid => state.grid = !state.grid,
                ToolbarAction::Collision => state.collision = !state.collision,
            }
        }
        apply_display(world, DisplayBaseline { collision: Some(state.collision) });
    }
    world.insert_resource(state);
}

fn release_mesh_wire(world: &mut World, state: &mut PreviewToolbarState) {
    if let Some((entity, previous)) = state.mesh_camera.take() {
        apply_shading(world, entity, previous);
    }
}

pub(super) fn synchronize_mesh_wire(world: &mut World,
    mut cameras: Local<Option<QueryState<Entity, With<PrimarySceneView>>>>,
) {
    let Some(mut state) = world.remove_resource::<PreviewToolbarState>() else { return; };
    let playing = world.resource::<EmbeddedProjectState>().mode == Mode::Playing;
    let camera = if playing && state.grid {
        cameras.get_or_insert_with(||world.query_filtered::<Entity, With<PrimarySceneView>>()).single(world).ok()
    } else { None };
    if state.mesh_camera.as_ref().map(|(entity, _)| *entity) != camera { release_mesh_wire(world, &mut state); }
    if let Some(entity) = camera {
        if state.mesh_camera.is_none() { state.mesh_camera = Some((entity, world.get::<ShadingView>(entity).copied())); }
        // 共享draw直接派生当前Mesh管线，保留Clipmap位移/裁切；不遍历模型或复制三角线资产。
        apply_shading(world, entity, Some(ShadingView::mesh_triangles()));
    }
    world.insert_resource(state);
}

pub(super) fn present(
    project: Res<EmbeddedProjectState>,
    state: Res<PreviewToolbarState>,
    mut roots: Query<(&mut Node, &mut UiTargetCamera), With<ToolbarRoot>>,
    primary: Query<(Entity, &Camera), (With<PrimarySceneView>, Without<ToolbarCamera>)>,
    mut cameras: Query<(Entity, &mut Camera), With<ToolbarCamera>>,
    mut buttons: Query<(&ToolbarAction, &Interaction, &mut BackgroundColor)>,
) {
    let playing = project.mode == Mode::Playing;
    let primary = primary.single().ok().filter(|(_, camera)| camera.is_active).map(|(entity, _)| entity);
    // 正常预览直接附着主视图，让UI与世界共用HDR/色调映射和最终目标。
    // 独立LDR相机即使clear=None，也会因不同ViewTarget覆盖HDR世界；后备相机只能在主视图缺席时启用。
    let fallback = cameras.single_mut().ok().map(|(entity, mut camera)| {
        camera.is_active = playing && primary.is_none();
        entity
    });
    let target = primary.or(fallback);
    for (mut node, mut camera) in &mut roots {
        node.display = if playing && target.is_some() { Display::Flex } else { Display::None };
        if let Some(target) = target && camera.0 != target { camera.0 = target; }
    }
    for (action, interaction, mut background) in &mut buttons {
        let selected = match action {
            ToolbarAction::Return => false,
            ToolbarAction::Grid => state.grid,
            ToolbarAction::Collision => state.collision,
        };
        let color = match (*interaction, selected) {
            (Interaction::Pressed, true) => Color::srgba(0.27, 0.52, 0.69, 1.0),
            (Interaction::Hovered, true) => Color::srgba(0.23, 0.45, 0.60, 0.98),
            (_, true) => ui::SELECTED,
            (Interaction::Pressed, false) => Color::srgba(0.31, 0.38, 0.47, 1.0),
            (Interaction::Hovered, false) => ui::HOVERED,
            _ => ui::NORMAL,
        };
        background.set_if_neq(BackgroundColor(color));
    }
}

pub(super) fn restore_pointer(
    project: Res<EmbeddedProjectState>,
    mut pointer: ResMut<ToolbarPointerInput>,
    mouse: Option<ResMut<ButtonInput<MouseButton>>>,
) {
    let saved = pointer.saved.take();
    if project.mode != Mode::Playing { pointer.capturing = false; return; }
    if let (Some(saved), Some(mut mouse)) = (saved, mouse) { *mouse = saved; }
}

pub(super) fn capture_pointer(
    project: Res<EmbeddedProjectState>,
    mut pointer: ResMut<ToolbarPointerInput>,
    targets: Query<&Interaction, With<ToolbarPointer>>,
    mouse: Option<ResMut<ButtonInput<MouseButton>>>,
    motion: Option<ResMut<AccumulatedMouseMotion>>,
    scroll: Option<ResMut<AccumulatedMouseScroll>>,
    buttons: Option<ResMut<Messages<MouseButtonInput>>>,
    motions: Option<ResMut<Messages<MouseMotion>>>,
    wheels: Option<ResMut<Messages<MouseWheel>>>,
) {
    if project.mode != Mode::Playing
        || (!pointer.capturing && !targets.iter().any(|interaction| *interaction != Interaction::None)) { return; }
    // UiFocus已经消费本帧输入；原值移动到小型资源，玩法只见空值。
    // 不能永久reset_all，否则真实WM_LBUTTONUP无法产生just_released，按钮会一直Pressed而拒绝第二次点击。
    // 按住移出工具条仍持续占用到真实松开，避免拖出时把UI点击的held状态交给玩法。
    if let Some(mut mouse) = mouse {
        pointer.capturing = mouse.get_pressed().next().is_some();
        pointer.saved = Some(std::mem::take(&mut *mouse));
    }
    if let Some(mut motion) = motion { motion.delta = Vec2::ZERO; }
    if let Some(mut scroll) = scroll { *scroll = default(); }
    if let Some(mut buttons) = buttons { buttons.clear(); }
    if let Some(mut motions) = motions { motions.clear(); }
    if let Some(mut wheels) = wheels { wheels.clear(); }
}
