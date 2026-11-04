//! 应用领域在完整实例采用和退役的同一事务内交接；共享场景不解释领域数据。
use bevy::prelude::*;
use feathrise_map_content::{ExpandedMapInstance, MapInstancePath};

pub(super) fn publish(world: &mut World, entity: Entity, node: &ExpandedMapInstance) {
    if let Some(hooks) = world.get_resource::<crate::SceneApplicationHooks>().copied() {
        (hooks.publish_instance)(world, entity, node);
    }
}
pub(super) fn retire(world: &mut World, entity: Entity, path: &MapInstancePath) -> Option<Box<dyn std::any::Any + Send + Sync>> {
    let hooks = world.get_resource::<crate::SceneApplicationHooks>().copied()?;
    (hooks.retire_instance)(world, entity, path)
}
