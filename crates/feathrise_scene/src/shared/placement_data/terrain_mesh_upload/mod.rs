//! 三维地形候选的分帧网格登记；所有块就绪以前阻止碰撞捕获和联合采用。
mod components;
mod resources;
mod systems;
#[cfg(test)]
mod tests;
use bevy::prelude::*;
pub use components::TerrainMeshUpload;

pub(super) fn install(app: &mut App) {
    app.init_resource::<resources::UploadGarbage>().add_systems(
        Update,
        systems::upload
            .after(crate::TerrainVolumeStreamSystems::Drive)
            .before(super::terrain_collision::drive_terrain_collision),
    );
}
