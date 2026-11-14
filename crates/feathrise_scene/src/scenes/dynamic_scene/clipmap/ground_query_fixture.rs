//! 有界CPU查询夹具：共享同一非线性R16显示页与作者高度页，只用于测试，不代表GPU采用。
use super::*;
use super::super::authority::{AuthorityHeightSnapshot, AuthorityIdentity, AuthorityPage};
use feathrise_environment::vegetation::blade::GpuBladeClipmapBand;
use feathrise_scene_content::{SparseHeightSpec, SparseHeightTile, SparseHeightTileKey};
use std::collections::BTreeMap;

pub(super) fn fixed_bands(half_cells: u32, levels: u32) -> [GpuBladeClipmapBand; 16] {
    // 旧固定模板只用于验证新可变描述的等价路径。
    let mut bands = [GpuBladeClipmapBand::default(); 16];
    for (level, band) in bands.iter_mut().enumerate().take(levels as usize) {
        *band = GpuBladeClipmapBand {
            inner_half_cells: if level == 0 {
                UVec2::ZERO
            } else {
                UVec2::splat(half_cells / 2)
            },
            outer_half_cells: UVec2::splat(half_cells),
            geometry_mip: level as u32,
        };
    }
    bands
}

pub(super) fn fixture_height_code(x: f32, z: f32) -> u16 {
    let height = 12.0 + (x * 0.13).sin() * 4.0 + (z * 0.17).cos() * 3.0 + x * z * 0.0001;
    ((height / 32.0 * 65535.0).round()) as u16
}

pub(super) fn snapshot() -> ClipmapGroundSnapshot {
    // 非方形场景与非线性高度。
    let heights = (0..257 * 257)
        .flat_map(|i| {
            let x = (i % 257) as f32;
            let z = (i / 257) as f32;
            fixture_height_code(x, z).to_le_bytes()
        })
        .collect(); // R16坡面、鞍形和频率变化。
    let mut table = Vec::new(); // 五级各单页，复用同一层但采样位置随Mip变化。
    for _ in 0..5 {
        for word in [1_u32, 0.0_f32.to_bits(), 32.0_f32.to_bits(), 0] {
            table.extend_from_slice(&word.to_le_bytes());
        }
    } // 可辨认的每级表入口。
    let height = GpuBladeClipmapHeight {stitches: Default::default(),
        holes: [Vec4::ZERO; 32],
        hole_count: 0,
        atlas: Handle::default(),
        page_table: Handle::default(),
        world_min: Vec2::ZERO,
        world_max: Vec2::new(96.0, 64.0),
        cell_size: Vec2::ONE,
        logical_cells: UVec2::new(96, 64),
        max_mip: 4,
        fine_center: Vec2::new(47.0, 31.0),
        bands: fixed_bands(8, 5),
        band_count: 5,
        height_min: 0.0,
        height_max: 32.0,
        revision: [1, 2, 3],
        missing_pages: [UVec4::ZERO; 16],
        ready: true,
    }; // 故意奇数中心使相邻环带独立吸附。
    let content_revision = ClipmapGroundContentRevision {
        root: 1,
        manifest: 2,
        live: None,
        crater: None,
        topology: 3,
    };
    let spec = SparseHeightSpec::new([0.0, 0.0], [96.0, 64.0], 1.0, 0.0, 32.0).unwrap();
    let key = SparseHeightTileKey { x: 0, z: 0 };
    let mut authority_tile = SparseHeightTile::blank(&spec, key).unwrap();
    for z in 0..=64_u16 {
        for x in 0..=96_u16 {
            // 这份旧R16夹具的CPU/GPU必须仍表示同一源值；新encode本身已不量化。
            // 显式把GPU字节解回米制再存f32，避免无关水深回归混入两个不同精度的地形。
            let height = f32::from(fixture_height_code(f32::from(x), f32::from(z))) / 65535.0 * 32.0;
            authority_tile
                .set([x, z], spec.encode_height(height).unwrap())
                .unwrap();
        }
    }
    ClipmapGroundSnapshot {
        author_stitches:Default::default(),
        holes: Arc::default(),
        terrain_meshes: Arc::default(),
        hole_revision: 0,
        scene_id: SceneId::parse("MAP-ground-test").unwrap(),
        revision: height.revision,
        missing_pages: height.missing_pages,
        ready: true,
        content_revision,
        height,
        pages: Some(Arc::new(GroundPageBytes {
            heights,
            table,
            width: 1,
            height_bounds: [0.0, 32.0],
        })),
        authority: Some(Arc::new(AuthorityHeightSnapshot {
            identity: AuthorityIdentity {
                scene_id: SceneId::parse("MAP-ground-test").unwrap(),
                content: content_revision,
            },
            revision: 1,
            spec,
            pages: BTreeMap::from([(key, AuthorityPage::Tile(Arc::new(authority_tile)))]),
            craters: BTreeMap::new(),
        })),
        authority_requests: AuthorityRequestSink::default(),
    } // 查询只读输入。
} // 不创建运行时World。
