//! 隐藏源锚点的身份；它不承担显示环带、物理或作者摆放地形标记。

use super::types::{ProxyGrid, ProxyIdentity};
use bevy::prelude::*;
use feathrise_environment::terrain_surface::TerrainSurfaceMaterial;

/// 仅供动态 Solari 目标发现的真实高度源；相机平移不会改变此实体的 Mesh。
#[derive(Component)]
pub struct ClipmapRaytracingAnchor {
    pub(super) axes: super::super::material::ClipmapProxyAxes,
    pub(super) identity: ProxyIdentity,
    pub(super) grid: ProxyGrid,
    pub(super) source: Handle<TerrainSurfaceMaterial>,
}

impl ClipmapRaytracingAnchor {
    pub fn content_revision(&self) -> [u64; 5] {
        self.identity.content
    }
    pub fn grid_cells(&self) -> [u32; 2] {
        self.grid.cells
    }
    pub fn mip_level(&self) -> u8 {
        self.grid.level
    }
}
