//! 场景与渲染的公共装配；相机控制、角色、战斗和具体内容由应用另行安装。

mod billboard_camera;
mod interaction;
#[cfg(feature = "test_panel")]
mod wireframe_view_specialization;

use bevy::prelude::*;
use feathrise_atmosphere::{WeatherPlugin, WorldTimePlugin};
use feathrise_grounding::GroundingPlugin;
use feathrise_settings::GraphicsSettingsPlugin;

pub use feathrise_settings::{StartupDisplayPreferences, StartupWindowPolicy};
pub use feathrise_scene_core::state::{EditorStartup, ScenePhase};
pub use interaction::WorldInteractionPlugin;

/// 显式场景装配不创建玩家或原游戏内容；场景拥有视图生命周期，应用自行提供控制。
pub struct SceneRuntimePlugin;
impl Plugin for SceneRuntimePlugin {
    fn build(&self, app: &mut App) {
        #[cfg(feature = "default_font")]
        if !app.is_plugin_added::<crate::app::default_font::DefaultFontPlugin>() {
            app.add_plugins(crate::app::default_font::DefaultFontPlugin);
        }
        #[cfg(any(feature = "dlss_frame_generation", feature = "fsr3_frame_generation"))]
        let startup_frame_generation_mode = app.world()
            .get_resource::<StartupDisplayPreferences>()
            .map(|preferences| preferences.frame_generation_mode)
            .unwrap_or_default();

        #[cfg(feature = "fsr3_upscaling")]
        app.add_plugins(feathrise_fsr3::Fsr3Plugin);
        #[cfg(feature = "test_panel")]
        app.add_plugins((
            bevy::pbr::wireframe::WireframePlugin::default(),
            wireframe_view_specialization::WireframeViewSpecializationPlugin,
        ));
        app.init_state::<ScenePhase>()
            .insert_resource(ClearColor(Color::srgb(0.5, 0.7, 1.0)))
            .add_plugins(GraphicsSettingsPlugin);
        // 工程插件可以先安装通用世界文字；公共装配仍复用同一个烘焙池和材质注册。
        if !app.is_plugin_added::<feathrise_billboard_text::BillboardTextPlugin>() {
            app.add_plugins(feathrise_billboard_text::BillboardTextPlugin);
        }
        app.add_systems(Update, billboard_camera::attach)
            .add_plugins((WorldTimePlugin, WeatherPlugin, GroundingPlugin))
            .add_plugins((
                crate::app::scene_loading::SceneLoadingPlugin,
                feathrise_scene::GameScenePlugin,
                feathrise_portal::PortalPlugin,
                WorldInteractionPlugin,
            ));
        #[cfg(any(feature = "test_panel", feature = "raytracing", feature = "rtx_rendering"))]
        app.add_plugins(feathrise_raytracing::RaytracingPlugin);
        #[cfg(feature = "dlss_frame_generation")]
        if startup_frame_generation_mode == feathrise_settings::FrameGenerationMode::Dlss2x {
            app.add_plugins(feathrise_frame_generation::FrameGenerationPlugin);
        }
        #[cfg(feature = "fsr3_frame_generation")]
        if startup_frame_generation_mode == feathrise_settings::FrameGenerationMode::Fsr3_2x {
            app.add_plugins(feathrise_fsr3_frame_generation::Fsr3FrameGenerationPlugin);
        }
    }
}
