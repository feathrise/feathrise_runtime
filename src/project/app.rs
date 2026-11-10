//! 工程场景与渲染装配；工程插件负责玩法，不隐式创建原RPG玩家。

use crate::project::{ProjectContext, assets, config, startup};
#[cfg(feature = "validation")]
use crate::project::smoke;
use avian3d::{PhysicsPlugins, prelude::ColliderCachePlugin};
use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use crate::app::game_plugins::{SceneRuntimePlugin};
use feathrise_settings::{StartupDisplayPreferences, StartupWindowPolicy};
use feathrise_scene_core::state::{ScenePhase, PendingSceneTransition};
use bevy_framepace::FramepacePlugin;
use bevy_hanabi::HanabiPlugin;

/// 工程显式提供自己的身份与插件，并以自身目录作为路径解析根。
pub fn run(project: ProjectContext, project_plugin: impl Plugin) -> Result<(), String> {
    if crate::project::author_preview::requested() {
        return crate::project::author_preview::run(project, project_plugin);
    }
    // 既有设置使用相对路径；在任务池/窗口创建前绑定本工程，之后终生不换根。
    std::env::set_current_dir(&project.root)
        .map_err(|error| format!("工程目录不可用：{error}"))?;
    let saved_play = std::env::var_os("EDITOR_SAVED_PLAY").is_some_and(|value| value == "1");
    let embedded = crate::project::embedded::Configuration::from_args(&project)?;
    let is_embedded = embedded.is_some();
    let startup = if is_embedded { None } else { startup::load_optional(&project, saved_play)? };
    crate::app::prepare_vulkan_environment();
    let mut app = App::new();
    #[cfg(feature = "function_hotpatch")]
    app.init_resource::<bevy::ecs::HotPatchChanges>().add_message::<bevy::ecs::HotPatched>();
    if let Some(config) = embedded { crate::project::embedded::prepare(&mut app, config, &project)?; }
    // 工程拥有自己的设置策略；公共启动不读取原游戏的 settings.json 聚合文件。
    let display = StartupDisplayPreferences::default();
    let policy = StartupWindowPolicy {
        // 嵌入只是作者临时租用窗口，不能压掉工程PostStartup表达的玩家全屏偏好。
        // SDK窗口租约会先保存工程请求，再在编辑态临时窗口化；预览归还同一原生窗口。
        force_windowed: saved_play,
    };
    app.insert_resource(feathrise_scene_content::RuntimeAssetRoot::new(
        project.asset_root(),
    )?)
    .insert_resource(project.clone())
    .insert_resource(display)
    .insert_resource(policy);
    app.insert_resource(crate::project::ProjectViewOrigin(startup.as_ref().map(|value| value.spawn).unwrap_or_default()));
    if let Some(startup) = startup {
        app.insert_resource(startup);
    } else {
        // 没有作者场景时保持真正空世界，不退回原游戏主菜单或默认场景。
        app.insert_resource(feathrise_scene_core::state::EditorStartup);
    }
    #[cfg(feature = "rtx_rendering")]
    app.insert_resource(bevy::anti_alias::dlss::DlssProjectId(
        bevy::asset::uuid::uuid!("14b34155-a529-4df8-8d57-b4a76b98807d"),
    ));
    #[cfg(all(feature = "dlss_frame_generation", target_os = "windows"))]
    if display.frame_generation_mode == feathrise_settings::FrameGenerationMode::Dlss2x {
        app.add_plugins(feathrise_frame_generation::StreamlineBootstrapPlugin);
    }
    #[cfg(all(feature = "fsr3_frame_generation", target_os = "windows"))]
    if display.frame_generation_mode == feathrise_settings::FrameGenerationMode::Fsr3_2x {
        app.add_plugins(feathrise_fsr3_frame_generation::Fsr3FrameGenerationBootstrapPlugin);
    }
    let assets = assets::install(&mut app, &project)?;
    app.add_plugins(config::default_plugins(policy, display, assets, is_embedded))
        .add_plugins(
            PhysicsPlugins::default()
                .build()
                .disable::<ColliderCachePlugin>(),
        )
        .add_plugins(HanabiPlugin)
        .add_plugins(FramepacePlugin)
        // 项目自有资源先于公共插件登记；玩法由调用方提供，SDK不补原游戏默认玩家。
        .add_plugins(project_plugin);
    app.add_plugins((
        SceneRuntimePlugin,
        crate::app::scene_loading::ProjectSceneAssetsPlugin,
    )); // 工程资产准备复用公共加载执行器，不伪造旧游戏的 PAK 就绪资源。
    if app.world().contains_resource::<startup::SavedGameStartup>() {
        app.add_systems(Startup, enter_saved_scene);
    }
    crate::app::install_vulkan_surface_guard(&mut app);
    crate::project::embedded::install(&mut app);
    #[cfg(feature = "validation")]
    if !is_embedded { smoke::install(&mut app, &project, saved_play)?; }
    if saved_play && !is_embedded {
        app.add_systems(Update, exit_saved_play);
    }
    println!(
        "NEW_EDITOR_SAVED_GAME_START {}",
        serde_json::json!({
            "root":project.root, "project_id":project.id, "saved_play":saved_play,
            "scene_id":app.world().get_resource::<startup::SavedGameStartup>().map(|startup| startup.scene_id.0.clone()),
            "runtime":"production-SceneRuntimePlugin",
        })
    );
    match app.run() {
        AppExit::Success => Ok(()),
        exit => Err(format!("工程实例退出失败：{exit:?}")),
    }
}

fn enter_saved_scene(
    mut commands: Commands,
    startup: Res<startup::SavedGameStartup>,
    mut active: ResMut<feathrise_scene_content::ActiveDynamicScene>,
    mut destination: ResMut<feathrise_portal::PortalDestination>,
    mut next: ResMut<NextState<ScenePhase>>,
) {
    active.select(startup.scene_id.clone());
    destination.dynamic_scene_id = Some(startup.scene_id.0.clone());
    destination.spawn_pos = Some(startup.spawn);
    // false保留明确启动场景及工程配置，不经原游戏菜单选择或重置。
    commands.insert_resource(
        PendingSceneTransition::named(ScenePhase::Active, &startup.name)
            .with_fresh_world(),
    );
    next.set(ScenePhase::Loading);
}

fn exit_saved_play(
    keys: Res<ButtonInput<KeyCode>>,
    mut cursors: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut exit: MessageWriter<AppExit>,
) {
    if !keys.just_pressed(KeyCode::F12) {
        return;
    }
    for mut cursor in &mut cursors {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
    exit.write(AppExit::Success);
}
