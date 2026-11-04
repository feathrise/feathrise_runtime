//! 水面SSR额外纹理所有权；资格撤销不得删掉标准折射刚替换的纹理。
use bevy::prelude::*; // 仅RenderApp实体持有的组件。
use bevy::render::render_resource::Texture; // GPU句柄用于精确比较资产身份，不回读像素。

#[derive(Component)] // 只标记水面反射自主创建的传输纹理。
pub(super) struct OwnedReflectionTexture {
    // 标准折射组件可以在下一帧覆盖同一实体。
    pub(super) texture: Texture, // 保留实际对象身份，撤销只删除自己当前仍拥有的纹理。
} // 结束所有权组件。

#[derive(Component)] // 本帧透明队列确实有水且颜色源可复制时才授信。
pub(super) struct ReflectionCaptureReady; // 保留GPU缓存与是否本帧复制分离，转向陆地不做全屏copy。
