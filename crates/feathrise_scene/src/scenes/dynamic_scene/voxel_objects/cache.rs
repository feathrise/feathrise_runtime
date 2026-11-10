//! 共享有界块缓存；源摘要和粗邻面合同共同决定身份，不能错误复用已经改变接缝的块。
use super::window::{Chunk, SourceIndex};
use feathrise_terrain::TerrainMeshInput;
use std::{collections::BTreeMap, sync::Arc};
const CACHE_BYTES: usize = 64 * 1024 * 1024;
const CACHE_ENTRIES: usize = 512; // 空网格和缓存身份也计入信用，不能随移动保留无限历史地址。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Key {
    source: [u8; 32],
    chunk: Chunk,
    coarse: Vec<Chunk>,
}
pub(super) fn key(index: &SourceIndex, chunk: Chunk, chunks: &[Chunk]) -> Key {
    let mut coarse = vec![chunk];
    // 边界角点可能继续从另一更粗块取得约束；递归闭包必须进入缓存身份。
    loop {
        let old = coarse.len();
        for &candidate in chunks {
            if !coarse.contains(&candidate)
                && coarse.iter().any(|owner| {
                    candidate.depth < owner.depth
                        && candidate
                            .bounds(index.domain)
                            .overlaps(owner.bounds(index.domain))
                })
            {
                coarse.push(candidate);
            }
        }
        if old == coarse.len() {
            break;
        }
    }
    coarse.sort_unstable();
    Key {
        source: index.digest,
        chunk,
        coarse,
    }
}
#[derive(Default)]
pub(super) struct ChunkCache {
    values: BTreeMap<Key, (u64, usize, Arc<TerrainMeshInput>)>,
    bytes: usize,
    clock: u64,
}
impl ChunkCache {
    pub fn get(&mut self, key: &Key) -> Option<Arc<TerrainMeshInput>> {
        self.clock = self.clock.wrapping_add(1);
        let (clock, _, value) = self.values.get_mut(key)?;
        *clock = self.clock;
        Some(value.clone())
    }
    /// 缓存只用于后台派生；退役的Vec同样在计算池锁内销毁，不落入帧关键路径。
    pub fn insert(&mut self, key: Key, value: TerrainMeshInput) -> Arc<TerrainMeshInput> {
        let bytes = value.positions.capacity() * 12
            + value.indices.capacity() * 4
            + 256
            + key.coarse.len() * 16;
        let value = Arc::new(value);
        if bytes > CACHE_BYTES {
            return value;
        }
        while self.bytes + bytes > CACHE_BYTES || self.values.len() >= CACHE_ENTRIES {
            let Some(old) = self
                .values
                .iter()
                .min_by_key(|(_, entry)| entry.0)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            let (_, retired, _) = self.values.remove(&old).unwrap();
            self.bytes -= retired;
        }
        self.clock = self.clock.wrapping_add(1);
        if let Some((_, old, _)) = self.values.insert(key, (self.clock, bytes, value.clone())) {
            self.bytes -= old;
        }
        self.bytes += bytes;
        value
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_coarse_boundary_changes_the_cached_identity() {
        let source = feathrise_scene_content::SceneVoxelTerrainSource {
            schema_version: feathrise_scene_content::SCENE_SCHEMA_VERSION,
            scene_id: feathrise_scene_content::SceneId::parse("MAP-cache").unwrap(),
            terrain_id: feathrise_scene_content::TerrainId::parse("box").unwrap(),
            cell_size_meters: 0.25,
            min: [-2.; 3],
            max: [2.; 3],
            blocks: Default::default(),
            surface_fields_path: None,
        };
        let index = SourceIndex::new(source).unwrap();
        let fine = Chunk {
            depth: 2,
            key: [2, 0, 0],
        };
        let coarse = Chunk {
            depth: 1,
            key: [0; 3],
        };
        let before = key(&index, fine, &[fine]);
        let after = key(&index, fine, &[coarse, fine]);
        assert!(before != after);
        let mut cache = ChunkCache::default();
        let mesh = cache.insert(
            before.clone(),
            TerrainMeshInput {
                positions: vec![[0.; 3]],
                indices: Vec::new(),
            },
        );
        assert!(Arc::ptr_eq(&mesh, &cache.get(&before).unwrap()));
        assert!(cache.get(&after).is_none());
    }
}
