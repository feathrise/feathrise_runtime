//! 完整场景场景驻留：实体、碰撞、相机、材质与运行时共同暂停/恢复，冷载仍走原流程。

mod assets;
mod components;
mod entities;
mod resources;
mod systems;
#[cfg(test)]
mod tests;

pub use resources::{ResidentEntityIndex, ResidentScenePreparation, ResidentScenes};
pub use assets::watch_scene_application_asset;
pub use systems::{
    arm_resident_scene_transition, close_resident_scene, mark_resident_document_open,
    prepare_resident_scene,
};

use bevy::prelude::*;
use feathrise_scene_core::residency::{
    SceneResidencyActivity, SceneResidencyAppExt, SceneResidencyResourcePlugin,
};
use feathrise_scene_core::state::ScenePhase;

pub(crate) fn install(app: &mut App) {
    if !app.is_plugin_added::<SceneResidencyResourcePlugin>() {
        app.add_plugins(SceneResidencyResourcePlugin);
    }
    app.init_resource::<ResidentScenes>()
        .init_resource::<resources::ResidentEntityIndex>()
        .register_optional_resident_resource::<feathrise_scene_core::state::SceneStartupGate>()
        .init_resident_resource::<ClearColor>()
        .add_observer(entities::register_scene_scoped_root)
        .add_observer(entities::register_owner)
        .add_observer(entities::unregister_owner)
        .add_observer(entities::inherit_parent_owner)
        .add_observer(entities::pause_added_body)
        .add_observer(entities::pause_added_collider)
        .add_systems(PreUpdate, entities::enroll_existing_descendants)
        .add_systems(Update, entities::resume_adopted_audio)
        .add_systems(
            Last,
            (
                systems::advance_scene_residency,
                systems::retire_closed_scenes,
            )
                .chain(),
        )
        .add_systems(
            StateTransition,
            systems::intercept_scene_transition
                .before(bevy::state::state::StateTransitionSystems::DependentTransitions),
        )
        .add_systems(
            StateTransition,
            systems::finish_scene_transition
                .after(bevy::state::state::StateTransitionSystems::EnterSchedules),
        );
    app.add_systems(OnEnter(ScenePhase::Loading), systems::begin_fresh_world);
    app.world_mut().init_resource::<SceneResidencyActivity>();
    assets::install(app);
    app.configure_sets(
        PostUpdate,
        bevy::app::AnimationSystems.run_if(feathrise_scene_core::residency::resident_runtime_active),
    );
    app.configure_sets(
        FixedPostUpdate,
        avian3d::prelude::PhysicsSystems::StepSimulation
            .run_if(feathrise_scene_core::residency::resident_runtime_active),
    ); // 切换场景准备时连物理步也不能采用半份碰撞。
    app.configure_sets(
        PostUpdate,
        avian3d::prelude::PhysicsSystems::StepSimulation
            .run_if(feathrise_scene_core::residency::resident_runtime_active),
    );
    for &scene in ScenePhase::active_phases() {
        app.add_systems(OnEnter(scene), systems::record_active_scene);
    }
}

/// 作者派生节点不一定带DespawnOnExit；只登记明确的场景预览根，不捕获独立资产/角色工作区。
pub fn register_resident_scene_root<T: Component>(app: &mut App) {
    app.add_observer(entities::register_additional_root::<T>);
}
