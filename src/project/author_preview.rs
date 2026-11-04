//! 工程自有作者预览的通用合同；领域解析与真实技能/角色执行由工程插件拥有。
use bevy::prelude::*;
mod service;
#[cfg(feature = "validation")]
pub mod validation;
pub mod wire;
pub(crate) use service::{requested, run};

/// 预览独占层，不与地图、玩家及作者手柄共享可见性。
pub const AUTHOR_RUNTIME_PREVIEW_LAYER: usize = 30;

#[derive(Resource, Clone, Debug)]
pub struct AuthorRuntimePreview {
    pub revision: u64,
    pub kind: String,
    pub payload: serde_json::Value,
}

/// 只有同一请求的完整根才可发布；SDK相机只读取此根，不解释游戏结构。
#[derive(Resource, Clone, Debug)]
pub struct AuthorRuntimePreviewScene {
    pub revision: u64,
    pub kind: String,
    pub root: Entity,
}

#[derive(Resource, Default, Clone, Debug)]
pub struct AuthorRuntimePreviewStatus {
    pub revision: u64,
    pub kind: String,
    pub ready: bool,
    pub error: Option<String>,
}

/// 当前修订的真实执行证据；只允许4KiB内的小对象，不承载场景或领域目录。
#[derive(Resource, Default, Clone, Debug)]
pub struct AuthorRuntimePreviewEvidence {
    pub revision: u64,
    pub kind: String,
    pub data: serde_json::Value,
}
