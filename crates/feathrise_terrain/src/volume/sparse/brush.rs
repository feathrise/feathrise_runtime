//! 笔刷自动分配邻块，内部没有保护墙；缺页或超预算时整段零采用。
use super::super::{mesh::build_density_mesh, types::*};
use super::types::*;
use bevy::math::{Rect, Vec2, Vec3};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub fn new_sparse_volume(
    step: f32,
    budget: Arc<VolumeMemoryBudget>,
) -> Result<SparseVolume, String> {
    let result = SparseVolume {
        cell_size: step,
        mesh_recipe: MeshRecipe::CompactV1,
        columns: BTreeMap::new(),
        blocks: BTreeMap::new(),
        meshes: BTreeMap::new(),
        budget,
        footprint: Arc::default(),
        last_rebuilt_blocks: 0,
        last_rebuilt_keys: Arc::default(),
    };
    result.validate()?;
    Ok(result)
}

pub fn apply_sparse_brush(
    source: &SparseVolume,
    brush: VolumeBrush,
    bounds: Rect,
    height: impl FnMut(Vec2) -> Result<f32, String>,
) -> Result<SparseVolume, String> {
    apply_sparse_brush_batch(
        source,
        crate::volume::VolumeBrushBatch::single(brush, source.cell_size)?,
        bounds,
        height,
    )
}

