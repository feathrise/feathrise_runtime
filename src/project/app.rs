//! 工程场景与渲染装配；工程插件负责玩法，不隐式创建原RPG玩家。

use crate::project::{ProjectContext, assets, config, startup};
#[cfg(feature = "validation")]
use crate::project::smoke;
use avian3d::{
    PhysicsPlugins,
    prelude::{ColliderCachePlugin, PhysicsSchedulePlugin},
};
use bevy::{
    asset::{AssetPlugin, AssetServer},
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use crate::app::game_plugins::SceneRuntimePlugin;
use feathrise_settings::{HostManagedPresentMode, StartupDisplayPreferences, StartupWindowPolicy};
use feathrise_scene_core::state::{ScenePhase, PendingSceneTransition};
use bevy_framepace::FramepacePlugin;
use bevy_hanabi::HanabiPlugin;

/// 在宿主的 App 中准备独立工程。请在安装 AssetPlugin 之前调用，
/// 将返回值交给 `DefaultPlugins.set(...)`，再添加 [`ProjectRuntimePlugin`]。
/// 文件校验在启动阶段完成，工程路径和资产源绑定到本 App。
pub fn prepare_project(app: &mut App, project: &ProjectContext) -> Result<AssetPlugin, String> {
    let startup = startup::load_optional(project, false)?;
    let assets = prepare_project_app(app, project, startup)?;
    // 宿主选择的呈现模式归自己的窗口配置和系统管理。
    app.insert_resource(HostManagedPresentMode);
    Ok(assets)
}

/// 在已经装配 Bevy 基础插件的 App 上安装工程的场景生命周期和资产系统。
/// 宿主应先调用 [`prepare_project`]，并在自己的入口决定何时运行 App。
pub struct ProjectRuntimePlugin;

impl Plugin for ProjectRuntimePlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.world().contains_resource::<ProjectContext>()
                && app.world().contains_resource::<AssetServer>(),
            "请先准备 Feathrise 工程并安装返回的 AssetPlugin",
        );
        install_physics_and_effects(app);
        // 复用公共场景加载执行器，工程资产由独立的准备系统提供。
        if !app.is_plugin_added::<SceneRuntimePlugin>() {
            app.add_plugins(SceneRuntimePlugin);
        }
        if !app.is_plugin_added::<crate::app::scene_loading::ProjectSceneAssetsPlugin>() {
            app.add_plugins(crate::app::scene_loading::ProjectSceneAssetsPlugin);
        }
        if app.world().contains_resource::<startup::SavedGameStartup>() {
            app.add_systems(Startup, enter_saved_scene);
        }
    }
}

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
    app.insert_resource(display).insert_resource(policy);
    let assets = prepare_project_app(&mut app, &project, startup)?;
    app.add_plugins(config::default_plugins(policy, display, assets, is_embedded));
    // 保留便捷入口的装配顺序，工程插件可在 build 中使用既有的物理和粒子资源。
    install_physics_and_effects(&mut app);
    app.add_plugins(FramepacePlugin)
        // 项目自有资源先于公共插件登记；玩法由调用方提供，SDK不补原游戏默认玩家。
        .add_plugins(project_plugin)
        .add_plugins(ProjectRuntimePlugin);
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

