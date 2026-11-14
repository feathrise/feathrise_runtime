//! 与密度/网格独立的三维表面作者块；稳定预设和图层ID随持久根保存。
//! 所有读写与投射由调用方放到现有单飞IO/计算槽，不在主帧扫描作者层。
use super::{store::Store, types::*};
use bevy::math::{Mat3, Vec3};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(super) const MAX_FIELD_BYTES: usize = 2 * 1024 * 1024;
const MAX_BRUSH_BLOCKS: usize = 128;
#[cfg(test)]
#[path = "fields_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeFieldChannel {
    Surface,
    Vegetation,
    Puddles,
}

#[derive(Clone, Debug)]
pub struct VolumeFieldBrush {
    pub channel: VolumeFieldChannel,
    /// 材质使用稳定预设ID，植被使用稳定正式层ID；绝不持久化GPU切片。
    pub id: String,
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub normal: [f32; 3],
    pub radius: f32,
    /// 本次刷印线性权重；持续种植使用flow_exposure，避免后台吞帧改变响应。
    pub strength: f32,
    /// 持续绘制的累计曝光（强度×秒×响应率）；与首次刷印分开，异步合并不能改变半径衰减。
    pub flow_exposure: f32,
    pub erase: bool,
    /// 作者点保存在源坐标，半径保持世界米；整体非均匀缩放不能拉伸实际涂绘范围。
    pub source_to_world: Mat3,
}

impl VolumeFieldBrush {
    fn world_distance(&self, delta: Vec3) -> f32 { (self.source_to_world * delta).length() }
    fn world_normal(&self, normal: Vec3) -> Vec3 { (self.source_to_world.inverse().transpose() * normal).normalize() }
    fn metric_valid(&self) -> bool { self.source_to_world.is_finite() && self.source_to_world.determinant().abs() > 1e-8 }
}

