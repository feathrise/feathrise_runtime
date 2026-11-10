//! 类型安全的场景资源暂停与恢复；大对象只移动所有权，公共Assets和服务不进入快照。

use crate::residency::SceneResidencyRuntimeSet;
use bevy::{
    ecs::{schedule::ScheduleLabel, system::ScheduleSystem},
    prelude::*,
};
use std::{
    any::{Any, TypeId},
    collections::{HashMap, HashSet},
};

type OwnedResource = Box<dyn Any + Send + Sync>;

struct ResourceSlot {
    id: TypeId,
    take: fn(&mut World) -> Option<OwnedResource>,
    restore: fn(&mut World, OwnedResource),
}

#[derive(Resource, Default)]
struct ResidentResourceRegistry {
    ids: HashSet<TypeId>,
    slots: Vec<ResourceSlot>,
}

/// 每个值保持其原类型及所有权，Arc、任务、GPU句柄和索引不做深复制。
#[derive(Default)]
pub struct ResidentResourceSnapshot {
    values: HashMap<TypeId, OwnedResource>,
}

pub trait SceneResidencyAppExt {
    /// 默认资源的活动槽始终存在；冷场景取得空基线，热场景取回原值。
    fn init_resident_resource<T: Resource + Default>(&mut self) -> &mut Self;
    /// 可选资源在冷场景保持缺席，例如RTT相机；不能为了暂停而创建伪造占位对象。
    fn register_optional_resident_resource<T: Resource>(&mut self) -> &mut Self;
    fn add_resident_systems<M>(
        &mut self,
        schedule: impl ScheduleLabel,
        systems: impl IntoScheduleConfigs<ScheduleSystem, M>,
    ) -> &mut Self;
}

fn restore<T: Resource>(world: &mut World, value: OwnedResource) {
    world.insert_resource(*value.downcast::<T>().expect("驻留资源槽类型一致"));
}

fn take_optional<T: Resource>(world: &mut World) -> Option<OwnedResource> {
    world
        .remove_resource::<T>()
        .map(|value| Box::new(value) as OwnedResource)
}

fn take_default<T: Resource + Default>(world: &mut World) -> Option<OwnedResource> {
    let result = take_optional::<T>(world);
    world.init_resource::<T>();
    result
}

fn register<T: Resource>(app: &mut App, take: fn(&mut World) -> Option<OwnedResource>) {
    app.init_resource::<ResidentResourceRegistry>();
    let mut registry = app.world_mut().resource_mut::<ResidentResourceRegistry>();
    let id = TypeId::of::<T>();
    if registry.ids.insert(id) {
        registry.slots.push(ResourceSlot {
            id,
            take,
            restore: restore::<T>,
        });
    }
}

impl SceneResidencyAppExt for App {
    fn init_resident_resource<T: Resource + Default>(&mut self) -> &mut Self {
        self.init_resource::<T>();
        register::<T>(self, take_default::<T>);
        self
    }

    fn register_optional_resident_resource<T: Resource>(&mut self) -> &mut Self {
        register::<T>(self, take_optional::<T>);
        self
    }

    fn add_resident_systems<M>(
        &mut self,
        schedule: impl ScheduleLabel,
        systems: impl IntoScheduleConfigs<ScheduleSystem, M>,
    ) -> &mut Self {
        self.add_systems(schedule, systems.in_set(SceneResidencyRuntimeSet))
    }
}

/// 槽数由编译时插件集合限定；只移动固定数量资源，不扫描场景实体、历史或资产。
pub fn take_resident_resources(world: &mut World) -> ResidentResourceSnapshot {
    if !world.contains_resource::<ResidentResourceRegistry>() {
        return ResidentResourceSnapshot::default();
    }
    world.resource_scope(|world, registry: Mut<ResidentResourceRegistry>| {
        let mut values = HashMap::with_capacity(registry.slots.len());
        for slot in &registry.slots {
            if let Some(value) = (slot.take)(world) {
                values.insert(slot.id, value);
            }
        }
        ResidentResourceSnapshot { values }
    })
}

/// 完整资源一起归还活动槽；实体激活前调用，不能发布半份场景。
pub fn restore_resident_resources(world: &mut World, snapshot: &mut ResidentResourceSnapshot) {
    world.resource_scope(|world, registry: Mut<ResidentResourceRegistry>| {
        for slot in &registry.slots {
            drop((slot.take)(world)); // 可选槽的缺席同样属于快照，清除LoadingScene新建的占位门。
            if let Some(value) = snapshot.values.remove(&slot.id) {
                (slot.restore)(world, value);
            }
        }
    });
}