fn prepare_project_app(
    app: &mut App,
    project: &ProjectContext,
    startup: Option<startup::SavedGameStartup>,
) -> Result<AssetPlugin, String> {
    if app.world().contains_resource::<AssetServer>() {
        return Err("请在 AssetPlugin 初始化前准备工程资产源".into());
    }
    if app.world().contains_resource::<ProjectContext>() {
        return Err("当前 App 已绑定 Feathrise 工程".into());
    }
    let first = project.asset_roots.first().ok_or("工程未登记默认资产根")?;
    let runtime_root = feathrise_scene_content::RuntimeAssetRoot::new(first)?;
    let assets = assets::install(app, project)?;
    app.insert_resource(runtime_root)
        .insert_resource(project.clone())
        .init_resource::<StartupDisplayPreferences>()
        .init_resource::<StartupWindowPolicy>()
        .insert_resource(crate::project::ProjectViewOrigin(
            startup.as_ref().map(|value| value.spawn).unwrap_or_default(),
        ));
    if let Some(startup) = startup {
        app.insert_resource(startup);
    } else {
        // 空工程不采用其它工程的默认场景或菜单。
        app.insert_resource(feathrise_scene_core::state::EditorStartup);
    }
    #[cfg(feature = "rtx_rendering")]
    if !app.world().contains_resource::<bevy::anti_alias::dlss::DlssProjectId>() {
        app.insert_resource(bevy::anti_alias::dlss::DlssProjectId(
            bevy::asset::uuid::uuid!("14b34155-a529-4df8-8d57-b4a76b98807d"),
        ));
    }
    // 帧生成启动插件须先于 RenderPlugin，宿主可在准备工程前设置显示偏好。
    #[cfg(all(
        target_os = "windows",
        any(feature = "dlss_frame_generation", feature = "fsr3_frame_generation")
    ))]
    let mode = app.world().resource::<StartupDisplayPreferences>().frame_generation_mode;
    #[cfg(all(feature = "dlss_frame_generation", target_os = "windows"))]
    if mode == feathrise_settings::FrameGenerationMode::Dlss2x {
        app.add_plugins(feathrise_frame_generation::StreamlineBootstrapPlugin);
    }
    #[cfg(all(feature = "fsr3_frame_generation", target_os = "windows"))]
    if mode == feathrise_settings::FrameGenerationMode::Fsr3_2x {
        app.add_plugins(feathrise_fsr3_frame_generation::Fsr3FrameGenerationBootstrapPlugin);
    }
    Ok(assets)
}

fn install_physics_and_effects(app: &mut App) {
    // 自定义物理步进与粒子插件由宿主先安装，SDK 复用它们的资源。
    if !app.is_plugin_added::<PhysicsSchedulePlugin>() {
        app.add_plugins(PhysicsPlugins::default().build().disable::<ColliderCachePlugin>());
    }
    if !app.is_plugin_added::<HanabiPlugin>() {
        app.add_plugins(HanabiPlugin);
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::io::AssetSourceBuilders;

    #[test]
    fn prepare_project_binds_assets_without_taking_over_the_host_app() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/support")
            .canonicalize()
            .unwrap();
        let project = ProjectContext {
            asset_roots: vec![root.join("assets")],
            root,
            id: "hosted-project-test".into(),
            name: "宿主入口测试".into(),
        };
        let cwd = std::env::current_dir().unwrap();
        let mut app = App::new();
        let world_id = app.world().id();
        let preferences = StartupDisplayPreferences {
            vsync: true,
            ..default()
        };
        app.insert_resource(preferences);

        // 第二个来源是普通文件，失败后连已经验证过的第一项也不能登记。
        let mut invalid = project.clone();
        invalid
            .asset_roots
            .push(project.asset_root().join("scenes/catalog.scene_catalog.json"));
        assert!(prepare_project(&mut app, &invalid).is_err());
        assert!(!app.world().contains_resource::<AssetSourceBuilders>());
        assert!(!app.world().contains_resource::<ProjectContext>());

        let assets = prepare_project(&mut app, &project).unwrap();
        assert_eq!(app.world().id(), world_id);
        assert_eq!(std::env::current_dir().unwrap(), cwd);
        assert_eq!(
            *app.world().resource::<StartupDisplayPreferences>(),
            preferences,
        );
        assert_eq!(app.world().resource::<ProjectContext>().id, project.id);
        assert!(app.world().contains_resource::<HostManagedPresentMode>());
        assert!(!app.world().contains_resource::<AssetServer>());
        assert!(prepare_project(&mut app, &project).is_err());
        app.add_plugins((MinimalPlugins, assets));
        assert!(app.world().contains_resource::<AssetServer>());
        assert!(matches!(
            prepare_project(&mut app, &project),
            Err(error) if error.contains("AssetPlugin")
        ));
    }
}
