//! 地图作者动态天空/天空盒的相机级异步加载、Cube 校验、热更新与精确回退。

use std::{path::Path, path::PathBuf}; // Bevy 同名内嵌 Shader 覆盖需要稳定虚拟路径与 watcher 源路径。

use bevy::{asset::io::embedded::EmbeddedAssetRegistry, prelude::App}; // 插件 build 阶段在 RenderStartup 前替换天空盒源码。

mod components; // 公开请求/状态与私有原状态快照。
mod environment; // 小型漫反射Cube缩放缓存把补光与高分辨率镜面强度分开。
mod systems; // 只响应请求和Image资产事件的有界运行时。

pub use components::{
    AuthoredSkyMode, AuthoredSkyboxPhase, AuthoredSkyboxRequest, AuthoredSkyboxStatus,
}; // 场景与 F12 共用同一互斥模式、请求入口和诊断。
pub(crate) use environment::install_authored_environment_map_support; // 插件启动时建立共享黑Cube与有界派生缓存。
pub(crate) use systems::{
    apply_authored_skyboxes, begin_authored_skybox_requests, observe_authored_skybox_assets,
}; // 世界时间插件固定加载、事件与采用顺序。

/// Bevy 0.19.1 天空盒管线读取的官方内嵌 Shader 虚拟路径。
const SKYBOX_EMBEDDED_PATH: &str = "bevy_core_pipeline/skybox/skybox.wgsl"; // 路径漂移必须由源码契约测试显式暴露。
/// 本项目仅修正主通道 viewport 寻址的天空盒 Shader 源码路径。
const SKYBOX_SOURCE_PATH: &str = "src/skybox/shaders/skybox_main_pass_viewport.wgsl"; // watcher 与 include_bytes 指向唯一正式副本。

/// 在 Bevy 创建天空盒 pipeline 前安装低分辨率主通道兼容 Shader。
pub(crate) fn install_main_pass_viewport_skybox_shader(app: &mut App) {
    let Some(registry) = app.world().get_resource::<EmbeddedAssetRegistry>() else {
        return; // 无 AssetPlugin 的纯逻辑宿主没有天空盒 pipeline，也无需安装渲染兼容层。
    };
    registry.insert_asset(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(SKYBOX_SOURCE_PATH), // watcher 跟踪实际编入 crate 的兼容源码。
        Path::new(SKYBOX_EMBEDDED_PATH), // 只覆盖 Bevy 天空盒 root，不改变 PBR、DLSS 或相机 uniform 布局。
        &include_bytes!("shaders/skybox_main_pass_viewport.wgsl")[..], // 主通道尺寸变化只由已有 View uniform 消费。
    ); // 覆盖发生在 RenderStartup 之前，稳定帧没有 CPU 或 GPU 额外工作。
}

#[cfg(test)]
mod tests {
    use super::*; // 直接锁定正式内嵌路径和实际编入二进制的 WGSL 源码。

    #[test]
    fn skybox_ray_reconstruction_uses_main_pass_viewport() {
        let source = include_str!("shaders/skybox_main_pass_viewport.wgsl"); // 检查 pipeline 实际加载的同一份源码。
        assert_eq!(
            SKYBOX_EMBEDDED_PATH,
            "bevy_core_pipeline/skybox/skybox.wgsl"
        ); // Bevy 升级导致官方路径变化时必须显式复审覆盖入口。
        assert!(
            source.contains("coords_to_ray_direction(in.position.xy, view.main_pass_viewport)")
        ); // 天空方向必须按本帧真实主通道尺寸覆盖完整视野。
        assert!(!source.contains("coords_to_ray_direction(in.position.xy, view.viewport)")); // 禁止重新按输出尺寸压缩内部像素 UV 并放大天空。
        assert_eq!(
            source
                .matches("coords_to_ray_direction(in.position.xy")
                .count(),
            1
        ); // 保持唯一片元入口，避免另一路径绕过兼容 viewport。
    }
}
