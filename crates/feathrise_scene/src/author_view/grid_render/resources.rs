//! 无限参考网格的透明通道管线和最多8项缓冲绑定缓存；不持有地图或场景实体。
use bevy::{core_pipeline::FullscreenShader, prelude::*, render::render_resource::*};

#[derive(Resource)]
pub(super) struct GridPipeline {
    pub(super) layout: BindGroupLayoutDescriptor,
    pub(super) fullscreen: FullscreenShader,
    pub(super) shader: Handle<Shader>,
}

#[derive(Default)]
pub(super) struct BindCache {
    pub(super) entries: Vec<((BufferId, BufferId), BindGroup)>,
}
