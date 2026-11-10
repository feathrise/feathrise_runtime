//! 每个体素独占在途结果与采用身份；原显示和碰撞一直保留至新候选全部准备完成。
use avian3d::prelude::Collider;
use bevy::{prelude::*, tasks::Task};
use feathrise_scene_content::{SceneVoxelTerrainDescriptor, SceneVoxelTerrainSource};
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Identity {
    pub root: Entity,
    pub source: AssetId<SceneVoxelTerrainSource>,
    pub revision: u64,
    pub descriptor: SceneVoxelTerrainDescriptor,
    pub display_center: [i32; 3],
    pub physics_centers: Vec<[i32; 3]>,
    pub player_center: [i32; 3],
    pub anchor_extent: f32,
    pub scale: Vec3,
}
pub(super) struct Built {
    pub identity: Identity,
    pub meshes: VecDeque<(Mesh, bevy::camera::primitives::Aabb)>,
    pub collider: Option<Collider>,
    pub display_query: Option<Collider>, // 精确点选使用显示三角面，不能借用独立碰撞LOD。
    pub index: std::sync::Arc<super::window::SourceIndex>,
    pub fields: crate::VolumeSurfaceFields,
    pub limited: bool,
}
pub(super) struct Candidate {
    pub root: Entity,
    pub built: Built,
    pub surface: super::surface::Binding,
    pub mesh_ids: Vec<AssetId<Mesh>>,
    pub warmup: Option<Entity>,
    pub ticket: Option<crate::WaterGpuUploadTicket>,
    pub started_at: std::time::Instant,
}
pub(super) struct RetiredRoot {
    pub root: Entity,
    pub fields: crate::VolumeSurfaceFields,
    pub collider: Option<Collider>,
}
pub(super) enum Garbage {
    Candidate(Candidate),
    Built(Built),
    Index(
        (
            AssetId<SceneVoxelTerrainSource>,
            u64,
            std::sync::Arc<super::window::SourceIndex>,
        ),
    ),
    Applied(
        Option<Collider>,
        Option<std::sync::Arc<super::window::SourceIndex>>,
        Option<super::surface::Binding>,
    ),
    Root(RetiredRoot),
}

/// 多对象和子场景共享两个CPU席位；任务真正结束前不会因换代归还额度。
#[derive(Resource, Clone, Default)]
pub(in crate::scenes::dynamic_scene) struct VoxelWorkBudget {
    busy: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    cache: std::sync::Arc<std::sync::Mutex<super::cache::ChunkCache>>,
}
pub(in crate::scenes::dynamic_scene) struct VoxelWorkLease {
    busy: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub(super) cache: std::sync::Arc<std::sync::Mutex<super::cache::ChunkCache>>,
}
impl VoxelWorkBudget {
    pub(in crate::scenes::dynamic_scene) fn acquire(&self) -> Option<VoxelWorkLease> {
        use std::sync::atomic::Ordering;
        self.busy
            .try_update(Ordering::AcqRel, Ordering::Acquire, |busy| {
                (busy < 2).then_some(busy + 1)
            })
            .ok()?;
        Some(VoxelWorkLease {
            busy: self.busy.clone(),
            cache: self.cache.clone(),
        })
    }
}
impl Drop for VoxelWorkLease {
    fn drop(&mut self) {
        self.busy.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}
#[derive(Resource, Default)]
pub(in crate::scenes::dynamic_scene) struct VoxelObjectRuntime {
    pub(super) source: Option<Handle<SceneVoxelTerrainSource>>,
    pub(super) source_path: Option<String>,
    pub(super) index: Option<(
        AssetId<SceneVoxelTerrainSource>,
        u64,
        std::sync::Arc<super::window::SourceIndex>,
    )>,
    pub(super) source_proxy: Option<Entity>,
    pub(super) active_surface: Option<super::surface::Binding>,
    pub(super) active_index: Option<std::sync::Arc<super::window::SourceIndex>>,
    pub(super) active_fields: Option<crate::VolumeSurfaceFields>,
    pub(super) active_collider: Option<Collider>,
    pub(super) revision: u64,
    pub(super) desired: Option<Identity>,
    pub(super) task: Option<Task<Result<Built, String>>>,
    pub(super) started: Option<Identity>,
    pub(super) failed: Option<Identity>,
    pub(super) candidate: Option<Candidate>,
    pub(super) applied: Option<Identity>,
    pub(in crate::scenes::dynamic_scene) active: Option<Entity>,
    pub(in crate::scenes::dynamic_scene) display_query: Option<Collider>,
    pub(in crate::scenes::dynamic_scene) generation: u64,
    pub(super) retired: VecDeque<RetiredRoot>,
    pub(super) garbage: Vec<Garbage>, // 最多当前结果和上一采用；新工作等单个回收槽结束。
    pub(super) release: Option<Task<()>>,
}
impl VoxelObjectRuntime {
    pub(in crate::scenes::dynamic_scene) fn ground_snapshot(
        &self,
        root: Entity,
    ) -> Option<super::super::clipmap::ClipmapGroundSnapshot> {
        let index = self.active_index.as_ref()?;
        let identity = self.applied.as_ref()?;
        let point = |key: [i32; 3]| {
            (Vec3::from_array(key.map(|v| v as f32)) + Vec3::splat(0.5)) * identity.anchor_extent
        };
        let mut foci = vec![point(identity.display_center)];
        let player = point(identity.player_center);
        if player != foci[0] {
            foci.push(player);
        }
        let radius = identity
            .descriptor
            .collision_lods
            .last()
            .map_or(64., |row| row.max_distance_meters)
            .min(64.0)
            / identity.scale.min_element();
        super::super::clipmap::ClipmapGroundSnapshot::from_density_mesh(
            index.source.scene_id.clone(),
            root,
            self.generation,
            index.digest,
            self.display_query.clone(),
            Vec3::from_array(index.source.min),
            Vec3::from_array(index.source.max),
            index.source.cell_size_meters,
            &foci,
            radius,
        )
        .ok()
    }
    pub(in crate::scenes::dynamic_scene) fn busy(&self) -> bool {
        self.task.is_some()
            || self.candidate.is_some()
            || self.release.is_some()
            || !self.garbage.is_empty()
    }
    pub(in crate::scenes::dynamic_scene) fn finished(&self) -> bool {
        self.task.as_ref().is_none_or(Task::is_finished)
            && self.release.as_ref().is_none_or(Task::is_finished)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_objects_share_two_slots_until_the_actual_cpu_owner_returns() {
        let first_object = VoxelWorkBudget::default();
        let second_object = first_object.clone();
        let io = std::sync::Arc::new(first_object.acquire().unwrap());
        let running_cpu = io.clone();
        let second = second_object.acquire().unwrap();
        assert!(first_object.acquire().is_none());
        drop(io); // 过期/退役的IO拥有者不能先于仍在运行的CPU子任务归还额度。
        assert!(second_object.acquire().is_none());
        drop(running_cpu);
        let third = second_object.acquire().unwrap();
        assert!(first_object.acquire().is_none());
        drop(second);
        drop(third);
        assert_eq!(
            first_object.busy.load(std::sync::atomic::Ordering::Acquire),
            0
        );
    }
}
