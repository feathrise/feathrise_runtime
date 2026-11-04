//! 地图体积驻留窗口：与F12共用联合采用门，远处数据保存在不可变块库。
mod chunks;
mod components;
mod display;
mod jobs;
mod resources;
mod systems;
#[cfg(test)]
mod tests;
mod types;
use bevy::prelude::*;
use feathrise_scene_core::residency::MapInstanceContextAppExt;
pub use components::{StreamedVolumeResident, volume_lod_signature};
pub use display::{VolumeDisplayBuild, VolumeSurfaceFields, build_volume_display};
pub(in crate::maps::dynamic_map) use chunks::build_meshes as build_surface_chunks;
pub use resources::{TerrainVolumeStreamControl, TerrainVolumeStreamStatus};
pub use types::TerrainVolumeStreamSystems;
pub(super) fn install(app: &mut App) {
    app.init_instance_resource::<resources::Runtime>()
        .init_resource::<display::FieldGarbage>()
        .init_instance_resource::<TerrainVolumeStreamControl>()
        .init_instance_resource::<TerrainVolumeStreamStatus>()
        .add_systems(Update, display::release_fields)
        .add_systems(
            Update,
            systems::drive
                .in_set(TerrainVolumeStreamSystems::Drive)
                .before(super::terrain_topology::TerrainTopologySystems::Collect)
                .run_if(in_state(feathrise_scene_core::state::ScenePhase::Active)),
        );
}

pub(in crate::maps::dynamic_map) fn add_instance_systems(schedule: &mut Schedule) {
    schedule.add_systems(systems::drive
        .after(crate::shared::placement_data::InstancePlacementAdvance)
        .before(super::terrain_topology::collect_terrain_topology));
}

pub(in crate::maps::dynamic_map) fn busy(world: &World, entity: Entity) -> bool {
    let state = &world.resource::<feathrise_scene_core::residency::InstanceResourceTable<resources::Runtime>>().values[&entity];
    state.work.is_some() || state.release.is_some()
}

pub(in crate::maps::dynamic_map) fn adopt_root_runtime(world: &mut World, entity: Entity) {
    let previous = std::mem::take(&mut *world.resource_mut::<resources::Runtime>());
    world.resource_mut::<feathrise_scene_core::residency::InstanceResourceTable<resources::Runtime>>().values.insert(entity, previous);
}
pub(in crate::maps::dynamic_map) fn finished(world: &World, entity: Entity) -> bool {
    let state = &world.resource::<feathrise_scene_core::residency::InstanceResourceTable<resources::Runtime>>().values[&entity];
    state.work.as_ref().is_none_or(|work| work.task.is_finished())
        && state.release.as_ref().is_none_or(bevy::tasks::Task::is_finished)
}
