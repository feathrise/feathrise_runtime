//! 子场景根的公开实例身份与整批候选隔离；来源ID始终保持原值。

use bevy::prelude::*;
use feathrise_scene_content::{SceneId, SceneInstancePath};

#[derive(Component, Clone, Debug)]
pub struct SubsceneInstance {
    pub path: SceneInstancePath,
    pub source_scene_id: SceneId,
    pub generation: u64,
    pub batch: Entity,
}

#[derive(Component)]
pub(super) struct SubsceneBatch;

/// 只有已原子采用的根才能参与游戏与编辑器命中，候选根不提供选择和承托信用。
#[derive(Component)]
pub struct SubsceneActive;
