//! 保存每次接管前的真实状态。取消恢复原世界，成功只归还控制，保留编排好的落点。
use super::clips::{OriginalPlayer, ShotClip};
use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, RigidBody, Rotation};
use bevy::prelude::*;
use feathrise_scene_core::cinematic::{
    CinematicAnimationOwner, CinematicPoseOwner, CinematicRestingActor,
};
use std::collections::{HashMap, HashSet};

pub(super) struct Plan {
    pub entity: Entity,
    pub pose: Option<Mat4>,
    pub clip: Option<ShotClip>,
}

struct OriginalPose {
    already_owned: bool,
    local: Transform,
    body: Option<RigidBody>,
    position: Option<Position>,
    rotation: Option<Rotation>,
    linear: Option<LinearVelocity>,
    angular: Option<AngularVelocity>,
}

fn parent_world(world: &World, e: Entity) -> Mat4 {
    world
        .get::<ChildOf>(e)
        .and_then(|parent| world.get::<GlobalTransform>(parent.parent()))
        .map_or(Mat4::IDENTITY, GlobalTransform::to_matrix)
}

pub(super) fn local_to_world(world: &World, e: Entity, local: Transform) -> Mat4 {
    parent_world(world, e) * local.to_matrix()
}

pub(super) fn set_world_pose(world: &mut World, entity: Entity, matrix: Mat4) {
    let local = Transform::from_matrix(parent_world(world, entity).inverse() * matrix);
    let pose = Transform::from_matrix(matrix);
    let mut e = world.entity_mut(entity);
    if let Some(mut t) = e.get_mut::<Transform>() {
        t.set_if_neq(local);
    }
    if let Some(mut p) = e.get_mut::<Position>() {
        p.set_if_neq(Position(pose.translation));
    }
    if let Some(mut r) = e.get_mut::<Rotation>() {
        r.set_if_neq(Rotation(pose.rotation));
    }
    if let Some(mut v) = e.get_mut::<LinearVelocity>() {
        v.0 = Vec3::ZERO;
    }
    if let Some(mut v) = e.get_mut::<AngularVelocity>() {
        v.0 = Vec3::ZERO;
    }
}

impl OriginalPose {
    fn begin(world: &mut World, e: Entity) -> Self {
        let mut entity = world.entity_mut(e);
        let original = Self {
            already_owned: entity.contains::<CinematicPoseOwner>(),
            local: *entity.get::<Transform>().unwrap(),
            position: entity.get::<Position>().copied(),
            rotation: entity.get::<Rotation>().copied(),
            linear: entity.get::<LinearVelocity>().copied(),
            angular: entity.get::<AngularVelocity>().copied(),
            body: entity.get::<RigidBody>().copied(),
        };
        if original.body == Some(RigidBody::Dynamic) {
            entity.insert(RigidBody::Kinematic);
        }
        entity.insert(CinematicPoseOwner);
        original
    }
    fn release(self, world: &mut World, entity: Entity, rollback: bool) {
        let Ok(mut e) = world.get_entity_mut(entity) else {
            return;
        };
        if !(rollback && self.already_owned) {
            e.remove::<CinematicPoseOwner>();
        }
        if self.body == Some(RigidBody::Dynamic) {
            e.insert(RigidBody::Dynamic);
        }
        if rollback {
            e.insert(self.local);
            if let Some(v) = self.position {
                e.insert(v);
            }
            if let Some(v) = self.rotation {
                e.insert(v);
            }
            if let Some(v) = self.linear {
                e.insert(v);
            }
            if let Some(v) = self.angular {
                e.insert(v);
            }
        }
    }
    fn return_control(&self, world: &mut World, entity: Entity) {
        if let Ok(mut e) = world.get_entity_mut(entity) {
            e.remove::<CinematicPoseOwner>();
            if self.body == Some(RigidBody::Dynamic) {
                e.insert(RigidBody::Dynamic);
            }
        }
    }
}

#[derive(Default)]
pub(super) struct Leases {
    poses: HashMap<Entity, OriginalPose>,
    players: HashMap<Entity, (OriginalPlayer, bool)>,
}

impl Leases {
    pub fn return_actor_movement(&self, world: &mut World, root: Entity) {
        if let Some(pose) = self.poses.get(&root) {
            pose.return_control(world, root);
        }
        if self.players.contains_key(&root) {
            world.entity_mut(root).insert(CinematicRestingActor);
        }
    }
    pub fn apply(&mut self, world: &mut World, plan: &Plan) {
        if let Some(mat) = plan.pose {
            self.poses
                .entry(plan.entity)
                .or_insert_with(|| OriginalPose::begin(world, plan.entity));
            set_world_pose(world, plan.entity, mat);
        }
        if let Some(clip) = &plan.clip {
            if self
                .players
                .get(&plan.entity)
                .is_some_and(|(original, _)| original.entity() != clip.player)
            {
                self.release_player(world, plan.entity);
            }
            self.players.entry(plan.entity).or_insert_with(|| {
                let owned = world.get::<CinematicAnimationOwner>(plan.entity).is_some();
                let original = OriginalPlayer::capture(world, clip.player);
                world
                    .entity_mut(plan.entity)
                    .insert(CinematicAnimationOwner);
                (original, owned)
            });
            clip.apply(world);
        }
    }

    /// 拖回首键之前或目标重建时，归还已经不在本采样中的轨道。
    pub fn remove_missing(
        &mut self,
        world: &mut World,
        poses: &HashSet<Entity>,
        players: &HashSet<Entity>,
    ) {
        for entity in self
            .poses
            .keys()
            .filter(|e| !poses.contains(e))
            .copied()
            .collect::<Vec<_>>()
        {
            self.poses
                .remove(&entity)
                .unwrap()
                .release(world, entity, true);
        }
        for entity in self
            .players
            .keys()
            .filter(|e| !players.contains(e))
            .copied()
            .collect::<Vec<_>>()
        {
            self.release_player(world, entity);
        }
    }

    pub fn release_player(&mut self, world: &mut World, entity: Entity) {
        if let Some((original, owned)) = self.players.remove(&entity) {
            original.restore(world);
            if !owned && let Ok(mut e) = world.get_entity_mut(entity) {
                e.remove::<(CinematicAnimationOwner, CinematicRestingActor)>();
            }
        }
    }

    /// 终帧的骨架、物件和原始快照直接移交，结算与读档重投之间不会闪回原动作一帧。
    pub fn adopt(&mut self, root: Entity, source: &mut Self) {
        if let Some(original) = source.poses.remove(&root) {
            self.poses.entry(root).or_insert(original);
        }
        if let Some(original) = source.players.remove(&root) {
            self.players.entry(root).or_insert(original);
        }
    }

    pub fn owns_player(&self, root: Entity) -> bool {
        self.players.contains_key(&root)
    }

    pub fn release(&mut self, world: &mut World, commit: bool) {
        for (entity, pose) in self.poses.drain() {
            pose.release(world, entity, !commit);
        }
        for root in self.players.keys().copied().collect::<Vec<_>>() {
            self.release_player(world, root);
        }
    }
}
