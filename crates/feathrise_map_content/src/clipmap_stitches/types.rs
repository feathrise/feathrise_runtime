//! 拼接的作者记录和运行时边界曲线；固定派生采样预算与地图逻辑网格数无关。
use crate::TerrainId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 一条派生边最多256段；长边增加派生边界格距，源高度与内部LOD保持原精度。
pub const CLIPMAP_STITCH_SEGMENTS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipmapEdge {
    West,
    East,
    South,
    North,
}
impl ClipmapEdge {
    pub const ALL: [Self; 4] = [Self::West, Self::East, Self::South, Self::North];
    pub fn index(self) -> usize {
        match self {
            Self::West => 0,
            Self::East => 1,
            Self::South => 2,
            Self::North => 3,
        }
    }
    pub fn axis(self) -> usize {
        if matches!(self, Self::West | Self::East) {
            1
        } else {
            0
        }
    }
}

/// 两个对象各存一份镜像记录；同一ID和互指边证明关系，不能根据空间重叠暗中合并。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipmapStitch {
    pub id: String,
    pub edge: ClipmapEdge,
    pub peer: TerrainId,
    pub peer_edge: ClipmapEdge,
    pub source_path: String,
    pub peer_source_path: String,
    /// 已采样的本地Y曲线；对侧曲线经完整Y平移/缩放转换，双方表达同一父世界曲线。
    pub profile: ClipmapEdgeProfile,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipmapEdgeProfile {
    pub edge: ClipmapEdge,
    pub step: f32,
    pub blend_width: f32,
    #[serde(default = "one")]
    pub collision_stride: u32,
    pub samples: Arc<[f32]>,
}
fn one() -> u32 {
    1
}
impl ClipmapEdgeProfile {
    pub fn sample(&self, distance: f32, stride: u32) -> f32 {
        let stride = stride.max(1) as usize;
        let last = self.samples.len().saturating_sub(1);
        let at = (distance.max(0.0) / self.step).min(last as f32);
        let lo = ((at as usize) / stride * stride).min(last);
        let hi = (lo + stride).min(last);
        let t = if hi == lo {
            0.0
        } else {
            (at - lo as f32) / (hi - lo) as f32
        };
        self.samples[lo] * (1.0 - t) + self.samples[hi] * t
    }
    pub fn validate(&self) -> Result<(), String> {
        if !self.step.is_finite()
            || self.step <= 0.0
            || !self.blend_width.is_finite()
            || self.blend_width <= 0.0
            || !(2..=CLIPMAP_STITCH_SEGMENTS + 1).contains(&self.samples.len())
            || !(self.samples.len() - 1).is_power_of_two()
            || !self.collision_stride.is_power_of_two()
            || self.collision_stride as usize > self.samples.len() - 1
            || self.samples.iter().any(|v| !v.is_finite())
        {
            return Err("Clipmap拼接曲线的格距、过渡宽度或高度无效".into());
        }
        Ok(())
    }
}
