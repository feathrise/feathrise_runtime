//! 普通静态网格的全局显示/碰撞LOD；作者源、专用后端和真实承托互不替代。
use bevy::{asset::AssetEvent, prelude::*};

mod capture;
mod collision;
mod components;
mod ownership;
mod resources;
mod systems;
mod types;

pub use components::{MeshLodAuthoringState, StaticMeshLodOriginal};
pub use resources::MeshLodRuntimeStatus;

pub(crate) fn install(app: &mut App) {
    app.init_resource::<resources::MeshLodRuntime>()
        .init_resource::<MeshLodRuntimeStatus>()
        .init_resource::<MeshLodAuthoringState>()
        .add_message::<AssetEvent<Mesh>>()
        .add_observer(systems::mesh_inserted)
        .add_observer(ownership::hierarchy_changed)
        .add_observer(ownership::owner_inserted::<crate::TerrainObjectInstance>)
        .add_observer(ownership::owner_inserted::<crate::MapPlacementInstance>)
        .add_observer(ownership::owner_inserted::<crate::SubMapActive>)
        .add_observer(ownership::world_ready)
        .add_systems(
            PostUpdate,
            (
                systems::asset_events,
                systems::register,
                systems::drive,
                systems::select,
            )
                .chain()
                .after(bevy::transform::TransformSystems::Propagate),
        );
    collision::install(app);
    app.add_systems(
        PostUpdate,
        systems::authoring_ready
            .after(systems::select)
            .after(collision::restore_unsafe),
    );
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod parity_tests;
