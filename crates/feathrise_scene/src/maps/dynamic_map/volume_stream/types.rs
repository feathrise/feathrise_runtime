//! 流送身份、单飞任务句柄与系统集；组件及资源由各自文件定义。
use super::super::terrain_topology::TerrainVolumePreviewTarget;
use bevy::{prelude::*, tasks::Task};
use feathrise_map_content::{MapId, MapPlacementRecord};
use std::sync::{Arc, atomic::AtomicU64};

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TerrainVolumeStreamSystems {
    Drive,
}

#[derive(Clone, PartialEq)]
pub(super) struct Identity {
    pub map: MapId,
    pub root: Entity,
    pub record: MapPlacementRecord,
    pub center: [i32; 3],
    pub physics_center: Option<[i32; 3]>,
    pub lods: Arc<[feathrise_map_terrain::volume::sparse::paged::VolumeDisplayLod]>,
}
pub(super) struct Built {
    pub record: MapPlacementRecord,
    pub meshes: Vec<Mesh>,
    pub fields: super::display::VolumeSurfaceFields,
}
pub(super) struct Work {
    pub identity: Identity,
    pub task: Task<Result<Built, String>>,
    pub started: f64,
    pub progress: Arc<AtomicU64>,
    pub observed: u64,
}
pub(super) struct Pending {
    pub identity: Identity,
    pub root: Entity,
    pub previous: Option<TerrainVolumePreviewTarget>,
    pub started: f64,
}
