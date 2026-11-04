//! 水面分块的有限主帧回收与大型纯CPU完成值的单任务退役。
use super::{DynamicWaterCpuInteraction, DynamicWaterMaterial, DynamicWaterSurface};
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use std::collections::VecDeque;

pub(super) struct RetiredSurface {
    pub root: Option<Entity>,
    pub mesh: Handle<Mesh>,
    pub material: Handle<DynamicWaterMaterial>,
    pub children: Vec<Entity>,
    pub meshes: Vec<Handle<Mesh>>,
}
#[derive(Resource, Default)]
struct Garbage {
    hidden_root: Option<Entity>,
    surfaces: VecDeque<RetiredSurface>,
    cpu: VecDeque<Box<dyn Send + Sync>>,
    task: Option<Task<()>>,
}

/// 移交完成值的所有权，不能在主帧遍历大数组后才启动退役。
pub fn retire_water_cpu<T: Send + Sync + 'static>(commands: &mut Commands, value: T) {
    commands.queue(move |world: &mut World| {
        retire_water_cpu_in_world(world, value);
    });
}

pub(crate) fn retire_water_cpu_in_world<T: Send + Sync + 'static>(world: &mut World, value: T) {
    world
        .get_resource_or_init::<Garbage>()
        .cpu
        .push_back(Box::new(value));
}

pub(super) fn retire_surface(commands: &mut Commands, surface: RetiredSurface) {
    commands.queue(move |world: &mut World| {
        let garbage_root = world.get_resource_or_init::<Garbage>().hidden_root;
        let garbage_root = garbage_root
            .filter(|&root| world.get_entity(root).is_ok())
            .unwrap_or_else(|| {
                let root = world
                    .spawn((
                        Name::new("水体后台退役"),
                        Transform::IDENTITY,
                        Visibility::Hidden,
                    ))
                    .id();
                world.get_resource_or_init::<Garbage>().hidden_root = Some(root);
                root
            });
        let mut interaction = None;
        if let Some(root) = surface.root {
            if let Ok(mut entity) = world.get_entity_mut(root) {
                // 旧编辑器显隐回执也不能把已退役的子块重新显示出来。
                entity
                    .insert((Visibility::Hidden, ChildOf(garbage_root)))
                    .remove::<(DynamicWaterCpuInteraction, super::PaintedWaterSurface)>();
                interaction = entity.take::<DynamicWaterSurface>(); // 足迹最后一个Arc也在worker释放。
            }
        }
        let mut garbage = world.get_resource_or_init::<Garbage>();
        if let Some(interaction) = interaction {
            garbage.cpu.push_back(Box::new(interaction));
        }
        garbage.surfaces.push_back(surface);
    });
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<Garbage>().add_systems(First, reap);
}
fn reap(
    mut commands: Commands,
    mut garbage: ResMut<Garbage>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<DynamicWaterMaterial>>>,
) {
    if garbage.task.as_mut().and_then(check_ready).is_some() {
        garbage.task = None;
    }
    if garbage.task.is_none() {
        if let Some(value) = garbage.cpu.pop_front() {
            garbage.task = Some(AsyncComputeTaskPool::get().spawn(async move { drop(value) }));
        }
    }
    let (Some(mut meshes), Some(mut materials)) = (meshes, materials) else {
        return;
    };
    let Some(surface) = garbage.surfaces.front_mut() else {
        return;
    };
    if let Some(mesh) = surface.meshes.pop() {
        if let Some(entity) = surface.children.pop() {
            commands.entity(entity).try_despawn();
        }
        meshes.remove(mesh.id()); // 每帧只释放一个上传块，根在全部子块之后回收。
    } else {
        if let Some(root) = surface.root {
            commands.entity(root).try_despawn();
        }
        meshes.remove(surface.mesh.id());
        materials.remove(surface.material.id());
        garbage.surfaces.pop_front();
    }
}
