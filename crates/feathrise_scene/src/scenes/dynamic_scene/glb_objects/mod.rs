//! 原始GLB保留完整层级、材质和网格；只在后台派生独立真实碰撞，不转换为高度场。
mod resources;
mod capture;
mod systems;
#[cfg(test)]
mod tests;
use bevy::prelude::*;
use feathrise_scene_core::residency::SceneInstanceContextAppExt;
pub(super) use resources::GlbObjectRuntime;
pub(super) fn install(app: &mut App) {
    app.init_instance_resource::<GlbObjectRuntime>().init_resource::<resources::CaptureBudget>();
}
pub(super) fn add_systems(schedule: &mut Schedule) {
    schedule.add_systems(systems::drive.after(super::systems::prepare_dynamic_scene_terrain));
}
