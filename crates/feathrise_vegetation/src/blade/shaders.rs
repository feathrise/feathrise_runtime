//! 草地 crate 内置 WGSL 资产的固定句柄与注册入口。

use bevy::{
    asset::{load_internal_asset, uuid_handle}, // 固定句柄让打包后的 crate 无需宿主项目复制着色器文件。
    prelude::{App, Handle, Shader}, // 插件构建阶段把 WGSL 源码直接注册进 Bevy 的 Shader 资产表。
};

/// GPU 候选分类、紧凑化和间接参数收尾着色器的稳定内部句柄。
pub(crate) const GPU_GRASS_COMPUTE_SHADER_HANDLE: Handle<Shader> =
    uuid_handle!("f80107e1-0188-4d43-b0f0-13bcf1744d77"); // 独立 UUID 避免与宿主应用或其他插件的内部资产冲突。

/// GPU 草叶主视图和阴影绘制着色器的稳定内部句柄。
pub(crate) const GPU_GRASS_DRAW_SHADER_HANDLE: Handle<Shader> =
    uuid_handle!("0616899b-d96b-470e-9082-f45eaa74ca89"); // 主绘制与阴影管线继续共享同一份顶点形变源码。

/// 注册 GPU 草地核心必需的 Compute 与 Draw 着色器。
pub(crate) fn register_core_shaders(app: &mut App) {
    load_internal_asset!(
        // 使用 Bevy 官方内部资产宏把源码编译进 crate 包。
        app,                              // 向当前主应用的 Shader 资产表注册固定句柄。
        GPU_GRASS_COMPUTE_SHADER_HANDLE,  // Compute 管线后续直接克隆该强类型句柄。
        "shaders/gpu_grass_compute.wgsl", // 相对当前模块定位已进入 package 的 WGSL 文件。
        load_compute_shader              // 正式装配共享边界绑定，仍使用相同Shader资产与缓存。
    );
    load_internal_asset!(
        // 绘制着色器同样不再经过宿主 AssetServer 路径解析。
        app,                              // 向同一应用注册主视图与阴影共用源码。
        GPU_GRASS_DRAW_SHADER_HANDLE,     // RenderApp 初始化阶段直接使用该稳定句柄。
        "shaders/gpu_grass_minimal.wgsl", // 文件随 crate 发布并由 Cargo 自动纳入包。
        Shader::from_wgsl                 // 继续支持 Bevy 的 #import 与 shader_defs 预处理。
    );
}

pub(crate) fn compute_shader_source()->String {
    let mut result=crate::components::clipmap_stitch_shader_source(true);
    for line in include_str!("shaders/gpu_grass_compute.wgsl").lines(){if !line.trim_start().starts_with('#'){result.push_str(line);result.push('\n');}}
    result
}
fn load_compute_shader(_source:impl Into<String>,path:impl Into<String>)->Shader {
    Shader::from_wgsl(compute_shader_source(),path)
}
