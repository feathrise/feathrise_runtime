//! Clipmap 的世界固定光追高度代理；只发布完整同代源，Solari 生命周期由光追库拥有。

use feathrise_scene_core::residency::SceneResidencyAppExt;

mod components;
mod constants;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod material_tests;
mod materials;
mod mesh;
mod resources;
mod systems;
#[cfg(test)]
mod tests;
mod types;

pub use components::ClipmapRaytracingAnchor;
pub(crate) use resources::ClipmapRaytracingRuntime;
pub use resources::{ClipmapRaytracingPhase, ClipmapRaytracingRequest, ClipmapRaytracingStatus};

use super::ClipmapGroundPublishSet;
use bevy::prelude::*;

/// 光追库在正式高度、材质和代理发布后观察目标，避免读取上一帧的失效锚点。
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClipmapRaytracingPublishSet;

pub use resources::ClipmapRaytracingInstanceRoute;

pub(crate) struct ClipmapRaytracingPlugin;

impl Plugin for ClipmapRaytracingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resident_resource::<ClipmapRaytracingRequest>()
            .init_resident_resource::<ClipmapRaytracingStatus>()
            .init_resident_resource::<ClipmapRaytracingRuntime>()
            .init_resident_resource::<ClipmapRaytracingInstanceRoute>()
            .add_resident_systems(
                Update,
                systems::drive_proxy
                    .in_set(ClipmapRaytracingPublishSet)
                    .after(ClipmapGroundPublishSet),
            )
            .add_resident_systems(PostUpdate, materials::sync_visible_material);
    }
}
