//! 只有明确动态刚体来源进入物理焦点集合，静态地图根不参与逐帧扫描。
use bevy::prelude::*;

#[derive(Component)]
pub(super) struct DynamicFocus;
