//! 与精细键互斥地选取粗节点；远处仍有网格和真实覆盖，不能替换回未雕刻高度面。
use super::super::{residency::MeshBlock, store::Store, types::*};
use super::cache::{self, GridRect, Key};
use std::{collections::BTreeSet, path::Path};

pub(super) fn append(
    root: &Path,
    store: &mut Store<'_>,
    source: Hash,
    depth: u32,
    prefix: u64,
    cell_size: f32,
    fine: &BTreeSet<u64>,
    blocks: &mut Vec<MeshBlock>,
    rects: &mut Vec<GridRect>,
) -> Result<(), String> {
    if source == EMPTY {
        return Ok(());
    }
    let key = Key {
        source,
        depth,
        prefix,
        cell_size,
    };
    let end = if depth == 0 {
        u64::MAX
    } else {
        prefix | ((1u64 << ((16 - depth) * 4)) - 1)
    };
    let contains_fine = fine.range(prefix..=end).next().is_some();
    if depth == 16 && contains_fine {
        return Ok(());
    }
    if !contains_fine {
        let hash = cache::read_ref(root, key)?.ok_or("远景代理树尚未完整准备")?;
        if let Some((block, coverage)) = cache::decode(key, store.read(hash, cache::MAX_BLOB)?)? {
            blocks.push(block);
            rects.extend(coverage);
            return Ok(());
        }
        if depth == 16 {
            return Err("远景叶块不能只有分支标记".into());
        }
    }
    for (slot, source) in store.node(source)?.into_iter().enumerate() {
        if source != EMPTY {
            let child = key.child(source, slot);
            append(
                root,
                store,
                source,
                child.depth,
                child.prefix,
                cell_size,
                fine,
                blocks,
                rects,
            )?;
        }
    }
    Ok(())
}
