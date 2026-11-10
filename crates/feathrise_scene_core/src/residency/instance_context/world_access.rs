//! exclusive清理与实体事件显式借用上下文，禁止迟到事件默认写入父场景。

use bevy::{ecs::component::Mutable, prelude::*};
use super::{InstanceResourceTable, SceneRuntimeScope};

pub fn instance_resource<T: Resource>(world: &World) -> &T {
    let instance = world.get_resource::<SceneRuntimeScope>().and_then(|scope| scope.instance);
    if let Some(instance) = instance {
        world.resource::<InstanceResourceTable<T>>().values.get(&instance).expect("子场景资源未初始化")
    } else { world.resource::<T>() }
}

pub fn instance_resource_mut<T: Resource<Mutability = Mutable>>(world: &mut World) -> Mut<'_, T> {
    let instance = world.get_resource::<SceneRuntimeScope>().and_then(|scope| scope.instance);
    if let Some(instance) = instance {
        world.resource_mut::<InstanceResourceTable<T>>().map_unchanged(|table|
            table.values.get_mut(&instance).expect("子场景资源未初始化"))
    } else { world.resource_mut::<T>() }
}
