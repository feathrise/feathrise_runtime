//! 首次后台补齐代理树；后续只构建网格摘要发生变化的路径，不扫描全部密度。
use super::super::{
    residency::{MeshBlock, ResidentInput, build_resident_volume, is_residency_budget_error},
    store::Store,
    types::*,
};
use super::{
    cache::{self, Key},
    pulse,
};
use crate::{TerrainMeshInput, volume::sparse::simplify::simplify_display_mesh};
use bevy::tasks::AsyncComputeTaskPool;
use std::{future::Future, path::Path, pin::Pin, sync::atomic::AtomicU64};

pub(super) fn prepare<'a>(
    root: &'a Path,
    source: Hash,
    depth: u32,
    prefix: u64,
    cell_size: f32,
    progress: Option<&'a AtomicU64>,
) -> Pin<Box<dyn Future<Output = Result<Hash, String>> + Send + 'a>> {
    Box::pin(async move {
        if source == EMPTY {
            return Ok(EMPTY);
        }
        let key = Key {
            source,
            depth,
            prefix,
            cell_size,
        };
        if let Some(hash) = cache::read_ref(root, key)? {
            pulse(progress);
            return Ok(hash);
        }
        let mut children = Vec::new();
        let mut raw = None;
        if depth == 16 {
            raw = Some(Store::new(root).read(source, cache::MAX_BLOB)?);
        } else {
            let node = Store::new(root).node(source)?;
            // 递归阶段只保留小型引用；不会在每一层同时持有十六份大型子网格。
            for (slot, source) in node.into_iter().enumerate() {
                if source != EMPTY {
                    let child = key.child(source, slot);
                    let hash =
                        prepare(root, source, child.depth, child.prefix, cell_size, progress)
                            .await?;
                    children.push((child, hash));
                }
            }
        }
        pulse(progress);
        let mut blocks = Vec::new();
        let mut rects = Vec::new();
        let mut can_merge = true;
        if let Some(bytes) = raw {
            let [x, _, z] = cache::coordinates(prefix);
            rects.push([[x, z], [x + 1, z + 1]]);
            blocks.push(MeshBlock { bytes, offset: 0 });
        } else {
            let mut store = Store::new(root);
            for (child, hash) in children {
                let bytes = match store.read(hash, cache::MAX_BLOB) {
                    Ok(b) => b,
                    Err(e) if is_residency_budget_error(&e) => {
                        can_merge = false;
                        break;
                    }
                    Err(e) => return Err(e),
                };
                if let Some((block, coverage)) = cache::decode(child, bytes)? {
                    blocks.push(block);
                    rects.extend(coverage);
                } else {
                    can_merge = false;
                    break;
                }
            }
        }
        let bytes = AsyncComputeTaskPool::get()
            .spawn(async move {
                if !can_merge {
                    return Ok(cache::encode(key, None, Vec::new()));
                }
                let Some(rects) = cache::merge_rectangles(rects, 32) else {
                    return Ok(cache::encode(key, None, Vec::new()));
                };
                let input = ResidentInput {
                    blocks,
                    columns: Default::default(),
                    keys: Default::default(),
                    extent: cell_size * 32.,
                };
                let merged = match build_resident_volume(input) {
                    Ok(v) => v.mesh.unwrap_or(TerrainMeshInput {
                        positions: Vec::new(),
                        indices: Vec::new(),
                    }),
                    Err(e) if is_residency_budget_error(&e) => {
                        return Ok(cache::encode(key, None, Vec::new()));
                    }
                    Err(e) => return Err(e),
                };
                // 叶块、父层、祖父层分别分配1/4、1/4、1/2格距；更高层只合并共面冗余。
                // 避免十六层重复简化各消费一整格额度，导致远景偏移随树深度累积。
                let error = cell_size
                    * match depth {
                        16 | 15 => 0.25,
                        14 => 0.5,
                        _ => 0.,
                    };
                let mesh = simplify_display_mesh(merged, error)?;
                if mesh.positions.len() > 65_536 || mesh.indices.len() > 131_072 * 3 {
                    return Ok(cache::encode(key, None, Vec::new()));
                }
                Ok::<_, String>(cache::encode(key, Some(mesh), rects))
            })
            .await?;
        let hash = cache::publish(root, key, &bytes)?;
        pulse(progress);
        Ok(hash)
    })
}
