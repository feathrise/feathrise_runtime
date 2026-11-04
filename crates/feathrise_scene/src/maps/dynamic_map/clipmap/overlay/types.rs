//! 地表辅助覆盖的完整作者身份；场景只借用既有RGBA图，绝不接收整图作者像素。

use bevy::prelude::*;
use feathrise_map_content::{MapId, TerrainId};

/// 同图切网格、换代或整体姿态变化不能把旧笔迹投到新对象上。
#[derive(Clone, Debug, PartialEq)]
pub struct ClipmapSurfaceOverlayIdentity {
    pub map_id: MapId,
    pub terrain_id: Option<TerrainId>,
    pub author_revision: u64,
    pub terrain_revision: u64,
    pub surface_revision: u64,
    pub source_to_world: Mat4,
}

#[derive(Clone, Debug)]
pub struct ClipmapSurfaceCoverage {
    pub identity: ClipmapSurfaceOverlayIdentity,
    pub image: Handle<Image>,
}
