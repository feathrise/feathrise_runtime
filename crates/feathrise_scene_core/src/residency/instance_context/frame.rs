//! 相机、玩家输入使用世界坐标；地图域只接收自己源坐标系中的观察位置。

use bevy::{ecs::system::SystemParam, prelude::*};
use super::{MapRuntimeScope, MapSourceFrame};

#[derive(SystemParam)]
pub struct InstanceFrame<'w> { scope: Option<Res<'w, MapRuntimeScope>> }

impl InstanceFrame<'_> {
    pub fn frame(&self) -> MapSourceFrame { self.scope.as_deref().map_or_else(MapSourceFrame::default, |scope| scope.frame) }
    pub fn to_local(&self, world: Vec3) -> Vec3 { self.frame().to_local(world) }
    pub fn to_world(&self, local: Vec3) -> Vec3 { self.frame().to_world(local) }
    pub fn instance(&self) -> Option<Entity> { self.scope.as_deref().and_then(|scope| scope.instance) }
}
