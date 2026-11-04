//! 只读取笔刷及接缝邻域；持久写集不包含远处作者数据或历史网格。
use super::super::{SparseVolume, VolumeMemoryBudget, new_sparse_volume, types::MAX_BLOCKS};
use super::{store::Store, types::*};
use crate::volume::{VolumeBrush, VolumeBrushBatch};
use bevy::math::Vec3;
use std::{collections::BTreeSet, path::Path, sync::Arc};

pub fn load_edit_window(
    root: &Path,
    volume: &PagedVolume,
    brush: VolumeBrush,
    budget: Arc<VolumeMemoryBudget>,
) -> Result<SparseVolume, String> {
    volume.validate()?;
    let from = Vec3::from_array(brush.from);
    let to = Vec3::from_array(brush.to);
    if !from.is_finite()
        || !to.is_finite()
        || !brush.radius.is_finite()
        || !(brush.world_step(volume.cell_size)..=8.).contains(&brush.radius)
        || !brush.shape_valid(volume.cell_size, 8.)
    {
        return Err("流送体积笔刷无效".into());
    }
    let halo = brush.sample_half_extents(volume.cell_size) + Vec3::splat(volume.cell_size);
    let min = ((from.min(to) - halo) / volume.extent()).floor().as_ivec3();
    let max = ((from.max(to) + halo) / volume.extent()).floor().as_ivec3();
    load_edit_box(root, volume, min, max, budget)
}

/// 曲线窗口按本批所有旋转截面的真实包围读取，不使用端点直线猜测范围。
pub fn load_edit_batch_window(
    root: &Path,
    volume: &PagedVolume,
    batch: &VolumeBrushBatch,
    budget: Arc<VolumeMemoryBudget>,
) -> Result<SparseVolume, String> {
    volume.validate()?;
    if batch.cell_size != volume.cell_size
        || batch
            .brushes()
            .any(|brush| !brush.shape_valid(volume.cell_size, 8.))
    {
        return Err("路径局部笔刷规格无效".into());
    }
    let (min, max) = batch.bounds();
    let halo = Vec3::splat(volume.cell_size);
    load_edit_box(
        root,
        volume,
        ((min - halo) / volume.extent()).floor().as_ivec3(),
        ((max + halo) / volume.extent()).floor().as_ivec3(),
        budget,
    )
}

fn load_edit_box(
    root: &Path,
    volume: &PagedVolume,
    min: bevy::math::IVec3,
    max: bevy::math::IVec3,
    budget: Arc<VolumeMemoryBudget>,
) -> Result<SparseVolume, String> {
    let n = max - min + 1;
    if n.x as i64 * n.y as i64 * n.z as i64 > 128 {
        return Err("本段笔刷作用域超过128块，请缩短拖动距离".into());
    }
    let mut store = Store::new(root);
    let mut data = new_sparse_volume(volume.cell_size, budget)?;
    for z in min.z - 1..=max.z + 1 {
        for x in min.x - 1..=max.x + 1 {
            if let Some(raw) = store.leaf(volume.columns, key([x, 0, z])?, COLUMN_BYTES)? {
                if raw.len() != COLUMN_BYTES {
                    return Err("冻结高度列长度无效".into());
                }
                let h = raw
                    .chunks_exact(4)
                    .map(|p| f32::from_le_bytes(p.try_into().unwrap()))
                    .collect::<Vec<_>>();
                data.columns
                    .insert([x, z], data.budget.retain(h, COLUMN_BYTES)?);
            }
        }
    }
    let mut keys = BTreeSet::new();
    for (&[x, z], h) in &data.columns {
        let low =
            (h.iter().copied().fold(f32::INFINITY, f32::min) / volume.extent()).floor() as i32 - 1;
        let high =
            (h.iter().copied().fold(f32::NEG_INFINITY, f32::max) / volume.extent()).floor() as i32;
        if high - low > 128 {
            return Err("冻结高度列高差超过编辑驻留预算".into());
        }
        // 深处扩列时也必须读取共享地表边的密度，不能把已挖开的邻列顶部重新封死。
        for y in (low - 1..=high + 1).chain(min.y - 1..=max.y + 1) {
            keys.insert([x, y, z]);
        }
    }
    for [x, y, z] in keys {
        if let Some(raw) = store.leaf(volume.density, key([x, y, z])?, DENSITY_BYTES)? {
            if raw.len() != DENSITY_BYTES || data.blocks.len() == MAX_BLOCKS {
                return Err("笔刷邻域密度缓存超出512块驻留预算".into());
            }
            let values = raw
                .chunks_exact(2)
                .map(|p| i16::from_le_bytes(p.try_into().unwrap()))
                .collect::<Vec<_>>();
            data.blocks
                .insert([x, y, z], data.budget.retain(values, DENSITY_BYTES)?);
        }
    }
    data.validate()?;
    Ok(data)
}

