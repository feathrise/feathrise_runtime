//! 光追高度准备的不可变身份、局部页快照和后台结果。

use bevy::{asset::AssetId, prelude::*};
use feathrise_environment::terrain_surface::TerrainSurfaceMaterial;
use feathrise_map_content::{
    MapId, SparseCraterTiles, SparseHeightManifestAsset, SparseHeightSpec, SparseHeightTile,
    SparseHeightTileAsset, SparseHeightTileKey,
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProxyIdentity {
    pub map: MapId,
    pub root: Entity,
    pub display: Entity,
    pub manifest: AssetId<SparseHeightManifestAsset>,
    pub material: AssetId<TerrainSurfaceMaterial>,
    pub content: [u64; 5],
    pub live_revision: Option<u64>,
    pub crater_revision: Option<u64>,
    pub topology_revision: u64,
    pub ticket: u64,
}

impl ProxyIdentity {
    pub fn same_source(&self, other: &Self) -> bool {
        self.map == other.map
            && self.root == other.root
            && self.display == other.display
            && self.manifest == other.manifest
            && self.material == other.material
            && self.content == other.content // 票据另在采用边界核对；镜头窗口身份不属于几何输入。
            && self.live_revision == other.live_revision
            && self.crater_revision == other.crater_revision
            && self.topology_revision == other.topology_revision
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ProxyGrid {
    pub level: u8,
    pub cells: [u32; 2],
}

impl ProxyGrid {
    pub fn uniform(self) -> Vec4 {
        Vec4::new(
            (1u32 << self.level) as f32,
            self.cells[0] as f32,
            self.cells[1] as f32,
            1.0,
        )
    }
}

pub(super) struct FormalPage {
    pub key: SparseHeightTileKey,
    pub hash: String,
    pub handle: Option<Handle<SparseHeightTileAsset>>,
}

pub(super) struct ProxyCapture {
    pub stitches:Arc<feathrise_environment::vegetation::blade::GpuBladeClipmapStitches>,
    pub holes: Arc<[feathrise_map_content::MapTerrainCutout]>,
    pub identity: ProxyIdentity,
    pub manifest: Handle<SparseHeightManifestAsset>,
    pub source: Handle<TerrainSurfaceMaterial>,
    pub grid: ProxyGrid,
    pub spec: SparseHeightSpec,
    pub refs: Vec<FormalPage>,
    pub formal: BTreeMap<SparseHeightTileKey, Arc<SparseHeightTile>>,
    pub live: BTreeMap<SparseHeightTileKey, Arc<SparseHeightTile>>,
    pub live_cursor: usize,
    pub live_count: usize,
    pub craters: SparseCraterTiles,
    pub crater_cursor: usize,
    pub crater_count: usize,
}

pub(super) struct ProxyResult {
    pub identity: ProxyIdentity,
    pub source: Handle<TerrainSurfaceMaterial>,
    pub mesh: Result<PreparedProxy, String>,
}

pub(super) struct PreparedProxy {
    pub mesh: Mesh,
    pub grid: ProxyGrid,
    pub axes: super::super::material::ClipmapProxyAxes,
}

pub(super) enum ProxyGarbage {
    Capture(ProxyCapture),
    Mesh(Mesh),
    Result(ProxyResult),
}
