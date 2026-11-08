//! GPU公告板crate内置WGSL的固定句柄与注册入口。

use bevy::asset::{load_internal_asset, uuid_handle}; // 固定句柄让宿主无需复制或经AssetServer查找Shader。
use bevy::prelude::{App, Handle, Shader}; // 插件构建阶段向主世界Shader资产表登记源码。
use bevy::shader::load_shader_library; // 世界矩阵与法线核心由主渲染和预通道共同导入。

/// 主渲染与阴影管线使用的稳定GPU公告板顶点Shader句柄。
pub(super) const VEGETATION_BILLBOARD_VERTEX_SHADER_HANDLE: Handle<Shader> =
    uuid_handle!("f76f2d6e-6b6c-4bb4-b391-2c6da40e1d45"); // 独立UUID避免与其他内部Shader资产冲突。

/// 深度、法线、运动向量与延迟预通道使用的稳定顶点Shader句柄。
pub(super) const VEGETATION_BILLBOARD_PREPASS_SHADER_HANDLE: Handle<Shader> =
    uuid_handle!("e35d0d5f-802c-45f7-9f7b-ea3c176df104"); // 独立UUID让两个接口分别缓存。

/// 注册共享GPU朝向库和两个Bevy PBR顶点入口。
pub(super) fn register_vegetation_billboard_shaders(app: &mut App) {
    load_shader_library!(app, "shaders/billboard.wesl"); // 主、阴影与预通道共用唯一世界矩阵核心。
    load_internal_asset!(
        app,                                       // 向当前应用登记主渲染Shader资产。
        VEGETATION_BILLBOARD_VERTEX_SHADER_HANDLE, // MaterialExtension返回同一固定句柄。
        "shaders/vertex.wesl",                     // 文件随vegetation crate一并编译打包。
        |source, _| Shader::from_wesl(source, "embedded://feathrise_vegetation/mesh_backend/billboard/shaders/vertex.wesl")
    ); // 完成主渲染与阴影顶点入口登记。
    load_internal_asset!(
        app,                                        // 向当前应用登记预通道Shader资产。
        VEGETATION_BILLBOARD_PREPASS_SHADER_HANDLE, // 预通道与延迟顶点入口共享句柄。
        "shaders/prepass.wesl",                     // 文件随vegetation crate一并编译打包。
        |source, _| Shader::from_wesl(source, "embedded://feathrise_vegetation/mesh_backend/billboard/shaders/prepass.wesl")
    ); // 完成预通道与延迟顶点入口登记。
} // 完成全部GPU公告板Shader注册。
