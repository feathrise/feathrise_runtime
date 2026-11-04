//! 正式工程常驻视口：保留独立运行入口，只有完整嵌入启动参数才安装控制管道与作者输入。

pub(crate) mod asset_overlay;
mod author_view;
mod input_handoff;
#[cfg(feature = "function_hotpatch")]
pub(crate) mod hotpatch;
mod native_region;
mod overlay;
mod tools;
mod tool_brush;
mod placement_update;
mod preview;
mod preview_toolbar;
#[cfg(feature = "validation")]
mod preview_validation;
mod resources;
mod snapshot;
mod source_readiness;
mod systems;
#[cfg(test)]
mod tests;
mod transport;
pub(crate) use transport::{take_channels as take_author_channels, executable_hash as author_executable_hash};
mod types;
#[cfg(feature = "validation")]
mod validation;
#[cfg(feature = "validation")]
mod validation_capture;
mod window;

use bevy::prelude::*;
pub(super) use transport::Configuration;
pub use types::*;
pub use preview::{AuthorDomainExtension,AuthorDomainExtensionStatus,AuthorDomainExtensionScene};
#[cfg(feature = "function_hotpatch")]
pub use hotpatch::FunctionHotpatchState;

/// 工程自有移动、AI 和其他主动玩法应加入此集合；普通独立游戏永远开放。
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectGameplay;

/// Winit的Last完成后只校正原生租约；尺寸发布固定留在First，避免相机与附件跨帧。
#[derive(bevy::ecs::schedule::ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
struct EmbeddedWindowSync;

pub(super) fn prepare(
    app: &mut App,
    config: Configuration,
    project: &crate::project::ProjectContext,
) -> Result<(), String> {
    let transport = transport::start(&config, project)?;
    #[cfg(feature = "function_hotpatch")]
    let hotpatch_available = hotpatch::prepare(app, &config, &transport.build_identity)?;
    // WorldId 由本次真实 App 生成，再结合进程/启动身份，不能把宿主给的序号当 World 证据。
    use std::hash::{Hash, Hasher};
    let mut identity = std::collections::hash_map::DefaultHasher::new();
    app.world().id().hash(&mut identity);
    std::process::id().hash(&mut identity);
    config.identity.session.hash(&mut identity);
    let world_id = identity.finish().max(1);
    let runtime = resources::Runtime::new(config, transport);
    #[cfg(feature = "function_hotpatch")]
    let runtime = { let mut runtime = runtime; runtime.hotpatch_available = hotpatch_available; runtime };
    app.init_resource::<asset_overlay::AssetOverlay>()
        .insert_resource(EmbeddedProjectState {
            mode: feathrise_editor_protocol::Mode::Editing,
            world_id,
            revision: 0,
            transitioning: false,
        })
        .insert_non_send(runtime)
        .insert_resource(bevy::winit::WinitSettings::continuous()); // 编辑态子窗口无焦点仍应持续显示和消费IPC。
    #[cfg(feature = "validation")]
    validation::install(app, project)?;
    Ok(())
}

pub(super) fn install(app: &mut App) {
    app.configure_sets(Update, ProjectGameplay.run_if(project_simulation_active))
        .configure_sets(
            FixedUpdate,
            ProjectGameplay.run_if(project_simulation_active),
        );
    if !app.world().contains_non_send::<resources::Runtime>() {
        return;
    }
    #[cfg(feature = "function_hotpatch")]
    hotpatch::install(app);
    overlay::install(app);
    tools::install(app);
    tool_brush::install(app);
    preview::install(app);
    author_view::install(app);
    preview_toolbar::install(app);
    #[cfg(feature = "validation")]
    preview_validation::install_render(app);
    source_readiness::install(app);
    input_handoff::install(app);
    app.init_schedule(EmbeddedWindowSync);
    app.world_mut().resource_mut::<bevy::app::MainScheduleOrder>()
        .insert_after(Last, EmbeddedWindowSync);
    app.init_resource::<systems::HeightSources>()
        .add_systems(First, window::synchronize)
        .add_systems(PreUpdate, window::close_requested.before(bevy::input::InputSystems))
        .add_systems(
            Update,
            (systems::drive, systems::heights)
                .chain()
                .before(feathrise_scene_core::input::GamePauseSystemSet::PublishGateFacts)
                .before(ProjectGameplay),
        )
        .add_systems(Update, systems::gameplay_tick.in_set(ProjectGameplay))
        .add_systems(
            PostUpdate,
            systems::camera.before(bevy::transform::TransformSystems::Propagate),
        )
        .add_systems(PostUpdate, window::capture_requests
            .before(bevy::camera::CameraUpdateSystems)
            .before(bevy::ui::UiSystems::Layout))
        .add_systems(EmbeddedWindowSync, window::finalize);
}
