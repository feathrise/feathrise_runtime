//! 固定工作集的三维窗口计划；远景覆盖有限盒体，近场按观察者或物理焦点细化。
use bevy::math::Vec3;
use feathrise_map_content::{MapCollisionLod, MapVoxelTerrainSource};
use std::sync::Arc;

pub(super) const DISPLAY_CHUNKS: usize = 128;
pub(super) const COLLISION_CHUNKS: usize = 64;
pub(super) const CELLS: u32 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Chunk {
    pub depth: u8,
    pub key: [u32; 3],
}
#[derive(Clone, Copy)]
pub(super) struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}
impl Bounds {
    #[cfg(test)]
    pub fn contains(self, p: Vec3) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all()
    }
    pub fn overlaps(self, other: Self) -> bool {
        self.min.cmple(other.max).all() && self.max.cmpge(other.min).all()
    }
}
#[derive(Clone)]
pub(super) struct SourceIndex {
    pub source: MapVoxelTerrainSource,
    pub domain: Bounds,
    /// Morton仅存小型地址；密度文件不会因建立索引而全部读取。
    edits: Arc<[u64]>,
    pub digest: [u8; 32],
}
impl SourceIndex {
    pub fn new(source: MapVoxelTerrainSource) -> Result<Self, String> {
        use sha2::{Digest, Sha256};
        source.validate()?;
        let pad = Vec3::splat(source.cell_size_meters);
        let domain = Bounds {
            min: Vec3::from_array(source.min) - pad,
            max: Vec3::from_array(source.max) + pad,
        };
        let mut hash = Sha256::new();
        hash.update(source.cell_size_meters.to_le_bytes());
        for v in source.min.into_iter().chain(source.max) {
            hash.update(v.to_le_bytes());
        }
        let mut edits = Vec::with_capacity(source.blocks.len());
        for block in source.blocks.iter() {
            for value in block.key {
                hash.update(value.to_le_bytes());
            }
            hash.update(block.sha256.as_bytes());
            let center = (Vec3::from_array(block.key.map(|v| v as f32)) + Vec3::splat(0.5))
                * source.cell_size_meters
                * 32.0;
            edits.push(morton(grid(domain, center)));
        }
        edits.sort_unstable();
        edits.dedup();
        Ok(Self {
            source,
            domain,
            edits: edits.into(),
            digest: hash.finalize().into(),
        })
    }
    fn edited(&self, bounds: Bounds) -> bool {
        // 块中心索引扩大半块检索；块触及窗口边界时不能漏掉另一侧的共享角点。
        let pad = Vec3::splat(self.source.cell_size_meters * 16.0);
        let lo = grid(self.domain, bounds.min - pad);
        let hi = grid(self.domain, bounds.max + pad);
        let mut stack = vec![(0u64, 0u8, [0u32; 3], [2_097_151u32; 3])];
        while let Some((prefix, depth, a, b)) = stack.pop() {
            if (0..3).any(|axis| b[axis] < lo[axis] || a[axis] > hi[axis]) {
                continue;
            }
            let bits = 63 - u32::from(depth) * 3;
            let end = prefix
                | if bits == 63 {
                    u64::MAX >> 1
                } else {
                    (1u64 << bits) - 1
                };
            let start = self.edits.partition_point(|key| *key < prefix);
            if self.edits.get(start).is_none_or(|key| *key > end) {
                continue;
            }
            if depth == 21 || (0..3).all(|axis| lo[axis] <= a[axis] && hi[axis] >= b[axis]) {
                return true;
            }
            let half = 1u32 << (20 - depth);
            for slot in 0..8u32 {
                let mut c = a;
                let mut d = b;
                for axis in 0..3 {
                    if slot & (1 << axis) != 0 {
                        c[axis] += half;
                    } else {
                        d[axis] -= half;
                    }
                }
                stack.push((prefix | (u64::from(slot) << (bits - 3)), depth + 1, c, d));
            }
        }
        false
    }
    fn occupied(&self, bounds: Bounds) -> bool {
        if self.edited(bounds) {
            return true;
        }
        let solid = Bounds {
            min: Vec3::from_array(self.source.min),
            max: Vec3::from_array(self.source.max),
        };
        bounds.overlaps(solid)
            && !(bounds.min.cmpgt(solid.min).all() && bounds.max.cmplt(solid.max).all())
    }
}
fn grid(domain: Bounds, point: Vec3) -> [u32; 3] {
    (((point - domain.min) / (domain.max - domain.min)) * 2_097_151.0)
        .clamp(Vec3::ZERO, Vec3::splat(2_097_151.0))
        .as_uvec3()
        .to_array()
}
fn morton(point: [u32; 3]) -> u64 {
    let mut result = 0;
    for bit in 0..21 {
        for axis in 0..3 {
            result |= u64::from((point[axis] >> bit) & 1) << (bit * 3 + axis);
        }
    }
    result
}
impl Chunk {
    pub fn bounds(self, domain: Bounds) -> Bounds {
        let extent = (domain.max - domain.min) / (1u32 << self.depth) as f32;
        let min = domain.min + Vec3::from_array(self.key.map(|v| v as f32)) * extent;
        Bounds {
            min,
            max: min + extent,
        }
    }
    pub fn children(self) -> [Self; 8] {
        std::array::from_fn(|slot| Self {
            depth: self.depth + 1,
            key: std::array::from_fn(|axis| self.key[axis] * 2 + ((slot >> axis) & 1) as u32),
        })
    }
}
pub(super) struct Plan {
    pub chunks: Vec<Chunk>,
    pub limited: bool,
}

