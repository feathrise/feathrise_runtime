//! 从场景 GLB 同时采样位置、旋转和缩放，由 Avian 同步模型、碰撞与承托速度。

use avian3d::prelude::{
    AngularVelocity, Collider, ColliderOf, ComputedCenterOfMass, LinearVelocity, Physics,
    PhysicsTime, Position, RigidBody, RigidBodyDisabled, Rotation,
};
use bevy::{ecs::system::EntityCommands, prelude::*};
use bevy_tnua::TnuaNotPlatform;
use feathrise_scene_content::{SceneMotionPlacementTrack, SceneMotionTrack, ScenePlacementRecord};
use feathrise_scene_core::{input::EditorInputState, residency::SceneInstanceOwner};
use std::sync::Arc;

use super::ScenePlacementInstance;

mod playback;
use playback::MotionOwners;
pub use playback::{SceneMotionAction, SceneMotionCommand, SceneMotionDirection, SceneMotionFinished};
pub(super) use playback::{MotionController, advance as advance_playback, apply_commands, carry as carry_playback};

#[derive(Clone, Debug)]
pub(crate) enum PlacementMotion {
    Legacy(Arc<SceneMotionTrack>),
    Glb(Arc<[SceneMotionPlacementTrack]>),
}

impl PlacementMotion {
    pub(super) fn first_track(&self) -> &SceneMotionTrack {
        match self {
            Self::Legacy(track) => track,
            Self::Glb(tracks) => &tracks[0].track,
        }
    }

