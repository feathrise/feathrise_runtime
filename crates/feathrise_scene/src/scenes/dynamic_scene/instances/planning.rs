//! 引用树比较只在单飞后台执行；只改整体位置时保留所有域、实体与NPC的身份。

use bevy::prelude::*;
use feathrise_scene_content::ExpandedSceneInstance;
use std::{collections::VecDeque, sync::Arc};

pub(super) struct Comparison {
    pub previous_root: Entity,
    pub revision: u64,
    pub structural_revision: u64,
    pub graph: Arc<[ExpandedSceneInstance]>,
    pub structural: bool,
    pub poses: VecDeque<usize>,
}

impl Comparison {
    pub(super) fn compatible(&self, root: Entity, revision: u64, structural_revision: u64) -> bool {
        self.previous_root == root && self.structural_revision == structural_revision
            && (!self.structural || self.revision == revision)
    } // 同结构允许上一完整姿态追随鼠标；来源或拓扑更改仍拒绝旧结果。
}

pub(super) fn compare(previous_root: Entity, revision: u64,
    before: Arc<[ExpandedSceneInstance]>, graph: Arc<[ExpandedSceneInstance]>) -> Comparison {
    let structural = before.len() != graph.len() || before.iter().zip(graph.iter()).enumerate().any(|(index, (old, new))|
        old.path != new.path || old.source_scene_id != new.source_scene_id || index > 0 && (
            if let (Some(before), Some(after)) = (&old.terrain, &new.terrain) {
                before.terrain != after.terrain || before.voxel != after.voxel || before.clipmap_stitches != after.clipmap_stitches
                    || before.world_min != after.world_min || before.world_max != after.world_max
                    || before.transform.scale != after.transform.scale
                    || before.surface_path != after.surface_path || before.vegetation_path != after.vegetation_path
                    || before.water_path != after.water_path || before.puddles != after.puddles
            } else { old.terrain.is_some() != new.terrain.is_some() || old.source_revision != new.source_revision }));
    let poses = if structural { VecDeque::new() } else {
        before.iter().zip(graph.iter()).enumerate().skip(1).filter_map(|(index, (old, new))|
            (old.local_transform != new.local_transform || old.transform != new.transform).then_some(index)).collect()
    };
    Comparison { previous_root, revision, structural_revision: 0, graph, structural, poses }
}
