//! 静态网格LOD的不可变来源票据、缓存条目与相机投影规格。
use bevy::{asset::AssetId, prelude::*};
use feathrise_scene_content::MeshLodPolicy;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct SourceKey {
    pub id: AssetId<Mesh>,
    pub revision: u64,
    pub epoch: u64,
}

pub(super) struct VisualTarget {
    pub owner: Entity,
    pub original: Handle<Mesh>,
    pub key: SourceKey,
    pub applied: usize,
}

pub(super) struct VisualLevel {
    pub mesh: Handle<Mesh>,
    pub error: f32,
    pub bytes: usize,
}

pub(super) struct VisualCache {
    pub levels: Vec<VisualLevel>,
    pub bytes: usize,
    pub pins: usize,
    pub complete: bool,
    pub failed: bool,
    pub retry_credit: Option<u64>,
}

pub(super) struct Owner {
    pub root: Entity,
    pub policy: MeshLodPolicy,
    pub building: bool,
    pub active: bool,
}

// 正式场景与独立作者承载复用同一投影公式，不让新视口另写距离LOD。
pub(super) use feathrise_terrain::StaticMeshLodView as View;
