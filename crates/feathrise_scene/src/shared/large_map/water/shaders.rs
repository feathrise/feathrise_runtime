//! 正式大地图 GPU 水面 WGSL 的固定句柄与注册入口。

use bevy::asset::{load_internal_asset, uuid_handle}; // 固定句柄让宿主无需复制或经AssetServer查找Shader。
use bevy::prelude::{App, Handle, Shader}; // 插件构建阶段向主世界Shader资产表登记源码。
use bevy::shader::load_shader_library; // 波形核心作为主渲染与预通道共享的可导入库。

/// 主渲染与阴影管线使用的稳定水面顶点Shader句柄。
pub(super) const DYNAMIC_WATER_VERTEX_SHADER_HANDLE: Handle<Shader> =
    uuid_handle!("9c3a7d53-3658-45b1-8d98-f84863f73761"); // 独立UUID避免与宿主及其他内部资产冲突。

/// 深度、法线、运动向量及延迟预通道使用的稳定水面顶点Shader句柄。
pub(super) const DYNAMIC_WATER_PREPASS_SHADER_HANDLE: Handle<Shader> =
    uuid_handle!("e7f2b0cc-113b-41f9-a7e6-76b06190cac2"); // 独立UUID让两个入口分别缓存。

/// 透明前向PBR通道使用的稳定水面片元Shader句柄。
pub(super) const DYNAMIC_WATER_FRAGMENT_SHADER_HANDLE: Handle<Shader> =
    uuid_handle!("3f4b8c6e-63e0-4df0-9588-c2d76f4f391a"); // 独立UUID让逐像素流纹与顶点位移分别缓存和诊断。

/// 注册共享水面波形库、两个顶点通道入口和透明标准PBR片元入口。
pub(super) fn register_dynamic_water_shaders(app: &mut App) {
    load_shader_library!(app, "shaders/water.wesl"); // 当前帧与历史帧都调用同一份波高和法线核心。
    load_shader_library!(app, "shaders/reflection.wesl"); // 同帧屏幕空间反射在透明片元中有限追踪。
    load_internal_asset!(
        app,                                // 向当前应用登记主渲染Shader资产。
        DYNAMIC_WATER_VERTEX_SHADER_HANDLE, // MaterialExtension返回同一固定句柄。
        "shaders/vertex.wesl",              // 文件随scene crate一并编译打包。
        |source, _| Shader::from_wesl(source, "embedded://feathrise_scene/shared/large_map/water/shaders/vertex.wesl")
    ); // 完成主渲染与阴影顶点入口登记。
    load_internal_asset!(
        app,                                 // 向当前应用登记预通道Shader资产。
        DYNAMIC_WATER_PREPASS_SHADER_HANDLE, // 预通道和延迟顶点入口共享该句柄。
        "shaders/prepass.wesl",              // 文件随scene crate一并编译打包。
        |source, _| Shader::from_wesl(source, "embedded://feathrise_scene/shared/large_map/water/shaders/prepass.wesl")
    ); // 完成预通道顶点入口登记。
    load_internal_asset!(
        app,                                  // 向当前应用登记逐像素水流Shader资产。
        DYNAMIC_WATER_FRAGMENT_SHADER_HANDLE, // 透明前向材质入口使用同一固定句柄。
        "shaders/fragment.wesl", // 文件随scene crate编译并继续复用StandardMaterial PBR。
        |source, _| Shader::from_wesl(source, "embedded://feathrise_scene/shared/large_map/water/shaders/fragment.wesl")
    ); // 完成动态水面片元入口登记。
} // 完成全部GPU水面Shader注册。
