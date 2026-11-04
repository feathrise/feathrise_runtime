//! 世界UI采样的有限RenderApp资源，管线未就绪时不允许从普通3D绘制分流。
use super::types::WorldUiSample;
use bevy::{asset::uuid_handle, prelude::*, render::render_resource::*}; // 固定Shader身份、GPU管线与绑定。 // 32字节空间采样参数。

pub(super) const WORLD_UI_SHADER: Handle<bevy::shader::Shader> =
    uuid_handle!("9648673f-619a-4a70-a789-2b519c9b9079"); // 与2DUI捕获shader分开便于GPU诊断。

#[derive(Resource, Default)] // 由世界UI阶段直接读取的非阻塞资格事实。
pub(crate) struct WorldUiCompositeReady(pub bool); // true意味着固定RGBA16F合成管线和参数缓冲都已可用。

#[derive(Resource)] // 仅保存一条固定格式管线，不随分辨率变化编译。
pub(super) struct WorldUiCompositor {
    // 世界标签当前帧的空间重采样资源。
    pub layout: BindGroupLayoutDescriptor, // 一个纹理与一个固定参数缓冲。
    pub queued: CachedRenderPipelineId,    // 后台PipelineCache编译身份。
    pub pipeline: Option<RenderPipeline>,  // 保留已成功管线，shader缓存刷新期间仍有普通标签托底。
    pub parameters: UniformBuffer<WorldUiSample>, // 同游戏队列每帧只写32字节，不等待GPU。
    pub binding: Option<(TextureViewId, BindGroup)>, // 当前主视图唯一纹理绑定，resize时才重建。
} // WorldUiCompositor结束。
