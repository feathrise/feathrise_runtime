//! 雨向相机独立标记，禁止误处理玩法、预览和水面相机。
use bevy::prelude::*; // 相机组件派生。
use bevy::render::extract_component::ExtractComponent; // 只提取专用标记。
#[derive(Component, Clone, Copy, ExtractComponent)] // 固定标记跨主世界与渲染世界。
#[extract_app(bevy::render::RenderApp)]
pub(super) struct RainOcclusionCamera; // 无颜色输出的纯深度相机。
