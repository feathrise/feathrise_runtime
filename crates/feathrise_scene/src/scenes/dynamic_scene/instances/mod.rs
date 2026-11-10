//! 完整引用场景的实例宿主；父图只拥有根变换，所有来源内容复用正式场景域。

mod components;
mod resources;
#[cfg(feature = "runtime_test_support")]
pub mod runtime_test_support; // 下游测试通过真实活动身份与框架验证编辑行为，不公开生产Runtime写入口。
mod schedules;
mod planning; // 完整树比较在后台，位置变化保留已有内容身份。
mod readiness; // 六域完成证据和真实后台并发归还。
mod driver; // 有界准备、隐藏候选、整体采用与增量退役。
mod physics; // 静态碰撞由完整批次门统一开关。
mod material_frames; // 作者采样保持源坐标，GPU参数承接整体位置。
mod characters; // 活动引用向唯一角色协调器发布正式源。
mod terrain_sources; // 独立地形只投影自己的作者句柄，场景容器仍唯一持有角色和传送。
#[cfg(test)]
mod tests;
#[cfg(test)]
mod physics_resume_tests; // 双时钟停止期间采用静态地形，再恢复真实物理联系与求解。
#[cfg(test)]
mod mesh_profile_tests; // 独立网格必须消费正式树木/灌木/岩石请求，且共享原型可安全退役。

pub use components::{SubsceneActive, SubsceneInstance};
pub use resources::SubsceneRuntimeStatus;
pub(super) use resources::{SubsceneRuntime, TerrainInstanceLookup};
/// 回归通过正式作者归属交接取得应用身份，避免测试私设状态而绕过生产门。
#[cfg(test)]
pub(super) fn test_apply_authoring_target(world: &mut World) {
    authoring_target::sync_preview_owner(world);
}
#[cfg(test)]
pub(super) use terrain_sources::{register_lookup, remove_lookup, remove_empty_lookup, seal_lookup};
pub use terrain_sources::{TerrainObjectInstance, TerrainObjectTransformPreview};
mod authoring_target;
mod vegetation_drafts;
pub use vegetation_drafts::TerrainVegetationDrafts;
mod water_drafts;
pub use water_drafts::TerrainWaterDrafts;
mod editor_visibility;
pub use editor_visibility::TerrainObjectEditorVisibility;
pub use authoring_target::TerrainObjectAuthoringTarget;
pub use authoring_target::TerrainObjectStitchStatus;
pub use authoring_target::TerrainObjectResource;
pub use authoring_target::TerrainObjectClipmapGround;
pub use authoring_target::TerrainAuthoringApply;
pub use authoring_target::TerrainVolumeAuthorPreviewRoot;

use bevy::prelude::*;
use feathrise_scene_content::*;
use feathrise_scene_core::residency::{SceneInstanceContextAppExt, SceneResidencyAppExt, install_instance_asset_journal};

pub(super) fn install(app: &mut App) {
    super::clipmap::stitches::install(app);
    app.init_resident_resource::<resources::SubsceneRuntime>()
        .init_resource::<TerrainInstanceLookup>() // 与driver的Runtime resource_scope独立，候选子调度持续可读。
        .init_resource::<TerrainObjectTransformPreview>()
        .init_resource::<TerrainObjectAuthoringTarget>()
        .init_resource::<TerrainObjectEditorVisibility>()
        .init_resource::<authoring_target::EmptyTerrainGround>()
        .init_resident_resource::<SubsceneRuntimeStatus>()
        .register_instance_resource::<ActiveDynamicScene>()
        .register_instance_resource::<State<feathrise_scene_core::state::ScenePhase>>()
        .register_instance_resource::<SceneAuthorPreview>()
        .register_instance_resource::<SceneInstanceAuthorPreview>()
        .register_instance_resource::<SparseHeightLiveOverride>()
        .register_instance_resource::<ScenePuddleLiveOverride>()
        .register_instance_resource::<ScenePuddleSaveHandoff>()
        .register_instance_resource::<crate::shared::surface_weather::PuddleRuntimeState>()
        .register_instance_resource::<feathrise_environment::terrain_surface::SurfacePuddleMask>()
        .register_instance_resource::<feathrise_environment::terrain_surface::TerrainSurfaceRenderStats>()
        .add_observer(physics::consolidate_static_bodies)
        .add_observer(physics::repair_collider_body)
        .add_observer(physics::consolidate_late_owned_bodies)
        .add_resident_systems(PreUpdate, driver::publish_root)
        .add_resident_systems(PreUpdate, driver::invalidate_submaps_for_placement_changes)
        .add_resident_systems(Update, driver::drive.after(SceneInstanceGraphPublishSet)
            .before(crate::SceneApplicationSet::PublishSource)
            .after(crate::shared::world_asset_lifecycle::WorldAssetLifecycleSystems::InvalidateRoots)
            .after(crate::shared::large_scene::DynamicWaterMeshOverrideSystems::ProcessPending))
        .add_resident_systems(Update, driver::publish_startup.after(driver::drive)
            .before(crate::shared::scene_runtime::complete_scene_startup))
        .add_resident_systems(PostUpdate, super::vegetation::follow_subscene_blade_windows
            .before(bevy::transform::TransformSystems::Propagate))
        .add_resident_systems(PostUpdate, terrain_sources::apply_preview
            .before(super::vegetation::follow_subscene_blade_windows)
            .before(bevy::transform::TransformSystems::Propagate));
    app.add_resident_systems(Update, authoring_target::sync_preview_owner.in_set(TerrainAuthoringApply));
    app.add_resident_systems(Update, authoring_target::rebind_volume_roots.after(TerrainAuthoringApply)
        .after(driver::drive).before(bevy::transform::TransformSystems::Propagate));
    app.add_resident_systems(PostUpdate, editor_visibility::apply.after(terrain_sources::apply_preview)
        .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate));
    app.configure_sets(PostUpdate,super::clipmap::stitches::StitchPublish.after(terrain_sources::apply_preview));
    install_instance_asset_journal::<SceneManifestAsset>(app);
    install_instance_asset_journal::<SceneVoxelTerrainSource>(app);
    install_instance_asset_journal::<ScenePlacementsAsset>(app);
    install_instance_asset_journal::<ScenePortalsAsset>(app);
    install_instance_asset_journal::<SceneBoundariesAsset>(app);
    install_instance_asset_journal::<SceneSurfaceAsset>(app);
    install_instance_asset_journal::<SceneSurfaceSourceAsset>(app); // 实例轮转可能超过两帧，浅坑CPU作者源热更也须保留真实换代通知。
    install_instance_asset_journal::<SceneVegetationAsset>(app);
    install_instance_asset_journal::<SceneWaterAsset>(app);
    install_instance_asset_journal::<SparseHeightManifestAsset>(app);
    install_instance_asset_journal::<SparseHeightTileAsset>(app);
    install_instance_asset_journal::<SceneWaterMaskAsset>(app);
    install_instance_asset_journal::<Image>(app);
    install_instance_asset_journal::<WorldAsset>(app);
    install_instance_asset_journal::<Mesh>(app);
}
