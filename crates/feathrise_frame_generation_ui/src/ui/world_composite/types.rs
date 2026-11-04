//! 世界UI空间采样的固定GPU参数；纹理只有左上render_size范围包含当前视图。
use bevy::{prelude::*, render::render_resource::ShaderType}; // 与WGSL vec4布局一致。

#[derive(Clone, Copy, Default, ShaderType)] // 固定32字节，不包含数组或跨帧历史。
pub(super) struct WorldUiSample {
    // 输出像素到当前帧独立颜色层像素的映射。
    pub mapping: Vec4, // xy为render/output比例，zw保留为零，不再存放时间抖动。
    pub bounds: Vec4,  // xy为有效render范围，zw预留并固定零。
} // WorldUiSample结束。
