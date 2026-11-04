//! 隐藏候选仍待登记的网格所有权；取消时把大CPU网格移交后台释放。
use bevy::prelude::*;
use std::collections::VecDeque;

#[derive(Component)]
#[component(on_remove = retire)]
pub struct TerrainMeshUpload {
    pub(super) meshes: VecDeque<Mesh>,
    pub(super) material: Handle<StandardMaterial>,
    total: usize,
}
impl TerrainMeshUpload {
    pub fn new(meshes: Vec<Mesh>, material: Handle<StandardMaterial>) -> Self {
        Self {
            total: meshes.len(),
            meshes: meshes.into(),
            material,
        }
    }
    pub fn progress(&self) -> (usize, usize) {
        (self.total - self.meshes.len(), self.total)
    }
}
fn retire(mut world: bevy::ecs::world::DeferredWorld, context: bevy::ecs::lifecycle::HookContext) {
    let retired = world
        .get_mut::<TerrainMeshUpload>(context.entity)
        .map(|mut upload| std::mem::take(&mut upload.meshes));
    if let Some(retired) = retired.filter(|m| !m.is_empty())
        && let Some(mut garbage) = world.get_resource_mut::<super::resources::UploadGarbage>()
    {
        garbage.pending.push(retired);
    }
}
