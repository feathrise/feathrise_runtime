//! 自行装配 Bevy App，再加入 SDK 运行时插件。

#[path = "../../support/mod.rs"]
mod demo;

use bevy::{
    prelude::*,
    window::{PresentMode, WindowResolution},
};
use feathrise_runtime::{ProjectRuntimePlugin, prepare_project};

fn main() -> Result<(), String> {
    let (project, game) = demo::load()?;
    feathrise_runtime::prepare_vulkan_environment();
    let mut app = App::new();
    // Bevy 初始化 AssetPlugin 前先登记工程资产源，窗口参数由本工程设置。
    let assets = prepare_project(&mut app, &project)?;
    let plugins = DefaultPlugins.set(assets).set(WindowPlugin {
        primary_window: Some(Window {
            title: "Feathrise · Minimal project".into(),
            resolution: WindowResolution::new(960, 640),
            present_mode: PresentMode::Fifo,
            ..default()
        }),
        ..default()
    });
    #[cfg(feature = "function_hotpatch")]
    let plugins = plugins.disable::<bevy::app::hotpatch::HotPatchPlugin>();
    app.add_plugins(plugins)
        .add_plugins((ProjectRuntimePlugin, game));
    feathrise_runtime::install_vulkan_surface_guard(&mut app);
    match app.run() {
        AppExit::Success => Ok(()),
        exit => Err(format!("示例运行失败：{exit:?}")),
    }
}
