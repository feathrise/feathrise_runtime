//! 官方PointerInput保持WindowEvent先后与按下坐标；单笔固定状态兼容一帧内完整拖动。

use bevy::{prelude::*, ecs::system::SystemParam, camera::RenderTarget,
    picking::pointer::{Location, PointerAction, PointerButton, PointerId, PointerInput},
    ui::{FocusPolicy, UiStack, ComputedUiTargetCamera, OverrideClip}, window::PrimaryWindow};
use crate::{components::{NumericInputFieldButton, NumericInputDisabled}, types::NumericInputScrubButton};

const EVENT_BUDGET: usize = 256;
const HIT_BUDGET: usize = 8192;

#[derive(Default)]
struct Capture {
    root: Option<Entity>, field: Option<Entity>, button: Option<PointerButton>, location: Option<Location>,
    start_x: f32, excursion: f32,
}

#[derive(Default)]
pub(super) struct Frame {
    pub handled: bool,
    pub owned: bool,
    pub start: Option<(Entity, NumericInputScrubButton, f32)>,
    pub position: Option<f32>,
    pub released: bool,
    pub canceled: bool,
    pub excursion: f32,
}

#[derive(SystemParam)]
pub(in crate::systems) struct OrderedPointer<'w, 's> {
    events: MessageReader<'w, 's, PointerInput>,
    capture: Local<'s, Capture>,
    stack: Option<Res<'w, UiStack>>,
    nodes: Query<'w, 's, (&'static ComputedNode, &'static UiGlobalTransform,
        Option<&'static InheritedVisibility>, Option<&'static FocusPolicy>,
        Option<&'static ComputedUiTargetCamera>, Option<&'static UiTargetCamera>)>,
    clipping: Query<'w, 's, (&'static ComputedNode, &'static UiGlobalTransform, &'static Node)>,
    parents: Query<'w, 's, &'static ChildOf, Without<OverrideClip>>,
    fields: Query<'w, 's, &'static NumericInputFieldButton, Without<NumericInputDisabled>>,
    cameras: Query<'w, 's, (&'static Camera, &'static RenderTarget)>,
    primary: Query<'w, 's, Entity, With<PrimaryWindow>>,
}

impl OrderedPointer<'_, '_> {
    pub(super) fn discard(&mut self) {
        self.events.clear();
        *self.capture = Capture::default();
    }

