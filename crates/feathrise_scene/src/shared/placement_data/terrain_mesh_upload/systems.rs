//! 每帧只移交一个固定大小网格；注册数量影响完成时间，不拒绝大地形。
use super::{components::TerrainMeshUpload, resources::UploadGarbage};
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, futures::check_ready},
};

pub(super) fn upload(
    mut commands: Commands,
    mut pending: Query<(Entity, &mut TerrainMeshUpload)>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    mut garbage: ResMut<UploadGarbage>,
) {
    if garbage.task.as_mut().and_then(check_ready).is_some() {
        garbage.task = None;
    }
    if garbage.task.is_none() && !garbage.pending.is_empty() {
        let retired = std::mem::take(&mut garbage.pending);
        garbage.task = Some(AsyncComputeTaskPool::get().spawn(async move { drop(retired) }));
    }
    let Some(mut meshes) = meshes else { return }; // 纯数据/无渲染宿主仍可推进后台退役。
    let mut uploaded = false;
    for (root, mut candidate) in &mut pending {
        if candidate.meshes.is_empty() {
            // 最后一块登记后再等一帧，使子实体GlobalTransform传播完成才允许捕获。
            commands.entity(root).remove::<TerrainMeshUpload>();
        } else if !uploaded {
            let mesh = candidate.meshes.pop_front().unwrap();
            commands.entity(root).with_child((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(candidate.material.clone()),
                Transform::default(),
                Visibility::Inherited, // 根仍隐藏，局部上传不能提前显示或安装物理。
            ));
            uploaded = true; // 全域一块，多个候选也不能放大本帧上传量。
        }
    }
}
