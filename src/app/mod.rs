//! 公共运行装配与Vulkan呈现保护；不依赖原游戏应用入口。
pub mod game_plugins;
pub mod scene_loading; // 无界面、无PAK的场景加载交接；应用各自提交要等待的关键资产。
#[cfg(any(feature = "default_font", test))]
pub mod default_font;
#[cfg(target_os = "windows")]
mod vulkan_surface_guard;
pub fn prepare_vulkan_environment() {
    #[cfg(target_os = "windows")]
    vulkan_surface_guard::disable_incompatible_gamepp_vulkan_layer();
}
pub fn install_vulkan_surface_guard(app: &mut bevy::prelude::App) {
    #[cfg(target_os = "windows")]
    app.add_plugins(vulkan_surface_guard::VulkanSurfaceGuardPlugin);
    #[cfg(not(target_os = "windows"))]
    let _ = app;
}
