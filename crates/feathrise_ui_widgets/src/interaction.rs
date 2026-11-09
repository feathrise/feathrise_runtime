//! 从 Bevy 现代拾取状态局部投影共享 UI 三态，保留左键、遮挡和 Changed 语义。

use crate::{UiInteraction, UiInteractionSystems, components::UiInteractionCancelled, resources::UiInteractionTargets};
use bevy::{
    ecs::entity::EntityHashSet,
    picking::{
        events::{PointerCancel, PointerPress, PointerRelease},
        hover::{HoverMap, Hovered},
        pointer::{PointerButton, PointerId},
        PickingSystems,
    },
    prelude::*,
    ui::{InteractionDisabled, Pressed},
};

/// 单独使用三态节点的应用也可安装此入口；共享控件插件会幂等安装它。
pub struct UiInteractionPlugin;

impl Plugin for UiInteractionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiInteractionTargets>()
            .configure_sets(PreUpdate, UiInteractionSystems::Sync.after(PickingSystems::Last))
            .add_observer(press)
            .add_observer(release)
            .add_observer(cancel)
            .add_systems(PreUpdate, synchronize.in_set(UiInteractionSystems::Sync)
                // 无拾取层的精简 App 不伪造命中，仍可显式驱动自身的三态业务。
                .run_if(resource_exists::<HoverMap>));
    }
}

fn press(
    mut event: On<PointerPress>,
    targets: Query<Has<InteractionDisabled>, With<UiInteraction>>,
    mut commands: Commands,
) {
    if event.button != PointerButton::Primary { return; }
    if let Ok(disabled) = targets.get(event.entity) {
        // 子文字可冒泡到最近控件；已经命中的控件不再把按下交给祖先背景。
        event.propagate(false);
        if !disabled { commands.entity(event.entity).remove::<UiInteractionCancelled>().insert(Pressed); }
    }
}

fn release(
    mut event: On<PointerRelease>,
    targets: Query<(), With<UiInteraction>>,
    mut commands: Commands,
) {
    if event.button == PointerButton::Primary && targets.contains(event.entity) {
        event.propagate(false);
        commands.entity(event.entity).remove::<(Pressed, UiInteractionCancelled)>();
    }
}

fn cancel(
    mut event: On<PointerCancel>,
    targets: Query<(), With<UiInteraction>>,
    mut commands: Commands,
) {
    if targets.contains(event.entity) {
        event.propagate(false);
        commands.entity(event.entity).remove::<Pressed>().insert(UiInteractionCancelled);
    }
}

type Hierarchy<'a> = (
    Option<&'a ChildOf>, Option<&'a Node>, Option<&'a Visibility>,
    Option<&'a InheritedVisibility>, Has<InteractionDisabled>, Has<UiInteraction>,
);

/// Hovered 含子孙命中；只采用最近控件，避免底层窗口和祖先按钮一起接收一次松开。
fn nearest_target(mut entity: Entity, hierarchy: &Query<Hierarchy>) -> Option<Entity> {
    for _ in 0..128 {
        let Ok((parent, _, _, _, _, target)) = hierarchy.get(entity) else { return None; };
        if target { return Some(entity); }
        entity = parent?.parent();
    }
    None
}

fn enabled(mut entity: Entity, hierarchy: &Query<Hierarchy>) -> bool {
    for _ in 0..128 {
        let Ok((parent, node, visibility, inherited, disabled, _)) = hierarchy.get(entity) else { return false; };
        if disabled || node.is_some_and(|node| node.display == Display::None)
            || visibility == Some(&Visibility::Hidden) || inherited.is_some_and(|visible| !visible.get()) {
            return false;
        }
        let Some(parent) = parent else { return true; };
        entity = parent.parent();
    }
    false
}

fn synchronize(
    hover: Res<HoverMap>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    windows: Query<&Window>,
    hierarchy: Query<Hierarchy>,
    sources: Query<(&Hovered, Has<Pressed>, Has<UiInteractionCancelled>), With<UiInteraction>>,
    mut projection: ParamSet<(
        Query<Entity, (With<UiInteraction>, Or<(
            Added<UiInteraction>, Changed<Hovered>, Added<Pressed>, Added<InteractionDisabled>, Added<UiInteractionCancelled>,
            Changed<Node>, Changed<Visibility>, Changed<InheritedVisibility>,
        )>)>,
        Query<&mut UiInteraction>,
    )>,
    mut removed_pressed: RemovedComponents<Pressed>,
    mut targets: ResMut<UiInteractionTargets>,
    mut commands: Commands,
) {
    let focused = windows.is_empty() || windows.iter().any(|window| window.focused);
    let current: EntityHashSet = if focused {
        hover.get(&PointerId::Mouse).into_iter().flat_map(|hits| hits.keys())
            .filter_map(|&entity| nearest_target(entity, &hierarchy)).collect()
    } else { EntityHashSet::default() };
    // 只访问当轮/上轮命中及真正改变来源的节点；稳定帧最多复核当前少量控件。
    let mut dirty = current.clone();
    dirty.extend(targets.hovered.iter().copied());
    // Added 的 tick 读取也属于组件只读访问，须与三态写入分时借用。
    dirty.extend(projection.p0().iter());
    dirty.extend(removed_pressed.read());
    let mut interaction = projection.p1();
    for entity in dirty {
        let Ok((hovered, was_pressed, cancelled)) = sources.get(entity) else { continue; };
        let held = mouse.as_ref().map_or(was_pressed, |mouse| mouse.pressed(MouseButton::Left));
        let enabled = focused && enabled(entity, &hierarchy);
        let cancelled = if cancelled && !held {
            commands.entity(entity).remove::<UiInteractionCancelled>();
            false
        } else { cancelled };
        if was_pressed && held && !enabled { commands.entity(entity).insert(UiInteractionCancelled); }
        let hovered = current.contains(&entity) && hovered.get() && enabled && !cancelled;
        let pressed = hovered && held;
        if pressed != was_pressed {
            if pressed { commands.entity(entity).insert(Pressed); }
            else { commands.entity(entity).remove::<Pressed>(); }
        }
        // 同帧按下又松开的旧三态仍呈现一次 Pressed，原按钮捕获才能核对快速点击；
        // 官方 Pressed 标记只反映实际按住，下一帧自然回到 Hovered。
        let press_edge = mouse.as_ref().is_some_and(|mouse| mouse.just_pressed(MouseButton::Left));
        let next = if hovered && (pressed || press_edge) { UiInteraction::Pressed }
            else if hovered { UiInteraction::Hovered } else { UiInteraction::None };
        if let Ok(mut state) = interaction.get_mut(entity) { state.set_if_neq(next); }
    }
    targets.hovered = current;
}

#[cfg(test)]
mod tests;
