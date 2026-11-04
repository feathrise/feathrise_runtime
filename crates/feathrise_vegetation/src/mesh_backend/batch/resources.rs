//! 批次 LOD 质量倍率与变化驱动选择状态。

use bevy::prelude::*;

/// 场景画质对通用 Mesh 批次 LOD 和最远距离的最终倍率。
#[derive(Clone, Copy, Debug, Resource, PartialEq)]
pub struct VegetationMeshBatchQuality {
    lod_multiplier: f32,
    view_distance_multiplier: f32,
}

impl Default for VegetationMeshBatchQuality {
    fn default() -> Self {
        Self {
            lod_multiplier: 1.0,
            view_distance_multiplier: 1.0,
        }
    }
}

impl VegetationMeshBatchQuality {
    /// 原子采用有限正倍率；非法输入完整保留上一份有效配置。
    pub fn set_multipliers(&mut self, lod_multiplier: f32, view_distance_multiplier: f32) {
        if !lod_multiplier.is_finite()
            || !view_distance_multiplier.is_finite()
            || lod_multiplier <= 0.0
            || view_distance_multiplier <= 0.0
        {
            return;
        }
        self.lod_multiplier = lod_multiplier;
        self.view_distance_multiplier = view_distance_multiplier;
    }

    pub const fn multipliers(self) -> (f32, f32) {
        (self.lod_multiplier, self.view_distance_multiplier)
    }
}

/// 只有观察位置真正移动或画质变化时才重建全部节点的逐实例 LOD 前缀。
#[derive(Default, Resource)]
pub(super) struct VegetationMeshBatchSelectionState {
    pub(super) view_position: Option<Vec3>,
    pub(super) revision: u64,
    pub(super) entities: Vec<Option<Entity>>, // 正式节点增删边沿维护索引，常态每帧只读32项。
    pub(super) lookup: bevy::ecs::entity::EntityHashMap<usize>,
    pub(super) free: Vec<usize>,
    pub(super) cursor: usize,
    pub(super) stamps: bevy::ecs::entity::EntityHashMap<super::selection::Stamp>,
}
