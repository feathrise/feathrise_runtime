//! Mesh 风 crate 内置 WGSL 的固定句柄与注册入口。

use bevy::asset::{load_internal_asset, uuid_handle}; // 固定句柄让宿主无需复制或经AssetServer查找Shader。
use bevy::prelude::{App, Handle, Shader}; // 插件构建阶段向主世界Shader资产表登记源码。
use bevy::shader::load_shader_library; // 位移核心作为主渲染与预通道共享的可导入库。

/// 主渲染与阴影管线使用的稳定顶点Shader句柄。
pub(super) const TREE_WIND_VERTEX_SHADER_HANDLE: Handle<Shader> =
    uuid_handle!("718576e8-b974-41e3-b376-779ecabe3b8f"); // 独立UUID避免与宿主及其他插件内部资产冲突。

/// 深度、法线、运动向量及延迟预通道使用的稳定顶点Shader句柄。
pub(super) const TREE_WIND_PREPASS_SHADER_HANDLE: Handle<Shader> =
    uuid_handle!("218fb11c-62ee-4bbb-aa96-26bd4dd90803"); // 独立UUID让两个入口可被材质管线分别缓存。

/// 注册共享位移库和两个顶点入口。
pub(super) fn register_mesh_wind_shaders(app: &mut App) {
    load_shader_library!(app, "shaders/wind.wgsl"); // 当前帧与历史帧都调用这一份顶点位移核心。
    load_internal_asset!(
        app,                            // 向当前应用登记主渲染Shader资产。
        TREE_WIND_VERTEX_SHADER_HANDLE, // MaterialExtension返回同一固定句柄。
        "shaders/vertex.wgsl",          // 文件随environment crate一并编译打包。
        Shader::from_wgsl               // 保留Bevy的#import、条件定义和绑定组替换。
    ); // 完成主渲染与阴影顶点入口登记。
    load_internal_asset!(
        app,                             // 向当前应用登记预通道Shader资产。
        TREE_WIND_PREPASS_SHADER_HANDLE, // 预通道和延迟顶点入口共享该句柄。
        "shaders/prepass.wgsl",          // 文件随environment crate一并编译打包。
        Shader::from_wgsl                // 保留运动向量、预通道和延迟条件编译。
    ); // 完成预通道顶点入口登记。
} // 完成全部自有Mesh风Shader注册。