/// 二叉幂嵌套采样允许共享面的细格严格落在粗格上；预算仅决定驻留派生精度，
/// 从不截断源目录或拒绝大对象。最后的远景叶块继续覆盖完整有限对象。
#[cfg(test)]
pub(super) fn plan(
    index: &SourceIndex,
    foci: &[Vec3],
    scale: Vec3,
    lods: &[MapCollisionLod],
    collision: bool,
) -> Plan {
    let maximum = if collision {
        COLLISION_CHUNKS
    } else {
        DISPLAY_CHUNKS
    };
    plan_with_limit(index, foci, scale, lods, collision, maximum)
}
pub(super) fn plan_with_limit(
    index: &SourceIndex,
    foci: &[Vec3],
    scale: Vec3,
    lods: &[MapCollisionLod],
    collision: bool,
    maximum: usize,
) -> Plan {
    let radius = lods.last().map_or(16.0, |row| row.max_distance_meters);
    let distance = |bounds: Bounds| {
        foci.iter()
            .map(|focus| ((focus.clamp(bounds.min, bounds.max) - *focus) * scale).length())
            .fold(f32::INFINITY, f32::min)
    };
    let accept = |chunk: Chunk| {
        let bounds = chunk.bounds(index.domain);
        index.occupied(bounds) && (!collision || distance(bounds) <= radius)
    };
    let root = Chunk {
        depth: 0,
        key: [0; 3],
    };
    let mut chunks = if accept(root) { vec![root] } else { Vec::new() };
    let mut blocked = std::collections::BTreeSet::new();
    let mut limited = false;
    loop {
        let next = chunks
            .iter()
            .enumerate()
            .filter(|(_, chunk)| chunk.depth < 21 && !blocked.contains(*chunk))
            .filter_map(|(offset, chunk)| {
                let bounds = chunk.bounds(index.domain);
                let d = distance(bounds);
                let multiplier = lods
                    .iter()
                    .find(|row| d <= row.max_distance_meters)
                    .or_else(|| lods.last())
                    .map_or(1, |row| row.spacing_multiplier);
                let ratio = (bounds.max - bounds.min).max_element()
                    / (CELLS as f32 * index.source.cell_size_meters * multiplier as f32);
                let player_distance = foci.first().map_or(d, |focus| {
                    ((focus.clamp(bounds.min, bounds.max) - *focus) * scale).length()
                });
                let priority = ratio / (1.0 + d)
                    + if collision {
                        ratio * 2.0 / (1.0 + player_distance)
                    } else {
                        0.0
                    };
                (ratio > 1.0001).then_some((offset, priority))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|v| v.0);
        let Some(offset) = next else {
            break;
        };
        let parent = chunks[offset];
        let children: Vec<_> = parent
            .children()
            .into_iter()
            .filter(|child| accept(*child))
            .collect();
        if chunks.len() - 1 + children.len() > maximum {
            blocked.insert(parent);
            limited = true;
            continue;
        }
        chunks.swap_remove(offset);
        chunks.extend(children);
    }
    chunks.sort_unstable();
    Plan { chunks, limited }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source() -> MapVoxelTerrainSource {
        MapVoxelTerrainSource {
            schema_version: 1,
            map_id: feathrise_map_content::MapId::parse("MAP-window").unwrap(),
            terrain_id: feathrise_map_content::TerrainId::parse("voxel").unwrap(),
            cell_size_meters: 0.25,
            min: [-5000., -2., -5000.],
            max: [5000., 0., 5000.],
            blocks: Default::default(),
            surface_fields_path: None,
        }
    }
    #[test]
    fn huge_thin_box_has_bounded_complete_far_display_and_independent_physics_window() {
        let index = SourceIndex::new(source()).unwrap();
        let lods = feathrise_map_content::default_collision_lods();
        let display = plan(&index, &[Vec3::ZERO], Vec3::ONE, &lods, false);
        let physics = plan(
            &index,
            &[Vec3::new(4000., 0., 4000.)],
            Vec3::ONE,
            &lods,
            true,
        );
        assert!(display.chunks.len() <= DISPLAY_CHUNKS && physics.chunks.len() <= COLLISION_CHUNKS);
        for p in [
            Vec3::new(-5000., 0., -5000.),
            Vec3::new(5000., 0., 5000.),
            Vec3::ZERO,
        ] {
            assert!(
                display
                    .chunks
                    .iter()
                    .any(|chunk| chunk.bounds(index.domain).contains(p))
            );
        }
        assert!(physics.chunks.iter().all(|chunk| {
            let bounds = chunk.bounds(index.domain);
            Vec3::new(4000., 0., 4000.)
                .distance(Vec3::new(4000., 0., 4000.).clamp(bounds.min, bounds.max))
                <= lods.last().unwrap().max_distance_meters
        }));
    }
    #[test]
    fn sparse_edit_index_retains_internal_cave_cells_without_loading_density() {
        let mut source = source();
        source.blocks = vec![feathrise_map_content::MapVoxelDensityBlockRef {
            key: [0, -1, 0],
            sha256: "a".repeat(64),
        }]
        .into();
        let index = SourceIndex::new(source).unwrap();
        assert!(index.edited(Bounds {
            min: Vec3::new(0., -1., 0.),
            max: Vec3::new(1., -0.5, 1.)
        }));
        assert!(!index.edited(Bounds {
            min: Vec3::new(1000., -1., 1000.),
            max: Vec3::new(1001., -0.5, 1001.)
        }));
    }
    #[test]
    fn source_larger_than_the_old_total_block_gate_remains_streamable() {
        let mut source = source();
        source.cell_size_meters = 0.01;
        source.blocks = (-2500..2500)
            .map(|x| feathrise_map_content::MapVoxelDensityBlockRef {
                key: [x, -1, 0],
                sha256: "a".repeat(64),
            })
            .collect::<Vec<_>>()
            .into();
        let index = SourceIndex::new(source).unwrap();
        let display = plan(
            &index,
            &[Vec3::ZERO],
            Vec3::ONE,
            &feathrise_map_content::default_collision_lods(),
            false,
        );
        assert!(display.limited);
        assert!(!display.chunks.is_empty());
        assert!(display.chunks.len() <= DISPLAY_CHUNKS);
        assert_eq!(index.source.blocks.len(), 5000); // 工作集预算不删除远处不可变作者块。
    }
}
