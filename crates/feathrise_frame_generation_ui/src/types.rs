//! 公共 UI 的后端身份、排序锚点及 GPU 帧结果，不包含原生句柄。
use bevy::{
    prelude::*,
    render::{render_resource::TextureFormat, texture::CachedTexture},
}; // 只持有 wgpu 管理的强引用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)] // 后端切换可精确撤销自己的请求。
pub enum FrameGenerationUiBackend {
    Dlss,
    Fsr3,
} // 不激活的后端不能清除另一个后端请求。
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)] // Core3d 同一视图内的完整事务顺序。
pub enum FrameGenerationUiSystems {
    Prepare,
    Begin,
    Capture,
    BackendFinalize,
    Restore,
} // 后端在 Prepare 授权，在 BackendFinalize 消费图像。
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)] // 主世界请求发布需早于字形引导暂停。
pub enum FrameGenerationUiRequestSystems {
    Publish,
} // 后端发布统一锚点。
pub struct UiFrame {
    // 仅由 Capture 发布完整当前帧图像。
    pub frame: u32,             // 同帧身份，禁止前一帧冒充当前帧。
    pub camera: Entity,         // 唯一主世界玩法相机。
    pub size: UVec2,            // 完整输出尺寸。
    pub format: TextureFormat,  // 与交换链视图一致的 sRGB 格式。
    pub texture: CachedTexture, // texture/default_view 均强引用到后端完成消费。
} // UiFrame 结束。
