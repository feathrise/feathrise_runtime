//! 静态碰撞LOD的唯一计算/退役席位、引用计数和32项物理复核信用。
use super::types::*;
use avian3d::parry::shape::Shape;
use bevy::{prelude::*, tasks::Task};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Weak,
};

#[derive(Resource, Default)]
pub(super) struct Runtime {
    pub pending: VecDeque<Entity>,
    pub queued: HashSet<Entity>,
    pub targets: HashMap<Entity, Target>,
    pub cursor: VecDeque<Entity>,
    pub discovery: VecDeque<(Entity, usize)>,
    pub references: HashMap<Key, usize>,
    pub cache: HashMap<Key, Cache>,
    pub cache_cursor: VecDeque<Key>,
    /// 派生形状在活动缓存和后台析构期间都占用同一份信用。
    pub cache_bytes: usize,
    pub failed: HashMap<Key, Failure>,
    pub task: Option<(Key, Task<(Weak<dyn Shape>, Result<Built, String>)>)>,
    pub completed: Option<(Key, Weak<dyn Shape>, Result<Built, String>)>,
    pub release: Option<Task<()>>,
    pub release_credit: usize,
    pub retired: VecDeque<Target>,
    pub coarse: HashSet<Entity>,
    pub credit_revision: u64,
}
