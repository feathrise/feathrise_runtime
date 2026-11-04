//! 分块 Mesh 批次内置顶点与纯深度裁切 Shader 的句柄和注册入口。

use bevy::asset::{load_internal_asset, uuid_handle};
use bevy::prelude::{App, AssetServer, Handle, Shader, StandardMaterial};
use bevy::pbr::Material;
use bevy::shader::ShaderRef;

pub(super) const VEGETATION_MESH_BATCH_SHADER_HANDLE: Handle<Shader> =
    uuid_handle!("d4527997-f3c9-4d73-9287-9ecf355974b8");

pub(super) const VEGETATION_MESH_BATCH_DEPTH_SHADER_HANDLE: Handle<Shader> =
    uuid_handle!("b69fd472-182b-4c3f-b7eb-d56769eaa227");

pub(crate) fn register_batch_shaders(app: &mut App) -> Option<Handle<Shader>> {
    load_internal_asset!(
        app,
        VEGETATION_MESH_BATCH_SHADER_HANDLE,
        "../shaders/mesh_batch.wgsl",
        Shader::from_wgsl
    );
    load_internal_asset!(
        app,
        VEGETATION_MESH_BATCH_DEPTH_SHADER_HANDLE,
        "../shaders/mesh_batch_depth.wgsl",
        Shader::from_wgsl
    );
    // 按官方ShaderRef取得身份，兼容继承StandardMaterial的风/公告板扩展；不能覆盖任意材质的自定义片元逻辑。
    match StandardMaterial::prepass_fragment_shader() {
        ShaderRef::Path(path) => Some(app.world().resource::<AssetServer>().load(path)),
        ShaderRef::Handle(handle) => Some(handle),
        ShaderRef::Default => None,
    }
}

// 与环境风 Shader 的跨包组合由 tests/workspace 验证；发布后的单包不读取相邻源码。
