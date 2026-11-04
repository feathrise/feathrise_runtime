//! 独立UI的RenderApp资源，仅保留固定管线与最近一组图像。
use super::types::{ActiveUi, UiBinding, UiTargets};
use bevy::{asset::uuid_handle, prelude::*, render::render_resource::*}; // 固定Shader身份与GPU资源句柄。 // 按类型层复用有限状态。

pub(super) const UI_SHADER: Handle<bevy::shader::Shader> =
    uuid_handle!("a138d847-fc67-4e6b-b5db-cc5a82c60151"); // 独立内嵌着色器身份。

#[derive(Resource)] // 设备建立后只排队三条固定格式管线。
pub(super) struct UiPipelines {
    // 不随尺寸变化重新编译管线。
    pub layout: BindGroupLayoutDescriptor, // 单纹理、非过滤采样绑定。
    pub bgra: CachedRenderPipelineId,      // BGRA sRGB交换链编码。
    pub rgba: CachedRenderPipelineId,      // RGBA sRGB交换链编码。
    pub restore: CachedRenderPipelineId,   // RGBA16F场景上预乘混合UI。
} // UiPipelines结束。

#[derive(Resource, Default)] // 不请求FSR时保持空集合。
pub(super) struct UiCapture {
    // 固定两个主纹理绑定、一张持久UI输出与至多一个活动事务。
    pub bindings: [Option<UiBinding>; 2], // ViewTarget的A/B两张主颜色，不随帧数增长。
    pub targets: Option<UiTargets>,       // 尺寸变化需经历原生交换链quiesce后才替换。
    pub active: Option<ActiveUi>,         // begin之后finish必须恢复普通完整颜色。
} // UiCapture结束。