pub fn prepare_delta(
    before: Option<&SparseVolume>,
    after: &SparseVolume,
) -> Result<VolumeDelta, String> {
    after.validate()?;
    let mut delta = VolumeDelta {
        columns: Vec::new(),
        density: Vec::new(),
        meshes: Vec::new(),
        bounds: None,
    };
    for (&p, h) in &after.columns {
        if before.is_some_and(|b| b.columns.get(&p).is_some_and(|old| Arc::ptr_eq(old, h))) {
            continue;
        }
        let bytes = h.iter().flat_map(|v| v.to_le_bytes()).collect();
        delta.columns.push(LeafWrite {
            key: key([p[0], 0, p[1]])?,
            bytes,
        });
        let bounds = [p, [p[0] + 1, p[1] + 1]];
        delta.bounds = Some(union(delta.bounds, bounds));
    }
    for (&p, v) in &after.blocks {
        if before.is_some_and(|b| b.blocks.get(&p).is_some_and(|old| Arc::ptr_eq(old, v))) {
            continue;
        }
        delta.density.push(LeafWrite {
            key: key(p)?,
            bytes: v.iter().flat_map(|v| v.to_le_bytes()).collect(),
        });
    }
    // 旧v1/v2首次迁移没有持久网格根，必须包括远处未重建的缓存；后续只写脏块。
    // 同时包含变空的块，否则填回或贯通后旧表面还会从网格索引读回来。
    let rebuilt: BTreeSet<_> = after
        .last_rebuilt_keys
        .iter()
        .copied()
        .chain(after.meshes.keys().copied().filter(|_| before.is_none()))
        .collect();
    for p in rebuilt {
        let bytes = if let Some(mesh) = after.meshes.get(&p) {
            let mut bytes =
                Vec::with_capacity(8 + mesh.positions.len() * 12 + mesh.indices.len() * 4);
            bytes.extend_from_slice(&(mesh.positions.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&(mesh.indices.len() as u32).to_le_bytes());
            bytes.extend(
                mesh.positions
                    .iter()
                    .flatten()
                    .flat_map(|v| v.to_le_bytes()),
            );
            bytes.extend(mesh.indices.iter().flat_map(|v| v.to_le_bytes()));
            bytes
        } else {
            Vec::new()
        };
        delta.meshes.push(LeafWrite {
            key: key(p)?,
            bytes,
        });
    }
    let bytes: usize = delta
        .columns
        .iter()
        .chain(&delta.density)
        .chain(&delta.meshes)
        .map(|w| w.bytes.len())
        .sum();
    if bytes > MAX_IO_BYTES {
        return Err("本段体积写集超过32MiB驻留预算，未改变作者版本".into());
    }
    Ok(delta)
}
fn union(a: Option<[[i32; 2]; 2]>, b: [[i32; 2]; 2]) -> [[i32; 2]; 2] {
    a.map(|a| {
        [
            [a[0][0].min(b[0][0]), a[0][1].min(b[0][1])],
            [a[1][0].max(b[1][0]), a[1][1].max(b[1][1])],
        ]
    })
    .unwrap_or(b)
}
pub fn write_delta(
    root: &Path,
    previous: &PagedVolume,
    delta: VolumeDelta,
) -> Result<PagedVolume, String> {
    previous.validate()?;
    if delta
        .columns
        .iter()
        .chain(&delta.density)
        .chain(&delta.meshes)
        .try_fold(0usize, |n, w| n.checked_add(w.bytes.len()))
        .is_none_or(|n| n > MAX_IO_BYTES)
    {
        return Err("体积写集超出32MiB预算".into());
    }
    let mut next = previous.clone();
    let mut store = Store::new(root);
    for w in delta.columns {
        next.columns = store.set(next.columns, &w)?;
    }
    for w in delta.density {
        next.density = store.set(next.density, &w)?;
    }
    for w in delta.meshes {
        next.meshes = store.set(next.meshes, &w)?;
    }
    store.flush(&[next.columns, next.density, next.meshes])?;
    if let Some(bounds) = delta.bounds {
        next.bounds = Some(union(next.bounds, bounds));
    }
    next.validate()?;
    Ok(next) // 任一对象失败，调用方仍持有previous；部分新对象未被正式placement引用。
}
