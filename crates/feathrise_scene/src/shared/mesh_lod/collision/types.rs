//! 碰撞LOD的不可变形状身份与派生结果；弱租约保护分配地址而不强留原始BVH。
use avian3d::{parry::shape::Shape, prelude::Collider};
use bevy::prelude::*;
use std::sync::{Arc, Weak};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Key {
    pub shape: usize,
    pub scale: [u32; 3],
}
pub(super) fn key(collider: &Collider) -> Key {
    Key {
        shape: Arc::as_ptr(&collider.shape().0) as *const () as usize,
        scale: collider.scale().to_array().map(f32::to_bits),
    }
}
pub(super) struct Level {
    pub collider: Collider,
    pub error: f32,
}
pub(super) struct Cache {
    pub levels: Arc<[Level]>,
    pub bytes: usize,
    pub pins: usize,
    pub _source: Weak<dyn Shape>,
}
pub(super) struct Target {
    pub owner: Entity,
    pub original: Collider,
    pub key: Key,
    pub applied: usize,
    pub rescale_guard: bool,
}
pub(super) struct Failure {
    pub _source: Weak<dyn Shape>,
    pub retry_credit: Option<u64>,
}
pub(super) struct Built {
    pub levels: Arc<[Level]>,
    pub bytes: usize,
    pub source: Weak<dyn Shape>,
}
pub(super) struct Focus {
    pub center: Vec3,
    pub radius: f32,
    pub travel: f32,
}
