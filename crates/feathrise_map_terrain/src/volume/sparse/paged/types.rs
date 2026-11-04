//! 持久根与一次有界写集；公开版本不持有密度、几何或整个块目录。
use serde::{Deserialize, Serialize};

pub(super) type Hash = [u8; 32];
pub(super) const EMPTY: Hash = [0; 32];
pub(super) const COLUMN_BYTES: usize = 33 * 33 * 4;
pub(super) const DENSITY_BYTES: usize = 32 * 32 * 32 * 2;
pub(super) const MAX_IO_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PagedVolume {
    pub cell_size: f32,
    pub focus: [f32; 3],
    pub(super) columns: Hash,
    pub(super) density: Hash,
    pub(super) meshes: Hash,
    /// 独立三维表面作者根；旧v3缺省为空，网格减面/重建不改写涂绘和种植。
    #[serde(default, skip_serializing_if = "is_empty")]
    pub(super) fields: Hash,
    /// 包围范围只用于作者索引，不能用来整体裁掉Clipmap。
    pub(super) bounds: Option<[[i32; 2]; 2]>,
    pub(super) mesh_recipe: u8,
}
impl PagedVolume {
    pub fn new(cell_size: f32) -> Result<Self, String> {
        let volume = Self {
            cell_size,
            focus: [0.; 3],
            columns: EMPTY,
            density: EMPTY,
            meshes: EMPTY,
            fields: EMPTY,
            bounds: None,
            mesh_recipe: 1,
        };
        volume.validate()?;
        Ok(volume)
    }
    pub fn extent(&self) -> f32 {
        self.cell_size * 32.
    }
    pub fn footprint(&self) -> Vec<[[f32; 2]; 2]> {
        self.bounds
            .map(|b| b.map(|p| p.map(|v| v as f32 * self.extent())))
            .into_iter()
            .collect()
    }
    pub(super) fn validate(&self) -> Result<(), String> {
        if !self.cell_size.is_finite()
            || !(0.01..=0.5).contains(&self.cell_size)
            || self.mesh_recipe != 1
            || self
                .focus
                .iter()
                .any(|v| !v.is_finite() || v.abs() > 100_000.)
            || self.bounds.is_some_and(|b| {
                (0..2).any(|i| b[0][i] >= b[1][i])
                    || b.iter().flatten().any(|v| v.abs_diff(0) > 1_000_000)
            })
            || (self.bounds.is_none() != (self.columns == EMPTY))
        {
            return Err("流送体积版本的格距、配方或作者范围无效".into());
        }
        Ok(())
    }
}

fn is_empty(hash: &Hash) -> bool {
    *hash == EMPTY
}

pub(super) struct LeafWrite {
    pub key: u64,
    pub bytes: Vec<u8>,
}
/// 已由计算worker编码的变更块；IO不再提取/简化网格。
pub struct VolumeDelta {
    pub(super) columns: Vec<LeafWrite>,
    pub(super) density: Vec<LeafWrite>,
    pub(super) meshes: Vec<LeafWrite>,
    pub(super) bounds: Option<[[i32; 2]; 2]>,
}

/// Morton格点键：每轴21位有符号坐标，不因地图尺寸增长增加索引深度。
pub(super) fn key(p: [i32; 3]) -> Result<u64, String> {
    if p.iter().any(|v| v.abs_diff(0) > 1_000_000) {
        return Err("流送体积块坐标越界".into());
    }
    let p = p.map(|v| (v + 1_048_576) as u64);
    let mut key = 0;
    for bit in 0..21 {
        for axis in 0..3 {
            key |= ((p[axis] >> bit) & 1) << (bit * 3 + axis);
        }
    }
    Ok(key)
}
