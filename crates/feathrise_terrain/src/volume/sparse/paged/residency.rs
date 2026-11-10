//! 只加载附近预派生网格；焊接及法线在计算池，窗口是完整发布单元。
use super::{store::Store, types::*};
use crate::TerrainMeshInput;
use bevy::math::Vec3;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
};

#[derive(Debug)]
pub struct ResidentVolume {
    pub mesh: Option<TerrainMeshInput>,
    pub footprint: Vec<[[f32; 2]; 2]>,
}
#[derive(Debug)]
pub(super) struct MeshBlock {
    pub bytes: Vec<u8>,
    pub offset: usize,
}
#[derive(Debug)]
pub struct ResidentInput {
    pub(super) blocks: Vec<MeshBlock>,
    pub(super) columns: BTreeSet<[i32; 2]>,
    pub(super) keys: BTreeSet<u64>,
    pub(super) extent: f32,
}
/// IO阶段只按精确键访问，不遍历整个持久树。极陡高度列也有固定读取门。
pub fn load_resident_volume(
    root: &Path,
    volume: &PagedVolume,
    focus: [f32; 3],
    radius: i32,
) -> Result<ResidentInput, String> {
    load_resident_volume_at_foci(root, volume, &[focus], radius)
}
pub fn is_residency_budget_error(error: &str) -> bool {
    [
        "当前驻留窗口对象读取超过32MiB预算",
        "体积索引驻留超过16MiB预算",
        "当前驻留窗口超过32个覆盖矩形预算",
    ]
    .contains(&error)
}
/// 显示/落笔窗口与真实玩家承托窗口最多两份，同一次IO预算、去重和联合发布。
pub fn load_resident_volume_at_foci(
    root: &Path,
    volume: &PagedVolume,
    foci: &[[f32; 3]],
    radius: i32,
) -> Result<ResidentInput, String> {
    volume.validate()?;
    if foci.is_empty()
        || foci.len() > 2
        || foci.iter().any(|p| !Vec3::from_array(*p).is_finite())
        || !(1..=4).contains(&radius)
    {
        return Err("体积驻留焦点或半径无效".into());
    }
    let mut requested: BTreeMap<[i32; 2], BTreeSet<i32>> = BTreeMap::new();
    for &focus in foci {
        let center = (Vec3::from_array(focus) / volume.extent())
            .floor()
            .as_ivec3();
        for z in center.z - radius..=center.z + radius {
            for x in center.x - radius..=center.x + radius {
                requested
                    .entry([x, z])
                    .or_default()
                    .extend(center.y - radius.max(2)..=center.y + radius.max(2));
            }
        }
    }
    // 显示窗口可读取任意数量的格式合法小块；32MiB是编辑批次预算，不能变成场景规模门。
    let mut store = Store::display(root);
    let mut columns = BTreeSet::new();
    let mut blocks = BTreeMap::new();
    for ([x, z], mut ys) in requested {
        let Some(raw) = store.leaf(volume.columns, key([x, 0, z])?, COLUMN_BYTES)? else {
            continue;
        };
        if raw.len() != COLUMN_BYTES {
            return Err("驻留高度列长度无效".into());
        }
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for p in raw.chunks_exact(4) {
            let h = f32::from_le_bytes(p.try_into().unwrap());
            if !h.is_finite() || h.abs() > 100_000. {
                return Err("驻留高度列含无效高度".into());
            }
            min = min.min(h);
            max = max.max(h);
        }
        let low = (min / volume.extent()).floor() as i32 - 1;
        let high = (max / volume.extent()).floor() as i32;
        ys.extend(low..=high);
        for y in ys {
            if let Some(raw) = store.leaf(
                volume.meshes,
                key([x, y, z])?,
                8 + 65_536 * 12 + 131_072 * 12,
            )? {
                blocks.insert([x, y, z], raw);
            }
        }
        columns.insert([x, z]);
    }
    let keys = blocks.keys().map(|p| key(*p)).collect::<Result<_, _>>()?;
    Ok(ResidentInput {
        blocks: blocks
            .into_values()
            .map(|bytes| MeshBlock { bytes, offset: 0 })
            .collect(),
        columns,
        keys,
        extent: volume.extent(),
    })
}
pub fn build_resident_volume(input: ResidentInput) -> Result<ResidentVolume, String> {
    let mut mesh = TerrainMeshInput {
        positions: Vec::new(),
        indices: Vec::new(),
    };
    let mut vertices = HashMap::new();
    for block in input.blocks {
        let bytes = &block.bytes[block.offset..];
        if bytes.len() < 8 {
            return Err("驻留网格块头被截断".into());
        }
        let n = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        let m = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        if n > 65_536 || m > 131_072 * 3 || m % 3 != 0 || bytes.len() != 8 + n * 12 + m * 4 {
            return Err("驻留网格块尺寸无效".into());
        }
        mesh.indices
            .try_reserve(m)
            .map_err(|e| format!("体积显示索引内存分配失败：{e}"))?;
        let mut remap = Vec::with_capacity(n);
        for raw in bytes[8..8 + n * 12].chunks_exact(12) {
            let p = std::array::from_fn::<_, 3, _>(|i| {
                f32::from_le_bytes(raw[i * 4..i * 4 + 4].try_into().unwrap())
            });
            if p.iter().any(|v| !v.is_finite() || v.abs() > 100_000.) {
                return Err("驻留网格顶点无效".into());
            }
            let k = p.map(|v| if v == 0. { 0 } else { v.to_bits() });
            let id = if let Some(&id) = vertices.get(&k) {
                id
            } else {
                // 只保留实际索引格式边界，不再以旧单网格262144项预算拒绝整次雕刻。
                let id = u32::try_from(mesh.positions.len())
                    .map_err(|_| "体积显示顶点超出32位索引格式")?;
                vertices.insert(k, id);
                mesh.positions.push(p);
                id
            };
            remap.push(id);
        }
        for raw in bytes[8 + n * 12..].chunks_exact(4) {
            let i = u32::from_le_bytes(raw.try_into().unwrap()) as usize;
            mesh.indices.push(*remap.get(i).ok_or("驻留网格索引越界")?);
        }
    }
    let footprint = rectangles(&input.columns, input.extent)?;
    let mesh = (!mesh.indices.is_empty()).then_some(mesh);
    Ok(ResidentVolume { mesh, footprint })
}
pub(super) fn rectangles(
    columns: &BTreeSet<[i32; 2]>,
    extent: f32,
) -> Result<Vec<[[f32; 2]; 2]>, String> {
    // 两份需求各至多9×9列，按相同跨度纵向合并；不把断开区域中间挖成洞。
    let mut cells = columns.clone();
    let mut rects = Vec::new();
    while let Some(&[x, z]) = cells.first() {
        let mut max_x = x + 1;
        while cells.contains(&[max_x, z]) {
            max_x += 1;
        }
        let mut max_z = z + 1;
        while (x..max_x).all(|x| cells.contains(&[x, max_z])) {
            max_z += 1;
        }
        for z in z..max_z {
            for x in x..max_x {
                cells.remove(&[x, z]);
            }
        }
        rects.push([
            [x as f32 * extent, z as f32 * extent],
            [max_x as f32 * extent, max_z as f32 * extent],
        ]);
    }
    if rects.len() > 32 {
        return Err("当前驻留窗口超过32个覆盖矩形预算".into());
    }
    Ok(rects)
}
