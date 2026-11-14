//! 静态模型共用正式GPU草和树灌木实例批次；模型局部样本随真实对象根变换。
mod build;
mod follow;
mod index;
mod resources;
mod runtime;
mod spawn;
#[cfg(test)]
mod tests;
pub use resources::DynamicModelVegetationStatus;
pub(in crate::scenes::dynamic_scene) use resources::ModelVegetationState;

use bevy::prelude::*;
use feathrise_scene_core::residency::{SceneInstanceContextAppExt, SceneResidencyAppExt};
pub(super) fn install(app: &mut App) {
    app.init_instance_resource::<ModelVegetationState>()
        .init_instance_resource::<DynamicModelVegetationStatus>()
        .init_resident_resource::<resources::ModelTaskBudget>()
        .init_resident_resource::<resources::ModelAssetEpoch>()
        .add_resident_systems(PreUpdate, runtime::observe);
    index::install(app);
    app.add_resident_systems(
        Update,
        runtime::drive
            .after(super::drive_dynamic_mesh_residency)
            .run_if(in_state(feathrise_scene_core::state::ScenePhase::Active)),
    )
    .add_resident_systems(
        PostUpdate,
        follow::follow
            .before(bevy::transform::TransformSystems::Propagate)
            .run_if(in_state(feathrise_scene_core::state::ScenePhase::Active)),
    )
    .add_resident_systems(
        OnExit(feathrise_scene_core::state::ScenePhase::Active),
        runtime::reset,
    );
}
pub(in crate::scenes::dynamic_scene) fn add_instance_systems(schedule: &mut Schedule) {
    schedule.add_systems(
        runtime::drive
            .after(super::drive_dynamic_mesh_residency)
            .after(crate::shared::placement_data::InstancePlacementAdvance),
    );
}
