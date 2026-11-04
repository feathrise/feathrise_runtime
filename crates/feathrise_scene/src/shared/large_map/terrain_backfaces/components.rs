//! 渲染提取专用材质覆盖；主世界的正式材质句柄与Clipmap租约保持原身份。

use bevy::{asset::UntypedAssetId, prelude::*};

#[derive(Component, Clone, Copy)]
pub(super) struct TerrainBackfacesRenderMaterial {
    pub original: UntypedAssetId,
    pub displayed: UntypedAssetId,
    pub previous: UntypedAssetId,
}

/// 缺省MeshTag与零值在Bevy的MeshInputUniform中等价；只临时借它通知渲染重新提取。
#[derive(Component)]
pub(super) struct TerrainBackfacesTemporaryTag;
