// 游戏设置 — StandardMaterial 纹理过滤的 RenderApp 接管入口。

mod state; // 材质与纹理依赖注册表、分批过渡状态。
mod systems; // 提取阶段跟踪与 GPU 采样器/材质绑定应用系统。

use bevy::{
    pbr::{MeshMaterial3d, StandardMaterial},
    prelude::*,
    render::{
        ExtractSchedule, Render, RenderApp, RenderSystems,
        erased_render_asset::{
            AssetExtractionSystems as ErasedAssetExtractionSystems, prepare_erased_assets,
        },
        render_asset::{AssetExtractionSystems as RenderAssetExtractionSystems, prepare_assets},
        texture::GpuImage,
    },
};

/// 注册纹理过滤的提取与分批 GPU 应用路径；无 RenderApp 的测试应用安全跳过。
pub(crate) fn configure_texture_filtering(app: &mut App) {
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return; // 纯 ECS 或无渲染测试不要求创建 GPU 资源。
    };

    render_app
        .init_resource::<state::TextureFilteringState>()
        .add_systems(
            ExtractSchedule,
            systems::extract_texture_filtering_state
                .after(ErasedAssetExtractionSystems) // 先读取 Bevy 已提取的 StandardMaterial 克隆。
                .after(RenderAssetExtractionSystems), // 同时读取已提取 Image 的原始采样器。
        )
        .add_systems(
            Render,
            systems::apply_texture_filtering
                .in_set(RenderSystems::PrepareAssets)
                .after(prepare_assets::<GpuImage>) // GPU 图片就绪后才能替换其采样器句柄。
                .before(prepare_erased_assets::<MeshMaterial3d<StandardMaterial>>), // 材质绑定必须在本帧读取到新采样器后再构建。
        );
}
