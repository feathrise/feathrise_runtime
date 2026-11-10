//! 有界密度块缓存与共享面约束采样；每个派生块只冻结33³角点，不读取整个作者层。
use super::window::{CELLS, Chunk, SourceIndex};
use bevy::math::Vec3;
use feathrise_terrain::volume::VOLUME_QUANTIZATION;
use std::{
    collections::{BTreeMap, HashMap},
    io::Read,
    path::Path,
    sync::Arc,
};

const CACHED_BLOCKS: usize = 64; // 两个共享worker各至多4MiB密度驻留；作者总量不是加载门。

pub(super) struct DensityReader {
    index: Arc<SourceIndex>,
    blocks: BTreeMap<[i32; 3], (u64, Arc<[i16]>)>,
    clock: u64,
}
pub(super) struct Lattice {
    nodes: Vec<Node>,
    outputs: Vec<usize>,
    raw: Vec<(usize, Vec3)>,
    requested: BTreeMap<[i32; 3], Vec<(usize, usize, f32, f32)>>,
}
struct Node {
    value: f32,
    density_scale: f32,
    dependencies: Vec<(usize, f32)>,
}

impl DensityReader {
    pub fn new(index: Arc<SourceIndex>) -> Self {
        Self {
            index,
            blocks: BTreeMap::new(),
            clock: 0,
        }
    }
    fn block(&mut self, root: &Path, key: [i32; 3]) -> Result<Option<Arc<[i16]>>, String> {
        self.clock = self.clock.wrapping_add(1);
        if let Some((used, block)) = self.blocks.get_mut(&key) {
            *used = self.clock;
            return Ok(Some(block.clone()));
        }
        let Ok(offset) = self
            .index
            .source
            .blocks
            .binary_search_by_key(&key, |reference| reference.key)
        else {
            return Ok(None);
        };
        let reference = &self.index.source.blocks[offset];
        let path = feathrise_scene_content::authoring_io::safe_content_path(
            root,
            &self.index.source.block_path(&reference.sha256),
        )?;
        let file =
            std::fs::File::open(path).map_err(|error| format!("读取体素密度块失败：{error}"))?;
        let mut bytes = Vec::with_capacity(32usize.pow(3) * 2);
        file.take((32usize.pow(3) * 2 + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() != 32usize.pow(3) * 2
            || feathrise_scene_content::authoring_io::bytes_hash(&bytes) != reference.sha256
        {
            return Err("体素密度块长度或摘要不匹配".into());
        }
        let block: Arc<[i16]> = bytes
            .chunks_exact(2)
            .map(|v| i16::from_le_bytes([v[0], v[1]]))
            .collect::<Vec<_>>()
            .into();
        if self.blocks.len() == CACHED_BLOCKS {
            let retired = self
                .blocks
                .iter()
                .min_by_key(|(_, (used, _))| *used)
                .map(|(key, _)| *key)
                .unwrap();
            self.blocks.remove(&retired);
        }
        self.blocks.insert(key, (self.clock, block.clone()));
        Ok(Some(block))
    }
    /// 按作者块聚合角点需求，一份文件本次只读一次；所有排序/样本枚举已在计算池完成。
    pub fn read(&mut self, root: &Path, mut lattice: Lattice) -> Result<Lattice, String> {
        for (key, points) in std::mem::take(&mut lattice.requested) {
            let Some(block) = self.block(root, key)? else {
                continue;
            };
            for (id, local, baseline, weight) in points {
                let value = block[local] as f32;
                // 稀疏块保存的是绝对密度，未编辑码须抵消回解析盒体；粗LOD不能丢掉薄盒体。
                lattice.nodes[id].value += (value - baseline) * weight;
            }
        }
        Ok(lattice)
    }
}
fn box_density(index: &SourceIndex, point: Vec3) -> f32 {
    let min = Vec3::from_array(index.source.min);
    let max = Vec3::from_array(index.source.max);
    let d = (point - (min + max) * 0.5).abs() - (max - min) * 0.5;
    let distance = d.max(Vec3::ZERO).length() + d.max_element().min(0.0);
    distance / index.source.cell_size_meters * VOLUME_QUANTIZATION
}

/// 与既有000→111四面体一致的连续插值，不采用双线性曲面替代三角共享面。
fn tetrahedron(fraction: Vec3) -> [([i32; 3], f32); 4] {
    let values = fraction.to_array();
    let mut axes = [0usize, 1, 2];
    axes.sort_by(|a, b| values[*b].total_cmp(&values[*a]).then_with(|| a.cmp(b)));
    let mut a = [0; 3];
    a[axes[0]] = 1;
    let mut b = a;
    b[axes[1]] = 1;
    [
        ([0; 3], 1.0 - values[axes[0]]),
        (a, values[axes[0]] - values[axes[1]]),
        (b, values[axes[1]] - values[axes[2]]),
        ([1; 3], values[axes[2]]),
    ]
}

/// 【功能设计】任何共享角点由接触它的最粗叶块定义，再在该叶块四面体内插值。
/// 细面因此是同一个粗三角面的细分；不能独立量化/简化边界，也不依赖隐藏裙边遮缝。
pub(super) fn plan_lattice(index: &SourceIndex, chunk: Chunk, chunks: &[Chunk]) -> Lattice {
    struct Planner<'a> {
        index: &'a SourceIndex,
        chunks: &'a [Chunk],
        lattice: Lattice,
        points: HashMap<[u32; 3], usize>,
    }
    impl Planner<'_> {
        fn point(&mut self, unit: Vec3, depth: u8) -> usize {
            let key = unit
                .to_array()
                .map(|value| if value == 0.0 { 0 } else { value.to_bits() });
            if let Some(&id) = self.points.get(&key) {
                return id;
            }
            let owner = self
                .chunks
                .iter()
                .filter(|chunk| chunk.depth < depth)
                .find(|chunk| {
                    let p = unit * (1u32 << chunk.depth) as f32
                        - Vec3::from_array(chunk.key.map(|v| v as f32));
                    p.cmpge(Vec3::ZERO).all() && p.cmple(Vec3::ONE).all()
                })
                .copied();
            let mut dependencies = Vec::new();
            if let Some(owner) = owner {
                let side = (1u32 << owner.depth) as f32 * CELLS as f32;
                let grid = unit * side;
                let floor = grid.floor();
                for (corner, weight) in tetrahedron(grid - floor) {
                    if weight > 0.0 {
                        let at = (floor + Vec3::from_array(corner.map(|v| v as f32))) / side;
                        dependencies.push((self.point(at, owner.depth), weight));
                    }
                }
            }
            let id = self.lattice.nodes.len();
            if dependencies.is_empty() {
                self.lattice.raw.push((
                    id,
                    self.index.domain.min + unit * (self.index.domain.max - self.index.domain.min),
                ));
            }
            let point =
                self.index.domain.min + unit * (self.index.domain.max - self.index.domain.min);
            let physical = box_density(self.index, point);
            let min = Vec3::from_array(self.index.source.min);
            let max = Vec3::from_array(self.index.source.max);
            let d = ((point - (min + max) * 0.5).abs() - (max - min) * 0.5) / (max - min);
            let normalized = d.max(Vec3::ZERO).length() + d.max_element().min(0.0);
            // 正数预条件处理不移动连续密度的零点；它防止薄盒体的粗横格被最薄轴量程
            // 压成巨大倒角。作者量化码和编辑差值仍在原米制量程中计算。
            let density_scale = if physical.abs() > 1e-20 {
                (normalized / physical).abs().max(f32::MIN_POSITIVE)
            } else {
                1.0
            };
            self.lattice.nodes.push(Node {
                value: physical,
                density_scale,
                dependencies,
            });
            self.points.insert(key, id);
            id
        }
    }
    let mut planner = Planner {
        index,
        chunks,
        points: HashMap::new(),
        lattice: Lattice {
            nodes: Vec::new(),
            outputs: Vec::with_capacity(33usize.pow(3)),
            raw: Vec::new(),
            requested: BTreeMap::new(),
        },
    };
    let side = ((1u32 << chunk.depth) * CELLS) as f32;
    for z in 0..=CELLS {
        for y in 0..=CELLS {
            for x in 0..=CELLS {
                let unit = Vec3::from_array(std::array::from_fn(|axis| {
                    (chunk.key[axis] * CELLS + [x, y, z][axis]) as f32
                })) / side;
                let id = planner.point(unit, chunk.depth);
                planner.lattice.outputs.push(id);
            }
        }
    }
    for &(id, point) in &planner.lattice.raw {
        let grid = point / index.source.cell_size_meters;
        let floor = grid.floor().as_ivec3();
        for (corner, weight) in tetrahedron(grid - floor.as_vec3()) {
            if weight <= 0.0 {
                continue;
            }
            let p = (floor + bevy::math::IVec3::from_array(corner)).to_array();
            let key = p.map(|v| v.div_euclid(32));
            if index
                .source
                .blocks
                .binary_search_by_key(&key, |reference| reference.key)
                .is_ok()
            {
                let local = p.map(|v| v.rem_euclid(32) as usize);
                let baseline = box_density(
                    index,
                    Vec3::from_array(p.map(|v| v as f32)) * index.source.cell_size_meters,
                )
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32);
                planner.lattice.requested.entry(key).or_default().push((
                    id,
                    (local[2] * 32 + local[1]) * 32 + local[0],
                    baseline,
                    weight,
                ));
            }
        }
    }
    planner.lattice.raw.clear();
    planner.lattice
}
pub(super) fn finish_lattice(mut lattice: Lattice) -> Vec<f32> {
    // 依赖先插入：固定DAG拓扑序计算，不递归读取文件或在worker里创建无界任务。
    for id in 0..lattice.nodes.len() {
        if !lattice.nodes[id].dependencies.is_empty() {
            lattice.nodes[id].value = lattice.nodes[id]
                .dependencies
                .iter()
                .map(|(parent, weight)| lattice.nodes[*parent].value * weight)
                .sum();
        } else {
            lattice.nodes[id].value *= lattice.nodes[id].density_scale;
        }
    }
    lattice
        .outputs
        .into_iter()
        .map(|id| lattice.nodes[id].value)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fine_and_coarse_faces_query_the_same_continuous_tetrahedra() {
        let source = feathrise_scene_content::SceneVoxelTerrainSource {
            schema_version: feathrise_scene_content::SCENE_SCHEMA_VERSION,
            scene_id: feathrise_scene_content::SceneId::parse("MAP-seam").unwrap(),
            terrain_id: feathrise_scene_content::TerrainId::parse("box").unwrap(),
            cell_size_meters: 0.5,
            min: [-2., -2., -2.],
            max: [2., 0., 2.],
            blocks: Default::default(),
            surface_fields_path: None,
        };
        let index = SourceIndex::new(source).unwrap();
        let coarse = Chunk {
            depth: 1,
            key: [0, 0, 0],
        };
        let fine = Chunk {
            depth: 2,
            key: [2, 0, 0],
        };
        let chunks = [coarse, fine];
        let mut reader = DensityReader::new(Arc::new(index.clone()));
        let coarse_values = finish_lattice(
            reader
                .read(Path::new("."), plan_lattice(&index, coarse, &chunks))
                .unwrap(),
        );
        let fine_values = finish_lattice(
            reader
                .read(Path::new("."), plan_lattice(&index, fine, &chunks))
                .unwrap(),
        );
        for z in 0..=32usize {
            for y in 0..=32usize {
                let gy = y as f32 * 0.5;
                let gz = z as f32 * 0.5;
                let base = [gy.floor() as usize, gz.floor() as usize];
                let expected = tetrahedron(Vec3::new(0., gy.fract(), gz.fract()))
                    .into_iter()
                    .map(|(p, w)| {
                        coarse_values
                            [((base[1] + p[2] as usize) * 33 + base[0] + p[1] as usize) * 33 + 32]
                            * w
                    })
                    .sum::<f32>();
                assert!((fine_values[(z * 33 + y) * 33] - expected).abs() < 0.001);
            }
        }
        assert!(reader.blocks.is_empty()); // 初始盒体无需任何密度资产。
    }
}
