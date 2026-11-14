//! 分页作者源的验收身份，实例热换代和不可变源版本必须同时匹配。

use bevy::prelude::Entity;
use feathrise_scene_content::{SceneId, TerrainId};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Identity {
    pub scene: SceneId,
    pub terrain: TerrainId,
    pub instance: Entity,
    pub path: String,
}
