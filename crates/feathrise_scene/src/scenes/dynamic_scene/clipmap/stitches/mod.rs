//! 完整边的运行时发布；只响应实际LOD/姿态变化，双方在提取前一起采用边界约束。
use super::super::instances::{SubsceneInstance, TerrainInstanceLookup, TerrainObjectInstance};
use super::{
    ground::DynamicClipmapGroundSurface,
    material::{ClipmapHeightMaterial, SparseClipmapSurfaceMaterial},
    residency::SparseHeightPageRuntime,
};
use bevy::prelude::*;
use feathrise_environment::vegetation::blade::{GpuBladeClipmapHeight, GpuBladeClipmapStitches};
use feathrise_scene_core::residency::{
    InstanceResourceTable, SceneInstanceOwner, SceneSourceFrame, SceneResidencyAppExt,
};
use std::{collections::BTreeSet, sync::Arc};

mod components;
mod encoding;
mod resources;
mod systems;
#[cfg(test)]
mod tests;
pub(in crate::scenes::dynamic_scene) use components::{StitchLodStamp, StitchPublish, StitchStatus};
pub(in crate::scenes::dynamic_scene) use encoding::collision_constraints;
use encoding::{aligned, encode, paired};
use resources::Followers;
use systems::{publish, track, track_owner, untrack, untrack_owner};

pub(in crate::scenes::dynamic_scene) fn install(app: &mut App) {
    app.init_resource::<Followers>()
        .add_observer(track)
        .add_observer(track_owner)
        .add_observer(untrack)
        .add_observer(untrack_owner);
    app.add_resident_systems(
        PostUpdate,
        publish
            .in_set(StitchPublish)
            .before(bevy::transform::TransformSystems::Propagate),
    );
}