/// 平面R8与三维字段共用的径向响应；先衰减曝光再指数饱和，合并等待帧也不会刷淡边缘。
pub fn field_paint_response(strength: f32, flow_exposure: f32, falloff: f32) -> f32 {
    let falloff = falloff.clamp(0.0, 1.0);
    let stamp = (strength * falloff).clamp(0.0, 1.0);
    stamp + (1.0 - stamp) * -(-flow_exposure.max(0.0) * falloff).exp_m1()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VolumeFieldSample {
    /// 作者点保留真实表面位置；只用格点键索引，不把位置吸回格子中心。
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub surface: Vec<(String, u8)>,
    pub vegetation: Vec<(String, u8)>,
    #[serde(default)]
    pub puddles: u8, // 分层积水潜力与材质/植被独立；旧VFS1块缺省为零。
}

#[derive(Clone, Debug, Default)]
pub struct VolumeFieldSnapshot {
    pub cell_size: f32,
    pub samples: BTreeMap<[i32; 3], VolumeFieldSample>,
    /// 同一空间格可包含薄壁两侧或两个近邻楼板；主样本保持旧查询路径，附加面不覆盖它。
    pub additional_samples: BTreeMap<[i32; 3], Vec<VolumeFieldSample>>,
    base_heights: BTreeMap<[i32; 2], Vec<f32>>,
    geometry: ([u8; 32], [u8; 32]),
    // 原生盒体没有Clipmap冻结列，仍需为自动岩土提供统一的源坐标深度。
    native_top: Option<f32>,
}
impl VolumeFieldSnapshot {
    pub fn empty(cell_size: f32) -> Self { Self { cell_size, ..Self::default() } }
    pub fn iter_samples(&self) -> impl Iterator<Item = &VolumeFieldSample> {
        self.samples.values().chain(self.additional_samples.values().flatten())
    }
    pub fn samples_at(&self, cell: &[i32; 3]) -> impl Iterator<Item = &VolumeFieldSample> {
        self.samples.get(cell).into_iter().chain(self.additional_samples.get(cell).into_iter().flatten())
    }
    pub fn with_native_geometry(mut self, density_identity: [u8; 32], top: f32) -> Self {
        self.geometry = (density_identity, [0; 32]);
        self.native_top = top.is_finite().then_some(top);
        self
    }
    /// 密度与原地表根构成承托身份；刷草和显示LOD换代不能冒充地形雕刻。
    pub fn geometry_identity(&self) -> ([u8; 32], [u8; 32]) {
        self.geometry
    }
    /// 地下深度依据雕刻前冻结表面，水平洞底不会被误判成外部地表。
    pub fn exposed_depth(&self, position: Vec3) -> Option<f32> {
        if self.cell_size <= 0.0 {
            return None;
        }
        let grid = position / self.cell_size;
        let floor = grid.floor();
        let x = floor.x as i32;
        let z = floor.z as i32;
        let Some(values) = self.base_heights.get(&[x.div_euclid(32), z.div_euclid(32)]) else {
            return self.native_top.map(|top| top - position.y);
        };
        let ix = x.rem_euclid(32) as usize;
        let iz = z.rem_euclid(32) as usize;
        let a = values[iz * 33 + ix];
        let b = values[iz * 33 + ix + 1];
        let c = values[(iz + 1) * 33 + ix];
        let d = values[(iz + 1) * 33 + ix + 1];
        let u = grid.x - floor.x;
        let v = grid.z - floor.z;
        let height = if u + v <= 1.0 {
            a + (b - a) * u + (c - a) * v
        } else {
            d + (c - d) * (1.0 - u) + (b - d) * (1.0 - v)
        };
        Some(height - position.y)
    }
    /// 只查相邻八格，按位置与朝向筛选；薄壁反面不能继承另一面的作者材质。
    pub fn sample(&self, position: Vec3, normal: Vec3) -> Option<&VolumeFieldSample> {
        if self.cell_size <= 0.0 {
            return None;
        }
        let cell = (position / self.cell_size).floor().as_ivec3();
        let mut best = None;
        let mut distance = self.cell_size * self.cell_size * 3.0;
        for z in 0..=1 {
            for y in 0..=1 {
                for x in 0..=1 {
                    for value in self.samples_at(&(cell + bevy::math::IVec3::new(x, y, z)).to_array()) {
                        let d = position.distance_squared(Vec3::from_array(value.position));
                        if d < distance && normal.dot(Vec3::from_array(value.normal)) > 0.25 {
                            best = Some(value);
                            distance = d;
                        }
                    }
                }
            }
        }
        best
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Block {
    samples: BTreeMap<u16, VolumeFieldSample>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    additional_samples: BTreeMap<u16, Vec<VolumeFieldSample>>,
}

/// IO冻结的局部作者块；计算worker只操作这些值，不能在密集投射循环中访问磁盘。
pub struct VolumeFieldWindow {
    blocks: BTreeMap<[i32; 3], Vec<u8>>,
}
pub struct VolumeFieldDelta {
    writes: Vec<LeafWrite>,
    focus: [f32; 3],
}

fn brush_bounds(
    volume: &PagedVolume,
    brush: &VolumeFieldBrush,
) -> Result<(bevy::math::IVec3, bevy::math::IVec3), String> {
    let from = Vec3::from_array(brush.from);
    let to = Vec3::from_array(brush.to);
    if !from.is_finite()
        || !to.is_finite()
        || !brush.radius.is_finite()
        || !(0.1..=8.0).contains(&brush.radius)
        || !brush.strength.is_finite()
        || !(0.0..=1.0).contains(&brush.strength)
        || !brush.flow_exposure.is_finite()
        || brush.flow_exposure < 0.0
        || brush.id.is_empty()
        || brush.id.len() > 256
        || !brush.metric_valid()
    {
        return Err("三维表面笔刷参数无效".into());
    }
    let inverse = brush.source_to_world.inverse();
    let inverse_abs = Mat3::from_cols(inverse.x_axis.abs(), inverse.y_axis.abs(), inverse.z_axis.abs());
    let halo = inverse_abs * Vec3::splat(brush.radius) + Vec3::splat(volume.cell_size * 2.0);
    let min = ((from.min(to) - halo)
        / volume.cell_size)
        .floor()
        .as_ivec3();
    let max = ((from.max(to) + halo)
        / volume.cell_size)
        .ceil()
        .as_ivec3();
    let a = min.div_euclid(bevy::math::IVec3::splat(32));
    let b = max.div_euclid(bevy::math::IVec3::splat(32));
    let size = b - a + bevy::math::IVec3::ONE;
    if i64::from(size.x) * i64::from(size.y) * i64::from(size.z) > MAX_BRUSH_BLOCKS as i64 {
        return Err("三维表面笔刷超过128个作用域块，未改变原数据".into());
    }
    Ok((min, max))
}
pub fn load_field_window(
    root: &Path,
    volume: &PagedVolume,
    brush: &VolumeFieldBrush,
) -> Result<VolumeFieldWindow, String> {
    volume.validate()?;
    let (min, max) = brush_bounds(volume, brush)?;
    let a = min.div_euclid(bevy::math::IVec3::splat(32));
    let b = max.div_euclid(bevy::math::IVec3::splat(32));
    let mut store = Store::new(root);
    let mut blocks = BTreeMap::new();
    for z in a.z..=b.z {
        for y in a.y..=b.y {
            for x in a.x..=b.x {
                let p = [x, y, z];
                let block = store
                    .leaf(volume.fields, key(p)?, MAX_FIELD_BYTES)?
                    .unwrap_or_default();
                blocks.insert(p, block);
            }
        }
    }
    Ok(VolumeFieldWindow { blocks })
}

fn decode(bytes: &[u8]) -> Result<Block, String> {
    if bytes.len() > MAX_FIELD_BYTES || !(bytes.starts_with(b"VFS1") || bytes.starts_with(b"VFS2")) {
        return Err("三维表面作者块格式或预算无效".into());
    }
    let block: Block = serde_json::from_slice(&bytes[4..]).map_err(|e| e.to_string())?;
    for (&index, value) in block.samples.iter().chain(block.additional_samples.iter().flat_map(|(index, values)| values.iter().map(move |value| (index, value)))) {
        if index >= 32768
            || !Vec3::from_array(value.position).is_finite()
            || !Vec3::from_array(value.normal).is_normalized()
            || value.surface.len() > 4
            || value.vegetation.len() > 4
            || value
                .surface
                .iter()
                .map(|(id, _)| id)
                .collect::<BTreeSet<_>>()
                .len()
                != value.surface.len()
            || value
                .vegetation
                .iter()
                .map(|(id, _)| id)
                .collect::<BTreeSet<_>>()
                .len()
                != value.vegetation.len()
            || value
                .surface
                .iter()
                .map(|(_, w)| u32::from(*w))
                .sum::<u32>()
                > 259
            || value
                .surface
                .iter()
                .chain(&value.vegetation)
                .any(|(id, w)| id.is_empty() || id.len() > 256 || *w == 0)
        {
            return Err("三维表面作者样本无效".into());
        }
    }
    Ok(block)
}
fn encode(block: &Block) -> Result<Vec<u8>, String> {
    // 旧单面块仍原样写VFS1；只有多面样本启用VFS2，新读取器同时接受两版。
    let mut raw = if block.additional_samples.is_empty() { b"VFS1".to_vec() } else { b"VFS2".to_vec() };
    raw.extend(serde_json::to_vec(block).map_err(|e| e.to_string())?);
    if raw.len() > MAX_FIELD_BYTES {
        return Err("三维表面作者块超过2MiB预算".into());
    }
    Ok(raw)
}
fn address(cell: [i32; 3]) -> ([i32; 3], u16) {
    let block = cell.map(|v| v.div_euclid(32));
    let p = cell.map(|v| v.rem_euclid(32) as u16);
    (block, p[0] + p[1] * 32 + p[2] * 1024)
}

/// 计算池生成局部读取清单，不在IO线程遍历密集网格或构造作者目录。
pub struct VolumeFieldReadPlan {
    keys: BTreeSet<[i32; 3]>,
    columns: BTreeSet<[i32; 2]>,
    bounds: Option<([i32; 3], [i32; 3])>,
}
pub struct VolumeFieldBytes {
    pub(super) fields: Vec<([i32; 3], Vec<u8>)>,
    pub(super) heights: Vec<([i32; 2], Vec<u8>)>,
}
pub fn plan_surface_fields(
    volume: &PagedVolume,
    positions: &[[f32; 3]],
    foci: &[[f32; 3]],
    radius: u32,
) -> VolumeFieldReadPlan {
    let mut keys = BTreeSet::new();
    let mut columns = BTreeSet::new();
    for &p in positions {
        let cell = (Vec3::from_array(p) / volume.cell_size).floor().as_ivec3();
        columns.insert([cell.x.div_euclid(32), cell.z.div_euclid(32)]);
        for z in 0..=1 {
            for y in 0..=1 {
                for x in 0..=1 {
                    keys.insert(address((cell + bevy::math::IVec3::new(x, y, z)).to_array()).0);
                }
            }
        }
    }
    // 平坦网格可被压成很少顶点，独立种植作者块仍必须覆盖精细驻留窗口内部。
    for f in foci.iter().take(2) {
        let center = (Vec3::from_array(*f) / volume.extent()).floor().as_ivec3();
        let r = radius.min(4) as i32 + 1;
        for z in -r..=r {
            for y in -r..=r {
                for x in -r..=r {
                    keys.insert((center + bevy::math::IVec3::new(x, y, z)).to_array());
                }
            }
        }
    }
    let bounds = positions.first().map(|&first| {
        let mut lo = Vec3::from_array(first);
        let mut hi = lo;
        for &p in positions.iter().skip(1) {
            lo = lo.min(Vec3::from_array(p));
            hi = hi.max(Vec3::from_array(p));
        }
        let lo = (lo / volume.extent()).floor().as_ivec3() - bevy::math::IVec3::ONE;
        let hi = (hi / volume.extent()).floor().as_ivec3() + bevy::math::IVec3::ONE;
        (lo.to_array(), hi.to_array())
    }); // 作者块独立于显示顶点；四角很远的平坦代理也不能丢失中部绘制和种植。
    VolumeFieldReadPlan {
        keys,
        columns,
        bounds,
    }
}
/// 原生体素显示末层可以覆盖很大的有限盒体，但字段只读取当前精细窗口。
/// 不把远景三角包围盒变成整作者树扫描，窗口外字段留在不可变对象库中。
pub fn plan_surface_field_windows(volume: &PagedVolume, foci: &[[f32; 3]], radius: u32) -> VolumeFieldReadPlan {
    let mut plan = plan_surface_fields(volume, &[], foci, radius);
    plan.bounds = None;
    plan
}
/// 有界IO槽只读不可变字节，JSON解码和样本验证留在计算池。
pub fn read_surface_fields(
    root: &Path,
    volume: &PagedVolume,
    plan: VolumeFieldReadPlan,
) -> Result<VolumeFieldBytes, String> {
    volume.validate()?;
    let mut store = Store::display(root);
    let mut fields = Vec::new();
    let mut heights = Vec::new();
    let mut leaves = BTreeMap::new();
    let mut columns = plan.columns;
    if let Some((lo, hi)) = plan.bounds {
        for (p, hash) in store.leaves_in_bounds(volume.fields, lo, hi)? {
            leaves.insert(p, hash);
        }
    }
    for p in plan.keys {
        if !leaves.contains_key(&p)
            && let Some(hash) = store.lookup(volume.fields, key(p)?)?
        {
            leaves.insert(p, hash);
        }
    }
    for (p, hash) in leaves {
        columns.insert([p[0], p[2]]);
        fields.push((p, store.read(hash, MAX_FIELD_BYTES)?));
    }
    for p in columns {
        if let Some(raw) = store.leaf(volume.columns, key([p[0], 0, p[1]])?, COLUMN_BYTES)? {
            heights.push((p, raw));
        }
    }
    Ok(VolumeFieldBytes { fields, heights })
}
pub fn decode_surface_fields(
    volume: &PagedVolume,
    bytes: VolumeFieldBytes,
) -> Result<VolumeFieldSnapshot, String> {
    let mut snapshot = VolumeFieldSnapshot {
        cell_size: volume.cell_size,
        geometry: (volume.density, volume.columns),
        ..Default::default()
    };
    for (p, raw) in bytes.heights {
        if raw.len() != COLUMN_BYTES {
            return Err("三维表面冻结高度长度无效".into());
        }
        let heights: Vec<f32> = raw
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        if heights.iter().any(|h| !h.is_finite()) {
            return Err("三维表面冻结高度含非法值".into());
        }
        snapshot.base_heights.insert(p, heights);
    }
    for (p, raw) in bytes.fields {
        let decoded = decode(&raw)?;
        for (index, value) in decoded.samples.into_iter().map(|(index, value)| (index, vec![value]))
            .chain(decoded.additional_samples) {
            let local = [
                i32::from(index % 32),
                i32::from(index / 32 % 32),
                i32::from(index / 1024),
            ];
            let cell = std::array::from_fn(|i| p[i] * 32 + local[i]);
            for value in value {
            if (Vec3::from_array(value.position) / volume.cell_size)
                .round()
                .as_ivec3()
                .to_array()
                != cell
            {
                return Err("三维作者样本与格坐标不一致".into());
            }
            if let std::collections::btree_map::Entry::Vacant(entry) = snapshot.samples.entry(cell) {
                entry.insert(value);
            } else {
                snapshot.additional_samples.entry(cell).or_default().push(value);
            }
            }
        }
    }
    Ok(snapshot)
}
/// 离线及测试入口；运行时显式拆成计算计划→IO字节→计算解码三个阶段。
pub fn load_surface_fields(
    root: &Path,
    volume: &PagedVolume,
    positions: &[[f32; 3]],
) -> Result<VolumeFieldSnapshot, String> {
    decode_surface_fields(
        volume,
        read_surface_fields(root, volume, plan_surface_fields(volume, positions, &[], 0))?,
    )
}

fn paint_weights(
    values: &mut Vec<(String, u8)>,
    id: &str,
    amount: f32,
    erase: bool,
    normalized: bool,
) {
    let mut weights: BTreeMap<String, f32> = values
        .iter()
        .map(|(s, w)| (s.clone(), f32::from(*w) / 255.0))
        .collect();
    let old = weights.get(id).copied().unwrap_or(0.0);
    let next = if erase {
        old * (1.0 - amount)
    } else {
        old + (1.0 - old) * amount
    };
    if normalized && !erase {
        for (key, w) in &mut weights {
            if key != id {
                *w *= 1.0 - amount;
            }
        }
    }
    weights.insert(id.to_owned(), next);
    let mut ordered: Vec<_> = weights
        .into_iter()
        .filter(|(_, w)| *w > 0.5 / 255.0)
        .collect();
    ordered.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ordered.truncate(4); // 单个样本四个贡献，目录与整层总数不受此限制。
    *values = ordered
        .into_iter()
        .filter_map(|(id, w)| {
            let w = (w * 255.0).round() as u8;
            (w > 0).then_some((id, w))
        })
        .collect();
}

/// 投射只采用命中面附近且朝向一致的点，笔刷不穿过薄洞顶涂到外部地表。
/// 调用方提供已冻结BVH；本函数的完整循环属于计算worker，返回新根后才允许采用。
pub fn prepare_field_paint(
    window: VolumeFieldWindow,
    previous: &PagedVolume,
    brush: &VolumeFieldBrush,
    mut project: impl FnMut(Vec3, Vec3, f32) -> Option<(Vec3, Vec3)>,
) -> Result<VolumeFieldDelta, String> {
    previous.validate()?;
    let mut blocks = window
        .blocks
        .into_iter()
        .map(|(key, raw)| {
            Ok((
                key,
                if raw.is_empty() {
                    Block::default()
                } else {
                    decode(&raw)?
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let from = Vec3::from_array(brush.from);
    let to = Vec3::from_array(brush.to);
    let normal = Vec3::from_array(brush.normal)
        .try_normalize()
        .ok_or("三维笔刷法线无效")?;
    if !from.is_finite()
        || !to.is_finite()
        || !brush.radius.is_finite()
        || !(0.1..=8.0).contains(&brush.radius)
        || !brush.strength.is_finite()
        || !(0.0..=1.0).contains(&brush.strength)
        || !brush.flow_exposure.is_finite()
        || brush.flow_exposure < 0.0
        || brush.id.is_empty()
        || brush.id.len() > 256
    {
        return Err("三维表面笔刷参数无效".into());
    }
    let step = previous.cell_size;
    let (min, max) = brush_bounds(previous, brush)?;
    let delta = to - from;
    let world_delta = brush.source_to_world * delta;
    let length = world_delta.length_squared();
    let mut changed = BTreeSet::new();
    let mut candidates = BTreeMap::new();
    for z in min.z..=max.z {
        for y in min.y..=max.y {
            for x in min.x..=max.x {
                let position = Vec3::new(x as f32, y as f32, z as f32) * step;
                let t = if length > 1e-12 {
                    ((brush.source_to_world * (position - from)).dot(world_delta) / length).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let center = from + delta * t;
                if brush.world_distance(position - center) > brush.radius + brush.world_distance(Vec3::splat(step)) {
                    continue;
                }
                // 先在点击层的切平面做极短投射。同格内的近邻楼板不能抢占下层候选；
                // 弯曲面离开切平面后仍由原完整邻域投射与后续连通检查覆盖。
                let plane = position + normal * (center - position).dot(normal);
                let epsilon = (step * 0.0001).max(0.00001);
                let Some((hit, n)) = project(plane + normal * epsilon, -normal, epsilon * 2.0)
                    .or_else(|| project(position + normal * step * 1.5, -normal, step * 3.0))
                else {
                    continue;
                };
                if brush.world_normal(n).dot(brush.world_normal(normal)) < 0.25 || brush.world_distance(hit - center) > brush.radius {
                    continue;
                }
                let cell = (hit / step).round().as_ivec3().to_array();
                let previous = candidates.entry(cell).or_insert((hit, n, center));
                if brush.world_distance(hit - center) < brush.world_distance(previous.0 - previous.2) {
                    *previous = (hit, n, center);
                }
            }
        }
    }
    // 小于作者格的笔刷仍应命中光标所在面；使用短射线避免从上方另一薄层起投。
    if let Some((hit, n)) = project(to + normal * step * 0.1, -normal, step * 0.2)
        && brush.world_normal(n).dot(brush.world_normal(normal)) >= 0.25
        && brush.world_distance(hit - to) <= brush.radius
    {
        candidates.insert((hit / step).round().as_ivec3().to_array(), (hit, n, to));
    }
    // 先找完整球形笔刷中的表面，再沿相邻切向面连通。不能用点击点的一层薄平面
    // 裁掉弯曲洞底，也不能因此跨过空隙刷到同朝向的另一层楼板。
    let seed = candidates
        .iter()
        .min_by(|a, b| {
            a.1.0
                .distance_squared(to)
                .total_cmp(&b.1.0.distance_squared(to))
        })
        .map(|(&cell, _)| cell);
    let mut connected = BTreeSet::new();
    let mut pending = Vec::new();
    if let Some(seed) = seed {
        connected.insert(seed);
        pending.push(seed);
    }
    while let Some(cell) = pending.pop() {
        let (p, n, _) = candidates[&cell];
        // n·点击法线至少0.25，陡坡相邻横格最多跳过四个纵格；只扩主轴邻域。
        let a = normal.abs();
        let axis = if a.x > a.y && a.x > a.z {
            0
        } else if a.y >= a.z {
            1
        } else {
            2
        };
        let reach = [0, 1, 2].map(|i| if i == axis { 4 } else { 1 });
        for z in -reach[2]..=reach[2] {
            for y in -reach[1]..=reach[1] {
                for x in -reach[0]..=reach[0] {
                    let neighbor = [cell[0] + x, cell[1] + y, cell[2] + z];
                    if connected.contains(&neighbor) {
                        continue;
                    }
                    let Some(&(q, m, _)) = candidates.get(&neighbor) else {
                        continue;
                    };
                    let delta = q - p;
                    // 同向平行楼板也可能只隔极小距离；固定半格容差会把它们误连。
                    // 连通只允许法线变化解释得了的弯曲误差，平面法线相同时仅容忍数值误差。
                    let bend = delta.length() * (n - m).length() * 0.5;
                    if n.dot(m) > 0.25
                        && delta.length_squared() <= step * step * 20.25
                        && delta.dot((n + m).normalize()).abs() <= bend + (step * 0.0001).max(0.00001)
                    {
                        connected.insert(neighbor);
                        pending.push(neighbor);
                    }
                }
            }
        }
    }
    for cell in connected {
        let (hit, n, center) = candidates[&cell];
        let (block, index) = address(cell);
        let Some(data) = blocks.get_mut(&block) else {
            return Err("三维表面投射越出冻结邻域".into());
        };
        let matches_surface = |sample: &VolumeFieldSample| {
            let known_normal = Vec3::from_array(sample.normal);
            let delta = hit - Vec3::from_array(sample.position);
            known_normal.dot(n) > 0.25
                && delta.dot((known_normal+n).normalize()).abs() <= (step*0.0001).max(0.00001)
        };
        let new_sample = || VolumeFieldSample {
                position: hit.to_array(),
                normal: n.to_array(),
                surface: Vec::new(),
                vegetation: Vec::new(),
                puddles: 0,
            };
        let primary = data.samples.get(&index).is_none_or(matches_surface);
        let sample = if primary {
            data.samples.entry(index).or_insert_with(new_sample)
        } else {
            let surfaces = data.additional_samples.entry(index).or_default();
            let slot = surfaces.iter().position(matches_surface).unwrap_or_else(|| {
                surfaces.push(new_sample()); surfaces.len()-1
            });
            &mut surfaces[slot]
        };
        let before = (!sample.surface.is_empty() || !sample.vegetation.is_empty() || sample.puddles > 0).then(|| sample.clone());
        sample.position = hit.to_array();
        sample.normal = n.to_array();
        let amount = field_paint_response(
            brush.strength,
            brush.flow_exposure,
            1.0 - brush.world_distance(hit - center) / brush.radius,
        );
        match brush.channel {
            VolumeFieldChannel::Surface => {
                paint_weights(&mut sample.surface, &brush.id, amount, brush.erase, true)
            }
            VolumeFieldChannel::Vegetation => paint_weights(
                &mut sample.vegetation,
                &brush.id,
                amount,
                brush.erase,
                false,
            ),
            VolumeFieldChannel::Puddles => {
                let value = f32::from(sample.puddles) / 255.0;
                sample.puddles = ((if brush.erase {
                    value * (1.0 - amount)
                } else {
                    value + (1.0 - value) * amount
                })
                .clamp(0.0, 1.0)
                    * 255.0)
                    .round() as u8;
            }
        }
        if before.as_ref() != Some(sample)
            && (!sample.surface.is_empty()
                || !sample.vegetation.is_empty()
                || sample.puddles > 0
                || before.is_some())
        {
            changed.insert(block);
        }
    }
    let mut writes = Vec::new();
    for p in changed {
        let mut block = blocks.remove(&p).unwrap();
        block
            .samples
            .retain(|_, s| !s.surface.is_empty() || !s.vegetation.is_empty() || s.puddles > 0);
        block.additional_samples.retain(|_, samples| {
            samples.retain(|s| !s.surface.is_empty() || !s.vegetation.is_empty() || s.puddles > 0);
            !samples.is_empty()
        });
        let bytes = if block.samples.is_empty() && block.additional_samples.is_empty() {
            Vec::new()
        } else {
            encode(&block)?
        };
        writes.push(LeafWrite {
            key: key(p)?,
            bytes,
        });
    }
    if writes.iter().map(|w| w.bytes.len()).sum::<usize>() > MAX_IO_BYTES {
        return Err("三维表面写集超过32MiB预算".into());
    }
    Ok(VolumeFieldDelta {
        writes,
        focus: brush.to,
    })
}

pub fn write_field_paint(
    root: &Path,
    previous: &PagedVolume,
    delta: VolumeFieldDelta,
) -> Result<PagedVolume, String> {
    if delta.writes.is_empty() {
        return Ok(previous.clone());
    }
    let mut store = Store::new(root);
    let mut next = previous.clone();
    for write in delta.writes {
        next.fields = store.set(next.fields, &write)?;
    }
    store.flush(&[next.fields])?;
    if next.fields != previous.fields {
        next.focus = delta.focus;
    }
    Ok(next)
}
