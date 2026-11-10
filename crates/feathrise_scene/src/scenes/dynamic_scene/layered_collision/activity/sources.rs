//! 角色承托优先采用物理射线实际命中的场景，空页沿用原来源，不能落到父图同名区域。
use avian3d::prelude::{RigidBody, RigidBodyDisabled, SpatialQuery, SpatialQueryFilter};
use bevy::{ecs::system::SystemParam, prelude::*};
use feathrise_scene_core::residency::{InstanceResourceTable, SceneInstanceOwner, SceneSourceFrame};
use crate::SceneCollisionParticipant;
use feathrise_scene_core::residency::TerrainCollisionRegion;
use crate::scenes::dynamic_scene::instances::{SubsceneActive, SubsceneInstance};

#[derive(Clone, Copy, Default)]
pub(super) struct Source { pub instance: Option<Entity>, pub frame: SceneSourceFrame }
#[derive(SystemParam)]
pub(in crate::scenes::dynamic_scene) struct Sources<'w, 's> {
    spatial: Option<SpatialQuery<'w, 's>>,
    owners: Query<'w, 's, &'static SceneInstanceOwner>,
    parents: Query<'w, 's, &'static ChildOf>,
    bodies: Query<'w, 's, (&'static RigidBody, Has<RigidBodyDisabled>)>,
    roots: Query<'w, 's, (&'static SubsceneInstance, &'static GlobalTransform)>,
    active: Query<'w, 's, (), With<SubsceneActive>>,
    regions: Option<ResMut<'w, InstanceResourceTable<TerrainCollisionRegion>>>,
}
impl Sources<'_, '_> {
    pub(super) fn has_sources(&self) -> bool { !self.roots.is_empty() && !self.active.is_empty() }
    fn context(&self, entity: Entity) -> Option<Source> {
        let (instance, transform) = self.roots.get(entity).ok()?;
        self.active.contains(instance.batch).then(|| Source { instance: Some(entity),
            frame: SceneSourceFrame::from_transform(transform.compute_transform()) })
    }
    fn collider_source(&self, entity: Entity) -> Option<Source> {
        let mut cursor = entity;
        let mut source = None;
        for _ in 0..256 {
            if source.is_none() {
                if let Ok(owner) = self.owners.get(cursor) { source = Some(self.context(owner.0)?); }
            }
            if let Ok((body, disabled)) = self.bodies.get(cursor) {
                return (*body == RigidBody::Static && !disabled).then_some(source.unwrap_or_default());
            }
            cursor = self.parents.get(cursor).ok()?.parent();
        }
        None
    }
    fn hit_source(&self, position: Vec3) -> Option<Source> {
        if let Some(spatial) = &self.spatial {
            if let Some(hit) = spatial.cast_ray_predicate(position + Vec3::Y * 2.0, Dir3::NEG_Y,
                10000.0, false, &SpatialQueryFilter::default(),
                &|entity| self.collider_source(entity).is_some()) {
                return self.collider_source(hit.entity);
            }
        }
        None
    }
    pub(super) fn resolve(&self, position: Vec3, identity: Option<&SceneCollisionParticipant>,
        previous: Option<Entity>) -> Source {
        if let Some(source) = self.hit_source(position) { return source; }
        if let Some(previous) = previous.and_then(|entity| self.context(entity)) { return previous; }
        identity.and_then(|identity| {
            let owner = identity.instance_owner?;
            let (instance, _) = self.roots.get(owner).ok()?;
            (identity.instance_path.as_ref() == Some(&instance.path)
                && identity.instance_generation == Some(instance.generation)
                && identity.source_scene.as_ref() == instance.source_scene_id.as_str())
                .then(|| self.context(owner)).flatten()
        }).unwrap_or_default()
    }
    pub(super) fn transfer_supported(&mut self, source: Source, position: Vec3, predicted: Vec3,
        half: Vec2, root: &mut TerrainCollisionRegion, player: Option<Vec3>) -> bool {
        // 只在场景边缘调用固定十条射线；父子图相接的实际地面可承托，缺页和空隙不能借父图兜底。
        let mut changes_source = false;
        for center in [position, predicted] {
            for offset in [Vec2::ZERO, half, -half, Vec2::new(half.x, -half.y), Vec2::new(-half.x, half.y)] {
                let point = center + Vec3::new(offset.x, 0.0, offset.y);
                let Some(hit) = self.hit_source(point) else { return false; };
                changes_source |= hit.instance != source.instance;
                if !self.region(hit, root, player).supports(hit.frame.to_local(point)) { return false; }
            }
        }
        changes_source
    }
    pub(super) fn region<'a>(&'a mut self, source: Source, root: &'a mut TerrainCollisionRegion,
        player: Option<Vec3>) -> &'a mut TerrainCollisionRegion {
        let region = source.instance.and_then(|entity| self.regions.as_deref_mut()?.values.get_mut(&entity))
            .unwrap_or(root);
        if region.enabled {
            if let Some(player) = player {
                let center = source.frame.to_local(player).xz();
                if region.center != center { region.center = center; region.revision = region.revision.wrapping_add(1); }
            }
        }
        region
    }
}
