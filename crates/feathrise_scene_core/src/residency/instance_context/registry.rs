//! 上下文资源注册与完整创建；父地图驻留时整套子图状态随父快照一起保存。

use bevy::prelude::*;
use std::{any::{Any, TypeId}, collections::HashSet};
use super::{InstanceResourceTable, MapRuntimeScope};
use crate::residency::SceneResidencyAppExt;

type OwnedInstanceResource = Box<dyn Any + Send + Sync>;

#[derive(Clone, Copy)]
struct ContextSlot {
    initialize: fn(&mut World, Entity),
    retire: fn(&mut World, Entity) -> Option<OwnedInstanceResource>,
}

#[derive(Resource, Default)]
struct ContextRegistry { ids: HashSet<TypeId>, slots: Vec<ContextSlot> }

fn initialize<T: Resource + FromWorld>(world: &mut World, instance: Entity) {
    if !world.resource::<InstanceResourceTable<T>>().values.contains_key(&instance) {
        let value = T::from_world(world);
        world.resource_mut::<InstanceResourceTable<T>>().values.insert(instance, value);
    }
}
fn retire<T: Resource>(world: &mut World, instance: Entity) -> Option<OwnedInstanceResource> {
    world.resource_mut::<InstanceResourceTable<T>>().values.remove(&instance)
        .map(|value| Box::new(value) as OwnedInstanceResource)
}

pub trait MapInstanceContextAppExt {
    fn init_instance_resource<T: Resource + Default>(&mut self) -> &mut Self;
    fn register_instance_resource<T: Resource + FromWorld>(&mut self) -> &mut Self;
}

impl MapInstanceContextAppExt for App {
    fn init_instance_resource<T: Resource + Default>(&mut self) -> &mut Self {
        self.init_resident_resource::<T>();
        self.register_instance_resource::<T>()
    }

    fn register_instance_resource<T: Resource + FromWorld>(&mut self) -> &mut Self {
        super::ownership::install(self);
        self.init_resource::<MapRuntimeScope>().init_resource::<ContextRegistry>();
        self.init_resident_resource::<InstanceResourceTable<T>>(); // 父图休眠时移动整份资源表，不克隆或销毁子图。
        let mut registry = self.world_mut().resource_mut::<ContextRegistry>();
        if registry.ids.insert(TypeId::of::<T>()) {
            registry.slots.push(ContextSlot { initialize: initialize::<T>, retire: retire::<T> });
        }
        self
    }
}

/// 类型集合由插件固定；创建一个上下文不会遍历父地图实体或复制源资产。
pub fn initialize_instance_context(world: &mut World, instance: Entity) {
    world.resource_scope(|world, registry: Mut<ContextRegistry>| {
        for slot in &registry.slots { (slot.initialize)(world, instance); }
    });
}

/// 只转移所有权；调用者在有界后台释放大数组，并让在途任务占槽至真实结束。
pub fn retire_instance_context(world: &mut World, instance: Entity) -> Vec<OwnedInstanceResource> {
    world.resource_scope(|world, registry: Mut<ContextRegistry>| {
        registry.slots.iter().filter_map(|slot| (slot.retire)(world, instance)).collect()
    })
}
