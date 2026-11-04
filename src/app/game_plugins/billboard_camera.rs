//! 为现有玩法/工程主相机声明通用文字朝向；无需安装原游戏角色头顶界面。

use bevy::prelude::*;
use feathrise_billboard_text::BillboardCamera;
use feathrise_scene_core::view::PrimarySceneView;

pub(super) fn attach(
    mut commands: Commands,
    cameras: Query<Entity, (With<PrimarySceneView>, Without<BillboardCamera>)>,
) {
    for camera in &cameras {
        commands.entity(camera).insert(BillboardCamera);
    }
}