/// 长笔划每批只读写局部窗口，全局序号保证平滑/低强度整平在交界处不会加倍。
pub fn apply_sparse_brush_batch(
    source: &SparseVolume,
    batch: crate::volume::VolumeBrushBatch,
    bounds: Rect,
    mut height: impl FnMut(Vec2) -> Result<f32, String>,
) -> Result<SparseVolume, String> {
    source.validate()?;
    let brush = batch.brush;
    let scope = batch.scope();
    let (from, to) = (Vec3::from_array(scope.from), Vec3::from_array(scope.to));
    if !from.is_finite()
        || !to.is_finite()
        || !brush.radius.is_finite()
        || !(brush.world_step(source.cell_size)..=8.0).contains(&brush.radius)
        || !brush.shape_valid(source.cell_size, 8.0)
        || !brush.strength.is_finite()
        || !(0.01..=1.0).contains(&brush.strength)
        || !bounds.min.is_finite()
        || !bounds.max.is_finite()
        || from.abs().max_element() > 100_000.
        || to.abs().max_element() > 100_000.
        || batch.cell_size != source.cell_size
    {
        return Err("分块三维笔刷参数无效".into());
    }
    if batch.last - batch.first > 256 {
        return Err("三维笔刷本段过长，请连续移动而不是跳过大片地形".into());
    }
    let (low, high) = batch.bounds();
    let min = ((low - Vec3::splat(source.cell_size)) / source.extent())
        .floor()
        .as_ivec3();
    let max = ((high + Vec3::splat(source.cell_size)) / source.extent())
        .floor()
        .as_ivec3();
    let n = max - min + 1;
    if n.x as i64 * n.y as i64 * n.z as i64 > 128 {
        return Err("本段笔刷影响超过128个体积块，请缩小半径或缩短拖动距离".into());
    }
    let mut next = source.clone();
    let mut dirty = BTreeSet::new();
    let mut any_edit = false;
    for z in min.z..=max.z {
        for x in min.x..=max.x {
            let key = [x, z];
            if next.columns.contains_key(&key) {
                continue;
            }
            let low = Vec2::new(x as f32, z as f32) * source.extent();
            let high = low + Vec2::splat(source.extent());
            if low.cmplt(bounds.min).any() || high.cmpgt(bounds.max).any() {
                return Err("三维笔刷已经到达场景边界，不能生成场景外的作者块".into());
            }
            if next.columns.len() == MAX_COLUMNS {
                return Err("当前体积层达到256个地表列预算，保留上一完整地形".into());
            }
            let mut heights = Vec::with_capacity(33 * 33);
            for iz in 0..=32 {
                for ix in 0..=32 {
                    let lattice = [x * 32 + ix, z * 32 + iz];
                    // 扩列继承旧列的冻结共享边，作者高度后来改变也不能撕开既有接缝。
                    heights.push(match next.height(lattice) {
                        Ok(frozen) => frozen,
                        Err(_) => height(low + Vec2::new(ix as f32, iz as f32) * source.cell_size)?,
                    });
                }
            }
            if heights.iter().any(|v| !v.is_finite() || v.abs() > 100_000.) {
                return Err("三维作者基础高度包含无效值或超出高度预算".into());
            }
            let min_y = (heights.iter().copied().fold(f32::INFINITY, f32::min) / source.extent())
                .floor() as i32
                - 1;
            let max_y = (heights.iter().copied().fold(f32::NEG_INFINITY, f32::max)
                / source.extent())
            .floor() as i32;
            if max_y - min_y > 128 {
                return Err("本列高差超出后台重建预算".into());
            }
            for y in min_y..=max_y {
                dirty.insert([x, y, z]);
            }
            let bytes = heights.len() * 4;
            next.columns
                .insert(key, next.budget.retain(heights, bytes)?);
        }
    }
    for brush in batch.brushes() {
        let center = Vec3::from_array(brush.from);
        let domain = brush.sample_half_extents(source.cell_size);
        let min = ((center - domain) / next.extent()).floor().as_ivec3();
        let max = ((center + domain) / next.extent()).floor().as_ivec3();
        let old = next.clone(); // 单次平滑读取同一不可变邻域；块数组仅在后台写侧复制。
        for z in min.z..=max.z {
            for y in min.y..=max.y {
                for x in min.x..=max.x {
                    let key = [x, y, z];
                    let mut samples = Vec::with_capacity(32usize.pow(3));
                    let mut changed = false;
                    for iz in 0..32 {
                        for iy in 0..32 {
                            for ix in 0..32 {
                                let p = [x * 32 + ix, y * 32 + iy, z * 32 + iz];
                                let current = old.sample(p)?;
                                let position =
                                    Vec3::from_array(p.map(|v| v as f32)) * next.cell_size;
                                let relative = position - center;
                                let sdf = brush.signed_distance(relative);
                                let mut value = current;
                                if brush.accepts(sdf, next.cell_size) {
                                    let current_f =
                                        current as f32 * next.cell_size / VOLUME_QUANTIZATION;
                                    let desired = match brush.kind {
                                        VolumeBrushKind::Dig => current_f.max(-sdf),
                                        VolumeBrushKind::Fill => current_f.min(sdf),
                                        VolumeBrushKind::Flatten => {
                                            brush.flatten_distance(position)
                                        }
                                        VolumeBrushKind::Smooth => {
                                            let mut sum = 0.;
                                            for axis in 0..3 {
                                                for direction in [-1, 1] {
                                                    let mut q = p;
                                                    q[axis] += direction;
                                                    sum += old.sample(q)? as f32;
                                                }
                                            }
                                            sum / 6.0 * next.cell_size / VOLUME_QUANTIZATION
                                        }
                                    };
                                    let factor = brush.weight(relative, next.cell_size);
                                    let edited = current_f + (desired - current_f) * factor;
                                    value = quantize(edited, next.cell_size);
                                    // 洞道相交处量化零平台会形成四面共用的夹边；改变过的样本保留至少一级符号。
                                    // 未改写的基础零高度仍精确落在原地表上，避免接管外缘被整体抬高。
                                    if value == 0 && current != 0 {
                                        value = if edited >= 0. { 1 } else { -1 };
                                    }
                                    changed |= value != current;
                                }
                                samples.push(value);
                            }
                        }
                    }
                    if changed {
                        any_edit = true;
                        if !next.blocks.contains_key(&key) && next.blocks.len() == MAX_BLOCKS {
                            return Err("当前体积层达到512个密度块预算，保留上一完整地形".into());
                        }
                        let bytes = samples.len() * 2;
                        next.blocks.insert(key, next.budget.retain(samples, bytes)?);
                        // 正边界格点由相邻块拥有，受影响单元还包括七个负方向邻块。
                        for dz in [0, -1] {
                            for dy in [0, -1] {
                                for dx in [0, -1] {
                                    dirty.insert([x + dx, y + dy, z + dz]);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if !any_edit {
        return Ok(source.clone());
    } // 平地平滑等无效落笔不能仅因预取列就替换原地表。
    if dirty.len() > 1024 {
        return Err("单段地表与共享边重建超过1024块预算，保留上一完整地形".into());
    }
    // 旧v2第一次有效编辑统一升级全部缓存，不能混用两种配方后编码成同一个新文件。
    if next.mesh_recipe == MeshRecipe::Legacy {
        next.mesh_recipe = MeshRecipe::CompactV1;
        rebuild_all(&mut next)?;
    } else {
        rebuild(&mut next, dirty)?;
    }
    Ok(next)
}

pub(super) fn rebuild(data: &mut SparseVolume, keys: BTreeSet<[i32; 3]>) -> Result<(), String> {
    data.last_rebuilt_blocks = 0;
    data.last_rebuilt_keys = keys.iter().copied().collect::<Vec<_>>().into();
    for key in keys {
        if !data.columns.contains_key(&[key[0], key[2]]) {
            continue;
        }
        let mut samples = Vec::with_capacity(33usize.pow(3));
        for z in 0..=32 {
            for y in 0..=32 {
                for x in 0..=32 {
                    samples.push(data.sample([
                        key[0] * 32 + x,
                        key[1] * 32 + y,
                        key[2] * 32 + z,
                    ])?);
                }
            }
        }
        let mesh = build_density_mesh(
            32,
            data.cell_size,
            samples,
            Vec3::from_array(key.map(|v| v as f32)) * data.extent(),
        )?;
        let mesh = if data.mesh_recipe == MeshRecipe::CompactV1 {
            super::simplify::simplify_chunk_mesh(mesh, data.cell_size)?
        } else {
            mesh
        };
        data.last_rebuilt_blocks += 1;
        if mesh.indices.is_empty() {
            data.meshes.remove(&key);
        } else {
            // Vec几何缓冲可能预留多于len的空间，撤销额度按真实请求容量计费。
            let bytes = mesh.positions.capacity() * 12 + mesh.indices.capacity() * 4;
            data.meshes.insert(key, data.budget.retain(mesh, bytes)?);
        }
    }
    data.footprint = rectangles(data)?.into();
    data.validate()
}

pub(super) fn rebuild_all(data: &mut SparseVolume) -> Result<(), String> {
    let mut keys = BTreeSet::new();
    for (key, h) in &data.columns {
        let min =
            (h.iter().copied().fold(f32::INFINITY, f32::min) / data.extent()).floor() as i32 - 1;
        let max =
            (h.iter().copied().fold(f32::NEG_INFINITY, f32::max) / data.extent()).floor() as i32;
        if max - min > 128 {
            return Err("基础高度列超过重建预算".into());
        }
        for y in min..=max {
            keys.insert([key[0], y, key[1]]);
        }
    }
    for key in data.blocks.keys() {
        for dz in [0, -1] {
            for dy in [0, -1] {
                for dx in [0, -1] {
                    keys.insert([key[0] + dx, key[1] + dy, key[2] + dz]);
                }
            }
        }
    }
    if keys.len() > 4096 {
        return Err("三维作者源完整重建超过4096块后台预算".into());
    }
    rebuild(data, keys)
}

fn rectangles(data: &SparseVolume) -> Result<Vec<[[f32; 2]; 2]>, String> {
    let mut rows: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
    for &[x, z] in data.columns.keys() {
        rows.entry(z).or_default().push(x);
    }
    let mut active: BTreeMap<(i32, i32), (i32, i32)> = BTreeMap::new();
    let mut rects = Vec::new();
    for (z, xs) in rows {
        let mut spans = BTreeSet::new();
        let mut start = xs[0];
        let mut end = start;
        for x in xs.into_iter().skip(1) {
            if x == end + 1 {
                end = x;
            } else {
                spans.insert((start, end));
                start = x;
                end = x;
            }
        }
        spans.insert((start, end));
        let ended: Vec<_> = active
            .iter()
            .filter(|(k, (_, last))| !spans.contains(k) || *last + 1 != z)
            .map(|(k, v)| (*k, *v))
            .collect();
        for (k, (first, last)) in ended {
            active.remove(&k);
            rects.push([
                [k.0 as f32, first as f32],
                [(k.1 + 1) as f32, (last + 1) as f32],
            ]);
        }
        for span in spans {
            active.entry(span).and_modify(|v| v.1 = z).or_insert((z, z));
        }
    }
    for (k, (first, last)) in active {
        rects.push([
            [k.0 as f32, first as f32],
            [(k.1 + 1) as f32, (last + 1) as f32],
        ]);
    }
    if rects.len() > 32 {
        return Err("地表列合并后仍超过32个活动覆盖矩形，保留上一完整地形".into());
    }
    for rect in &mut rects {
        for p in rect {
            for v in p {
                *v *= data.extent();
            }
        }
    }
    rects.sort_by(|a, b| {
        a[0][0]
            .total_cmp(&b[0][0])
            .then(a[0][1].total_cmp(&b[0][1]))
    });
    Ok(rects)
}

pub fn migrate_dense_volume(
    source: &VolumeData,
    budget: Arc<VolumeMemoryBudget>,
) -> Result<SparseVolume, String> {
    source.validate()?;
    if source.cells % 32 != 0 {
        return Err("旧三维源的边长不是32格的整数倍，不能无损迁移为分块作者层".into());
    }
    let mut next = new_sparse_volume(source.cell_size, budget)?;
    let count = source.cells.div_ceil(32) as i32;
    for z in 0..count {
        for x in 0..count {
            let mut heights = Vec::with_capacity(33 * 33);
            for iz in 0..=32 {
                for ix in 0..=32 {
                    heights.push(
                        source.base_heights[((z * 32 + iz).min(source.cells as i32) as usize)
                            * source.side()
                            + (x * 32 + ix).min(source.cells as i32) as usize],
                    );
                }
            }
            next.columns
                .insert([x, z], next.budget.retain(heights, 33 * 33 * 4)?);
            for y in 0..count {
                let mut samples = Vec::with_capacity(32usize.pow(3));
                let mut edited = false;
                for iz in 0..32 {
                    for iy in 0..32 {
                        for ix in 0..32 {
                            let p = [x * 32 + ix, y * 32 + iy, z * 32 + iz];
                            let baseline = quantize(
                                p[1] as f32 * source.cell_size - next.height([p[0], p[2]])?,
                                source.cell_size,
                            );
                            let v = if p.iter().all(|v| *v <= source.cells as i32) {
                                source.density
                                    [source.index(p[0] as usize, p[1] as usize, p[2] as usize)]
                            } else {
                                baseline
                            };
                            edited |= v != baseline;
                            samples.push(v);
                        }
                    }
                }
                if edited {
                    next.blocks
                        .insert([x, y, z], next.budget.retain(samples, 32usize.pow(3) * 2)?);
                }
            }
        }
    }
    rebuild_all(&mut next)?;
    Ok(next)
}
