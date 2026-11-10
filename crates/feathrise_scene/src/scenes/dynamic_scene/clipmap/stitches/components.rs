//! 完整边的LOD发布身份与编辑状态组件。
use bevy::prelude::*;
#[derive(Component, Clone, Copy, PartialEq)]
pub(in crate::scenes::dynamic_scene) struct StitchLodStamp(pub Vec4);
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::scenes::dynamic_scene) struct StitchPublish;
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(in crate::scenes::dynamic_scene) struct StitchStatus {
    pub assigned: u8,
    pub active: u8,
}
