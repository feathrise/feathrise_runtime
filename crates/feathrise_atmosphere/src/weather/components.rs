// 天气 ECS 组件：声明体积雾运行时所有权和唯一有限雾体积身份。

use bevy::{light::VolumetricFog, prelude::*}; // 体积雾作者状态与组件派生来自 Bevy。

/// 标记允许天气系统主动接管的作者预览相机。
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct WeatherPreviewCamera; // F12显式安装该标记；普通菜单、小地图和角色预览仍不会收到玩法天气。

/// 保存玩法相机在天气体积雾首次接管前的作者组件。
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct AuthoredGameplayVolumetricFog {
    pub(crate) value: Option<VolumetricFog>, // None 表示作者原本没有启用体积雾渲染。
}

/// 保存天体方向光在天气体积光首次接管前是否具备光束资格。
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct AuthoredCelestialVolumetricLight {
    pub(crate) present: bool, // 关闭全局档位时按该值恢复作者的 VolumetricLight 组件。
}

/// 标记天气系统维护的唯一相机局部有限雾体积。
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct WeatherVolumetricFogVolume; // 开启档才存在，关闭或离开玩法场景时立即清理。
