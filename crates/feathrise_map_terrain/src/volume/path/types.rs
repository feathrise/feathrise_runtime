//! 多点路径的冻结参数与弧长游标；只有内部窗口持有逐刷印数据。
use crate::volume::{VolumeBrush, VolumeBrushKind};
use bevy::math::{Mat4, Rect, Vec3};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VolumePathSection {
    #[default]
    Rectangle,
    Round,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VolumePathParameters {
    pub curved: bool,
    pub section: VolumePathSection,
    pub width: f32,
    pub height: f32,
}
impl Default for VolumePathParameters {
    fn default() -> Self {
        Self {
            curved: false,
            section: VolumePathSection::Rectangle,
            width: 2.,
            height: 2.,
        }
    }
}

/// 所有节点代表底面中心；曲线展开只在后台执行，后续批次共享同一几何版本。
#[derive(Clone, Debug)]
pub struct VolumePathPlan {
    pub(super) points: Arc<[Vec3]>,
    pub(super) lengths: Arc<[f64]>,
    pub(super) node_lengths: Arc<[f64]>,
    pub(super) parameters: VolumePathParameters,
    pub(super) world_step: f32,
    pub(super) source_to_world: Mat4,
    pub(super) source_bounds: Rect,
}

#[derive(Clone, Debug)]
pub struct VolumePathBatches {
    pub(super) plan: Arc<VolumePathPlan>,
    pub(super) kind: VolumeBrushKind,
    pub(super) strength: f32,
    pub(super) offset: Vec3,
    pub(super) cell_size: f32,
    pub(super) world_step: f32,
    pub(super) steps: u64,
    pub(super) next: u64,
}

impl VolumePathPlan {
    pub fn parameters(&self) -> VolumePathParameters {
        self.parameters
    }
    pub fn length(&self) -> f64 {
        *self.lengths.last().unwrap()
    }
}

impl VolumePathBatches {
    pub fn finished(&self) -> bool {
        self.next > self.steps
    }
    pub fn completed_stamps(&self) -> u64 {
        self.next
    }
    pub fn total_stamps(&self) -> u64 {
        self.steps + 1
    }
    pub(super) fn stamp(&self, index: u64) -> VolumeBrush {
        self.plan
            .brush_at(
                self.plan.length() * (index as f64 / self.steps as f64),
                self.kind,
                self.strength,
                self.world_step,
            )
            .in_source_frame(self.plan.source_to_world)
            .translated(self.offset)
    }
}