    pub(super) fn poll(&mut self) -> Frame {
        let mut frame = Frame { owned: self.capture.root.is_some(), ..default() };
        if self.events.len() > EVENT_BUDGET {
            // 异常输入洪流整笔取消，不能截断后把最后位置误当成成功松手。
            frame.handled = true;
            frame.canceled = frame.owned;
            self.discard();
            warn!("数字拖动指针事件超过单帧预算，当前手势已取消");
            return frame;
        }
        // 一次只结束一笔，后续真实按下留在官方消息游标中，下一帧再消费。
        for event in self.events.read() {
            if event.pointer_id != PointerId::Mouse { continue; }
            frame.handled = true;
            if let Some(field) = self.capture.field {
                if self.capture.location.as_ref().is_some_and(|old| old.target != event.location.target) {
                    frame.canceled = true;
                } else {
                    match event.action {
                        PointerAction::Move { .. } => self.capture.location = Some(event.location.clone()),
                        PointerAction::Release(button) if Some(button) == self.capture.button => {
                            self.capture.location = Some(event.location.clone());
                            frame.released = true;
                        }
                        PointerAction::Cancel => frame.canceled = true,
                        _ => {}
                    }
                }
                frame.position = self.capture.location.as_ref().and_then(|location|
                    field_x(field, location, &self.nodes, &self.cameras, self.primary.single().ok()));
                if let Some(x) = frame.position {
                    self.capture.excursion = self.capture.excursion.max((x - self.capture.start_x).abs());
                    frame.excursion = self.capture.excursion;
                }
                if frame.position.is_none() { frame.canceled = true; }
                if frame.released || frame.canceled { *self.capture = Capture::default(); break; }
            } else if let PointerAction::Press(button @ (PointerButton::Primary | PointerButton::Secondary)) = event.action {
                let root = hit(&event.location, self.stack.as_deref(), &self.nodes,
                    &self.clipping, &self.parents, &self.fields, &self.cameras, self.primary.single().ok());
                if let Some((root, field, x)) = root {
                    let kind = if button == PointerButton::Primary { NumericInputScrubButton::Value }
                        else { NumericInputScrubButton::Sensitivity };
                    frame.start = Some((root, kind, x));
                    frame.position = Some(x);
                    frame.owned = true;
                    self.capture.root = Some(root);
                    self.capture.field = Some(field);
                    self.capture.button = Some(button);
                    self.capture.location = Some(event.location.clone());
                    self.capture.start_x = x;
                    self.capture.excursion = 0.;
                }
            }
        }
        if frame.owned && frame.position.is_none() && !frame.canceled {
            frame.position = self.capture.field.zip(self.capture.location.as_ref()).and_then(|(field, location)|
                field_x(field, location, &self.nodes, &self.cameras, self.primary.single().ok()));
            if frame.position.is_none() { frame.canceled = true; *self.capture = Capture::default(); }
        }
        frame
    }
}

type Nodes<'w, 's> = Query<'w, 's, (&'static ComputedNode, &'static UiGlobalTransform,
    Option<&'static InheritedVisibility>, Option<&'static FocusPolicy>,
    Option<&'static ComputedUiTargetCamera>, Option<&'static UiTargetCamera>)>;

fn window_position(location: &Location, camera: Entity,
    cameras: &Query<(&Camera, &RenderTarget)>, primary: Option<Entity>) -> Option<Vec2> {
    let (camera, target) = cameras.get(camera).ok()?;
    if !camera.is_active || target.normalize(primary)? != location.target { return None; }
    let point = location.position * camera.target_scaling_factor().unwrap_or(1.);
    point.is_finite().then_some(point)
}

fn physical_position(location: &Location, camera: Entity,
    cameras: &Query<(&Camera, &RenderTarget)>, primary: Option<Entity>) -> Option<Vec2> {
    let mut point = window_position(location, camera, cameras, primary)?;
    let (camera, _) = cameras.get(camera).ok()?;
    if let Some(viewport) = camera.physical_viewport_rect() {
        point -= viewport.min.as_vec2();
    }
    point.is_finite().then_some(point)
}

fn field_x(root: Entity, location: &Location, nodes: &Nodes,
    cameras: &Query<(&Camera, &RenderTarget)>, primary: Option<Entity>) -> Option<f32> {
    let (_, _, _, _, computed, explicit) = nodes.get(root).ok()?;
    let camera = computed.and_then(ComputedUiTargetCamera::get).or_else(|| explicit.map(|value| value.0))?;
    // 布局只负责按下命中；整笔以窗口物理坐标计算位移。字段因文本/面板移动或缩放，
    // 不能被当成鼠标继续拖动，也不能在静止帧把已采用数值推向另一档。
    Some(window_position(location, camera, cameras, primary)?.x)
}

fn hit(location: &Location, stack: Option<&UiStack>, nodes: &Nodes,
    clipping: &Query<(&ComputedNode, &UiGlobalTransform, &Node)>,
    parents: &Query<&ChildOf, Without<OverrideClip>>,
    fields: &Query<&NumericInputFieldButton, Without<NumericInputDisabled>>,
    cameras: &Query<(&Camera, &RenderTarget)>, primary: Option<Entity>) -> Option<(Entity, Entity, f32)> {
    let stack = stack?;
    for &entity in stack.uinodes.iter().rev().take(HIT_BUDGET) {
        let Ok((node, transform, visible, policy, computed, explicit)) = nodes.get(entity) else { continue; };
        if !visible.is_some_and(|value| value.get()) || node.is_empty() { continue; }
        let Some(camera) = computed.and_then(ComputedUiTargetCamera::get).or_else(|| explicit.map(|value| value.0)) else { continue; };
        let Some(point) = physical_position(location, camera, cameras, primary) else { continue; };
        if !node.contains_point(*transform, point)
            || !bevy::ui::clip_check_recursive(point, entity, clipping, parents) { continue; }
        // 沿官方UI栈与FocusPolicy裁决顶层；不能只测字段矩形而穿过上方对话框。
        if let Ok(field) = fields.get(entity) {
            return field_x(entity, location, nodes, cameras, primary).map(|x| (field.root, entity, x));
        }
        if policy != Some(&FocusPolicy::Pass) { return None; }
    }
    None
}
