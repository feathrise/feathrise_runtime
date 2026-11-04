//! 稀疏块按Arc共享；每份真实分配持有额度，历史重复引用不会重复计费。
use super::super::types::*;
use crate::TerrainMeshInput;
use std::{
    collections::BTreeMap,
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

pub const SPARSE_CHUNK_CELLS: i32 = 32;
pub const SPARSE_CELL_SIZE: f32 = 0.25;
pub const MAX_AUTHOR_VOLUME_GLB_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_BLOCKS: usize = 512;
pub(super) const MAX_COLUMNS: usize = 256;
// 一层聚合为一个真实碰撞根，先遵守碰撞捕获上限，派生光追几何也能单帧完成有界上传。
pub(super) const MAX_SPARSE_VERTICES: usize = 262_144;
pub(super) const MAX_SPARSE_TRIANGLES: usize = 262_144;
pub(super) const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;

/// 源/网格同代校验需要知道提取配方；缺省0保持已经保存的v2逐三角重建结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MeshRecipe {
    Legacy,
    CompactV1,
}

#[derive(Debug)]
pub struct VolumeMemoryBudget {
    used: AtomicUsize,
    legacy: AtomicUsize,
    limit: usize,
}
impl Default for VolumeMemoryBudget {
    fn default() -> Self {
        Self {
            used: AtomicUsize::new(0),
            legacy: AtomicUsize::new(0),
            limit: 64 * 1024 * 1024,
        }
    }
}
impl VolumeMemoryBudget {
    #[cfg(test)]
    pub(super) fn limited(limit: usize) -> Self {
        Self {
            used: AtomicUsize::new(0),
            legacy: AtomicUsize::new(0),
            limit,
        }
    }
    pub fn used_bytes(&self) -> usize {
        self.used.load(Ordering::Acquire)
    }
    /// 开启单飞任务前预留唯一旧v1数组，迁移中的旧历史与新块共用同一总额度。
    pub fn reserve_legacy_bytes(&self, bytes: usize) -> Result<(), String> {
        if self
            .used_bytes()
            .checked_add(bytes)
            .is_none_or(|n| n > self.limit)
        {
            return Err("旧三维历史与分块缓存合计超过64MiB预算".into());
        }
        self.legacy.store(bytes, Ordering::Release);
        Ok(())
    }
    pub(super) fn retain<T>(
        self: &Arc<Self>,
        data: T,
        bytes: usize,
    ) -> Result<Arc<VolumeBuffer<T>>, String> {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|n| {
                    *n <= self
                        .limit
                        .saturating_sub(self.legacy.load(Ordering::Acquire))
                })
            })
            .map_err(|_| {
                "三维作者源、网格缓存和撤销历史达到64MiB预算，请保存后重新打开会话".to_string()
            })?;
        Ok(Arc::new(VolumeBuffer {
            data,
            budget: self.clone(),
            bytes,
        }))
    }
}
#[derive(Debug)]
pub(super) struct VolumeBuffer<T> {
    pub data: T,
    budget: Arc<VolumeMemoryBudget>,
    bytes: usize,
}
impl<T> Deref for VolumeBuffer<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.data
    }
}
impl<T> Drop for VolumeBuffer<T> {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[derive(Clone, Debug)]
pub struct SparseVolume {
    pub(super) cell_size: f32,
    pub(super) mesh_recipe: MeshRecipe,
    pub(super) columns: BTreeMap<[i32; 2], Arc<VolumeBuffer<Vec<f32>>>>,
    pub(super) blocks: BTreeMap<[i32; 3], Arc<VolumeBuffer<Vec<i16>>>>,
    pub(super) meshes: BTreeMap<[i32; 3], Arc<VolumeBuffer<TerrainMeshInput>>>,
    pub(super) budget: Arc<VolumeMemoryBudget>,
    pub(super) footprint: Arc<[[[f32; 2]; 2]]>,
    pub last_rebuilt_blocks: usize,
    pub(super) last_rebuilt_keys: Arc<[[i32; 3]]>,
}
impl SparseVolume {
    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
    pub fn column_count(&self) -> usize {
        self.columns.len()
    }
    pub fn footprint(&self) -> &[[[f32; 2]; 2]] {
        &self.footprint
    }
    pub fn memory_bytes(&self) -> usize {
        self.blocks.len() * 32usize.pow(3) * 2 + self.columns.len() * 33usize.pow(2) * 4
    }
    pub fn extent(&self) -> f32 {
        SPARSE_CHUNK_CELLS as f32 * self.cell_size
    }
    pub fn value_at(&self, point: [i32; 3]) -> Result<f32, String> {
        if point.iter().any(|v| v.abs_diff(0) > 10_000_000) {
            return Err("三维查询格点超出有效整数范围".into());
        }
        Ok(self.sample(point)? as f32 * self.cell_size / VOLUME_QUANTIZATION)
    }
    pub(super) fn sample(&self, point: [i32; 3]) -> Result<i16, String> {
        let key = point.map(|v| v.div_euclid(SPARSE_CHUNK_CELLS));
        if let Some(block) = self.blocks.get(&key) {
            let p = point.map(|v| v.rem_euclid(SPARSE_CHUNK_CELLS) as usize);
            return Ok(block[(p[2] * 32 + p[1]) * 32 + p[0]]);
        }
        Ok(quantize(
            point[1] as f32 * self.cell_size - self.height([point[0], point[2]])?,
            self.cell_size,
        ))
    }
    pub(super) fn height(&self, point: [i32; 2]) -> Result<f32, String> {
        // 正边界优先唯一拥有者；没有相邻列时读已有列的共享边，绝不以零高度填缺页。
        let k = point.map(|v| v.div_euclid(32));
        for dz in [0, -1] {
            for dx in [0, -1] {
                let key = [k[0] + dx, k[1] + dz];
                let p = [point[0] - key[0] * 32, point[1] - key[1] * 32];
                if p.iter().all(|v| (0..=32).contains(v))
                    && let Some(h) = self.columns.get(&key)
                {
                    return Ok(h[(p[1] * 33 + p[0]) as usize]);
                }
            }
        }
        Err("三维区域缺少同代基础高度，等待作者页后重试".into())
    }
    pub(super) fn validate(&self) -> Result<(), String> {
        if !self.cell_size.is_finite()
            || !(0.01..=0.5).contains(&self.cell_size)
            || self.columns.len() > MAX_COLUMNS
            || self.blocks.len() > MAX_BLOCKS
            || self.memory_bytes() > MAX_SOURCE_BYTES
            || self.columns.iter().any(|(k, h)| {
                k.iter().any(|v| v.abs_diff(0) > 1_000_000)
                    || h.len() != 33usize.pow(2)
                    || h.iter().any(|v| !v.is_finite() || v.abs() > 100_000.)
            })
            || self.blocks.iter().any(|(k, b)| {
                k.iter().any(|v| v.abs_diff(0) > 1_000_000)
                    || b.len() != 32usize.pow(3)
                    || !self.columns.contains_key(&[k[0], k[2]])
            })
        {
            return Err("分块三维作者源格式或32MiB容量无效".into());
        }
        // 邻列共享边必须来自同一冻结高度；否则补列后会改变旧网格却未进入脏块集合。
        for (key, h) in &self.columns {
            if let Some(right) = self.columns.get(&[key[0] + 1, key[1]]) {
                if (0..=32).any(|z| h[z * 33 + 32] != right[z * 33]) {
                    return Err("分块作者高度共享边不一致".into());
                }
            }
            if let Some(back) = self.columns.get(&[key[0], key[1] + 1]) {
                if (0..=32).any(|x| h[32 * 33 + x] != back[x]) {
                    return Err("分块作者高度共享边不一致".into());
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum VolumeSource {
    Dense(VolumeData),
    Sparse(SparseVolume),
    Paged(super::paged::PagedVolume),
}
impl From<VolumeData> for VolumeSource {
    fn from(value: VolumeData) -> Self {
        Self::Dense(value)
    }
}
impl VolumeSource {
    pub fn memory_bytes(&self) -> usize {
        match self {
            Self::Dense(v) => v.memory_bytes(),
            Self::Sparse(v) => v.memory_bytes(),
            Self::Paged(_) => std::mem::size_of::<super::paged::PagedVolume>(),
        }
    }
    pub fn footprint(&self) -> Vec<[[f32; 2]; 2]> {
        match self {
            Self::Dense(v) => vec![[[0.; 2], [v.extent(); 2]]],
            Self::Sparse(v) => v.footprint.to_vec(),
            Self::Paged(v) => v.footprint(),
        }
    }
    pub fn is_sparse(&self) -> bool {
        matches!(self, Self::Sparse(_) | Self::Paged(_))
    }
}
impl PartialEq for VolumeSource {
    fn eq(&self, rhs: &Self) -> bool {
        match (self, rhs) {
            (Self::Dense(a), Self::Dense(b)) => a == b,
            (Self::Paged(a), Self::Paged(b)) => a == b,
            (Self::Sparse(a), Self::Sparse(b)) => {
                a.cell_size == b.cell_size
                    && a.mesh_recipe == b.mesh_recipe
                    && a.columns.len() == b.columns.len()
                    && a.blocks.len() == b.blocks.len()
                    && a.columns
                        .iter()
                        .all(|(k, v)| b.columns.get(k).is_some_and(|w| v.data == w.data))
                    && a.blocks
                        .iter()
                        .all(|(k, v)| b.blocks.get(k).is_some_and(|w| v.data == w.data))
            }
            _ => false,
        }
    }
}
