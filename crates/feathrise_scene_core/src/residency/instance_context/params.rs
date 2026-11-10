//! 系统参数只借用目标实例的值；不会把某个子图临时塞回父图单一Resource。

use bevy::{ecs::{component::Mutable, system::SystemParam}, prelude::*};
use std::ops::{Deref, DerefMut};
use super::{InstanceResourceTable, SceneRuntimeScope};

#[derive(SystemParam)]
pub struct InstanceRes<'w, T: Resource> {
    root: Res<'w, T>,
    scope: Option<Res<'w, SceneRuntimeScope>>,
    instances: Option<Res<'w, InstanceResourceTable<T>>>,
}

impl<T: Resource> InstanceRes<'_, T> {
    pub fn for_instance(&self, instance: Option<Entity>) -> Option<&T> {
        if let Some(instance) = instance { self.instances.as_deref()?.values.get(&instance) }
        else { Some(&self.root) }
    }
    pub fn is_changed(&self) -> bool {
        if self.scope.as_deref().is_some_and(|scope| scope.instance.is_some()) {
            self.instances.as_ref().is_some_and(|table| table.is_changed())
        } else { self.root.is_changed() }
    }
}

impl<T: Resource> Deref for InstanceRes<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        if let Some(instance) = self.scope.as_deref().and_then(|scope| scope.instance) {
            self.instances.as_deref().and_then(|table| table.values.get(&instance))
                .expect("执行子场景域系统前必须完整初始化上下文")
        } else { &self.root }
    }
}

#[derive(SystemParam)]
pub struct InstanceResMut<'w, T: Resource<Mutability = Mutable>> {
    root: ResMut<'w, T>,
    scope: Option<Res<'w, SceneRuntimeScope>>,
    instances: Option<ResMut<'w, InstanceResourceTable<T>>>,
}

impl<T: Resource<Mutability = Mutable>> Deref for InstanceResMut<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        if let Some(instance) = self.scope.as_deref().and_then(|scope| scope.instance) {
            self.instances.as_deref().and_then(|table| table.values.get(&instance))
                .expect("执行子场景域系统前必须完整初始化上下文")
        } else { &self.root }
    }
}

impl<T: Resource<Mutability = Mutable>> DerefMut for InstanceResMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        if let Some(instance) = self.scope.as_deref().and_then(|scope| scope.instance) {
            self.instances.as_deref_mut().and_then(|table| table.values.get_mut(&instance))
                .expect("执行子场景域系统前必须完整初始化上下文")
        } else { &mut self.root }
    }
}

impl<T: Resource<Mutability = Mutable>> InstanceResMut<'_, T> {
    /// GLB/碰撞完成observer按事件实体来源路由；此时当前调度通常已经回到父图。
    pub fn for_instance_mut(&mut self, instance: Option<Entity>) -> Option<&mut T> {
        if let Some(instance) = instance {
            self.instances.as_deref_mut()?.values.get_mut(&instance)
        } else { Some(&mut self.root) }
    }
}
