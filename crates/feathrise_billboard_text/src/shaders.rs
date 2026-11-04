//! Billboard 文字材质使用的内置顶点Shader句柄与注册入口。

use bevy::asset::{load_internal_asset, uuid_handle}; // 让通用crate不依赖宿主项目的assets目录。
use bevy::prelude::{App, Handle, Shader}; // 插件构建阶段登记两个PBR兼容顶点入口。

/// 最终透明文字和时间引导主通道使用的顶点Shader。
pub(super) const BILLBOARD_COLOR_VERTEX_SHADER: Handle<Shader> =
    uuid_handle!("d5b6d58e-3647-47c8-9bd9-5f3f5fa6fb1c");

/// 时间引导深度、法线、运动向量和Deferred通道使用的顶点Shader。
pub(super) const BILLBOARD_PREPASS_VERTEX_SHADER: Handle<Shader> =
    uuid_handle!("a8c782e0-3c1f-44f1-97cc-a3ca14f982f8");

/// 注册与Bevy StandardMaterial前向/预通道接口完全匹配的自有顶点入口。
pub(super) fn register_billboard_shaders(app: &mut App) {
    load_internal_asset!(
        app,
        BILLBOARD_COLOR_VERTEX_SHADER,
        "shaders/color_vertex.wgsl",
        Shader::from_wgsl
    ); // 主颜色层按材质Uniform选择抖动或无抖动投影。
    load_internal_asset!(
        app,
        BILLBOARD_PREPASS_VERTEX_SHADER,
        "shaders/prepass_vertex.wgsl",
        Shader::from_wgsl
    ); // RR引导层与主颜色层共享同一投影模式。
}