    pub(super) fn shares_definition(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Legacy(left), Self::Legacy(right)) => Arc::ptr_eq(left, right),
            (Self::Glb(left), Self::Glb(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl From<Arc<SceneMotionTrack>> for PlacementMotion {
    fn from(track: Arc<SceneMotionTrack>) -> Self { Self::Legacy(track) }
}

#[derive(Component)]
pub(super) struct MotionPlatform {
    pub(super) motion: PlacementMotion,
    collider: Option<Entity>,
    origin: Transform,
    elapsed: Option<f64>,
    binding: Option<usize>,
    routing: Option<(Entity, u64)>,
    enabled: bool,
}

impl MotionPlatform {
    pub(super) fn collider_ready(&self, world: &World, root: Entity) -> bool {
        world.get::<RigidBody>(root) == Some(&RigidBody::Kinematic)
            && self.collider.is_none_or(|collider| world.get::<Collider>(collider).is_some()
                && world.get::<ChildOf>(collider).is_some_and(|parent| parent.parent() == root)
                && world.get::<ColliderOf>(collider).is_some_and(|owner| owner.body == root))
    }
}

pub(super) fn spawn(
    root: &mut EntityCommands,
    transform: Transform,
    record: &ScenePlacementRecord,
    motion: impl Into<PlacementMotion>,
) {
    let motion = motion.into();
    let body = root.id();
    root.insert((RigidBody::Kinematic, RigidBodyDisabled, TnuaNotPlatform));
    // 作者盒已经包含初始缩放。子盒抵消初值，之后随根的比例变化同步形状和偏心位置。
    let collider = motion.first_track().collider_for(record).map(|shape| {
        let size = shape.half_extents * 2.0;
        root.commands().spawn((
            Name::new("场景动画碰撞"),
            Collider::cuboid(size.x, size.y, size.z),
            Transform::from_translation(shape.center / transform.scale).with_scale(transform.scale.recip()),
            ChildOf(body),
        )).id()
    });
    root.insert(MotionPlatform {
        motion, collider, origin: transform, elapsed: None, binding: None, routing: None, enabled: false,
    });
}

pub(super) fn drive(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    physics: Option<Res<Time<Physics>>>,
    editor: Option<Res<EditorInputState>>,
    parents: Query<(&GlobalTransform, &InheritedVisibility)>,
    owners: MotionOwners,
    controllers: Query<&MotionController>,
    mut platforms: Query<(
        Entity, &ScenePlacementInstance, &ChildOf, &mut Position, &mut Rotation,
        &mut LinearVelocity, &mut AngularVelocity, &ComputedCenterOfMass, &mut Transform,
        &mut MotionPlatform, Has<RigidBodyDisabled>, Option<&SceneInstanceOwner>,
    )>,
) {
    let Some(physics) = physics else { return; };
    let delta = time.delta_secs_f64() * physics.relative_speed_f64();
    // 独立作者宿主继续跑碰撞调试，但作者预览拥有变换，游戏轨迹既不写入也不计时。
    let paused = physics.is_paused() || editor.is_some_and(|input| input.scene_editor_open);
    for (entity, placement, parent, mut position, mut rotation, mut linear, mut angular, center, mut transform, mut platform, disabled, owner) in &mut platforms {
        let Ok((frame, visibility)) = parents.get(parent.parent()) else { continue; };
        // 子场景候选可能已采用内部摆放，祖先可见性仍挡住尚未发布的整图及已停放的副本。
        let enabled = placement.is_active() && visibility.get();
        if enabled != platform.enabled {
            if enabled {
                commands.entity(entity).remove::<(RigidBodyDisabled, TnuaNotPlatform)>();
            } else {
                commands.entity(entity).insert((RigidBodyDisabled, TnuaNotPlatform));
            }
            platform.enabled = enabled;
        } else if disabled {
            continue; // 不解除外部物理停用，也不推进该平台的相位。
        }
        if !enabled || paused || delta <= 0.0 {
            linear.set_if_neq(LinearVelocity(Vec3::ZERO));
            angular.set_if_neq(AngularVelocity(Vec3::ZERO));
            continue;
        }
        let Some((local, reset)) = sample(&mut platform, &owners, owner, placement, &controllers, delta) else {
            // 手动片段仍在初始姿势，相同承托刚体保持静止并继续提供碰撞。
            linear.set_if_neq(LinearVelocity(Vec3::ZERO));
            angular.set_if_neq(AngularVelocity(Vec3::ZERO));
            continue;
        };
        let target = frame.transform_point(local.translation);
        let target_rotation = frame.rotation() * local.rotation;
        let valid = target.is_finite() && target_rotation.is_finite();
        let mut center = center.0;
        if valid && transform.scale != local.scale {
            // Prepare 会在本步按新比例更新子盒及其质心，角速度补偿也应使用该质心。
            center *= local.scale / transform.scale;
            transform.scale = local.scale;
        }
        if !valid || reset {
            // 第零帧和循环断点采用整套姿态，避免把不相邻的首尾姿态差转成承托冲量。
            if valid {
                position.set_if_neq(Position(target));
                rotation.set_if_neq(Rotation(target_rotation));
            }
            linear.set_if_neq(LinearVelocity(Vec3::ZERO));
            angular.set_if_neq(AngularVelocity(Vec3::ZERO));
            continue;
        }
        let difference = (target_rotation * rotation.0.inverse()).normalize();
        let shortest = if difference.w < 0.0 { -difference } else { difference };
        let spin = shortest.to_scaled_axis() / delta as f32;
        // Avian 积分质心速度；偏心碰撞盒旋转时仍需让模型原点命中作者的下一姿态。
        let displacement = target - position.0 + (target_rotation * center - rotation.0 * center);
        let velocity = displacement / delta as f32;
        linear.set_if_neq(LinearVelocity(if velocity.is_finite() { velocity } else { Vec3::ZERO }));
        angular.set_if_neq(AngularVelocity(if spin.is_finite() { spin } else { Vec3::ZERO }));
    }
}

fn sample(
    platform: &mut MotionPlatform,
    owners: &MotionOwners,
    owner: Option<&SceneInstanceOwner>,
    placement: &ScenePlacementInstance,
    controllers: &Query<&MotionController>,
    delta: f64,
) -> Option<(Transform, bool)> {
    let MotionPlatform { motion, origin, elapsed, binding, routing, .. } = platform;
    match motion {
        PlacementMotion::Legacy(track) => {
            let (seconds, reset) = elapsed.map_or((0.0, true), |time| track.advance(time, delta));
            *elapsed = Some(seconds);
            Some((track.sample_transform(seconds, *origin), reset))
        }
        PlacementMotion::Glb(tracks) => {
            let root = owners.active_batch(owner.map(|owner| owner.0), placement.scene_id())?;
            let controller = controllers.get(root).ok()?;
            let revision = (root, controller.routing_revision());
            let mut switched = false;
            if *routing != Some(revision) {
                let next = controller.selected_binding(tracks);
                switched = *binding != next;
                *routing = Some(revision);
                *binding = next;
            }
            let track = &tracks[(*binding)?];
            let (seconds, discontinuity) = controller.sample(track.clip_index)?;
            Some((track.track.sample_transform(seconds, *origin), switched || discontinuity))
        }
    }
}

#[cfg(test)]
mod tests;
