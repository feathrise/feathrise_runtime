//! 完整边的LOD发布身份与编辑状态组件。
use bevy::prelude::*;
#[derive(Component, Clone, Copy, PartialEq)]
pub(in crate::maps::dynamic_map) struct StitchLodStamp(pub Vec4);
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::maps::dynamic_map) struct StitchPublish;
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(in crate::maps::dynamic_map) struct StitchStatus {
    pub assigned: u8,
    pub active: u8,
}
