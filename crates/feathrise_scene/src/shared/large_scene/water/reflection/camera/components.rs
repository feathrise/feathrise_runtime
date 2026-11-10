//! 只有主世界通过水面反射资格门的视图才允许复制当前帧色深。

use bevy::prelude::*; // ECS轻量标记。
use bevy::render::extract_component::ExtractComponent; // 正式提取生命周期包含标记移除。

#[derive(Component, Clone, Copy, Default, ExtractComponent)] // 不附带新相机或历史缓冲。
#[extract_app(bevy::render::RenderApp)]
pub(in crate::shared::large_scene::water::reflection) struct WaterReflectionView; // 唯一活动透视SSR视图。
