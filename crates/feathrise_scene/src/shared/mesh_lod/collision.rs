//! 单点替换既有静态三角碰撞；安全近区、接触与未知焦点始终保留原始形状。
use bevy::prelude::*;
mod build;
mod components;
mod resources;
mod systems;
mod types;

pub(super) fn coarse_empty(world: &World) -> bool {
    world.resource::<resources::Runtime>().coarse.is_empty()
}

pub(super) use systems::restore_unsafe;

pub(super) fn install(app: &mut App) {
    app.init_resource::<resources::Runtime>()
        .add_observer(systems::collider_inserted)
        .add_observer(systems::body_inserted)
        .add_observer(systems::source_inserted::<ChildOf>)
        .add_observer(systems::source_inserted::<crate::ScenePlacementInstance>)
        .add_observer(systems::source_inserted::<crate::TerrainObjectInstance>)
        .add_observer(systems::source_inserted::<crate::SubsceneActive>)
        .add_observer(systems::source_inserted::<avian3d::prelude::ColliderOf>)
        .add_observer(systems::source_inserted::<avian3d::prelude::RigidBody>)
        .add_systems(PreUpdate, (systems::restore_unsafe, systems::drive).chain())
        .add_systems(FixedPreUpdate, systems::restore_unsafe)
        .add_systems(
            PostUpdate,
            systems::restore_unsafe.after(bevy::transform::TransformSystems::Propagate),
        );
}

#[cfg(test)]
use crate::shared::placement_data::StaticTerrainShape;
#[cfg(test)]
use avian3d::{parry::shape::SharedShape, prelude::*};
#[cfg(test)]
use bevy::mesh::VertexAttributeValues;
#[cfg(test)]
use {
    build::{cook, source_mesh},
    resources::Runtime,
    types::key,
};
#[cfg(test)]
mod tests;
