//! 动态GPU叶片窗口的稳定帧跟随。

use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use bevy::prelude::*;
use crate::SceneRuntimeObserver;
use feathrise_environment::vegetation::blade::{GpuBladePatch, GpuBladeView};

use super::components::DynamicBladeWindow;

type DynamicBladeWindowData = (
    &'static DynamicBladeWindow,
    &'static mut GpuBladePatch,
    &'static mut Transform,
);

/// 稳定帧只更新当前场景实际登记的四档叶片窗参数，不扫描密度像素或生成逐株数据。
pub(in crate::scenes::dynamic_scene) fn follow_dynamic_blade_windows(
    frame: feathrise_scene_core::residency::InstanceFrame,
    players: Query<&Transform, (With<SceneRuntimeObserver>, Without<DynamicBladeWindow>)>,
    cameras: Query<
        &Transform,
        (
            With<GpuBladeView>,
            Without<SceneRuntimeObserver>,
            Without<DynamicBladeWindow>,
        ),
    >,
    mut windows: InstanceQuery<DynamicBladeWindowData, (Without<SceneRuntimeObserver>, Without<GpuBladeView>)>,
) {
    let Ok(player) = players.single() else {
        return;
    };
    let player_position = frame.to_local(player.translation);
    let residency = cameras
        .single()
        .map(|camera| frame.to_local(camera.translation))
        .unwrap_or(player_position);
    let focus = Vec3::new(residency.x, player_position.y, residency.z);
    for (window, mut patch, mut transform) in &mut windows {
        follow_window(window, &mut patch, &mut transform, focus, residency);
    }
}

fn follow_window(window: &DynamicBladeWindow, patch: &mut GpuBladePatch,
    transform: &mut Transform, focus: Vec3, residency: Vec3) {
    patch.set_focus(focus);
    let snapped = patch.snapped_origin_xz(residency.xz(), window.snap_cells);
    let current = patch.origin();
    if current.x == snapped.x && current.z == snapped.y { return; }
    let origin = Vec3::new(snapped.x, current.y, snapped.y);
    patch.set_origin(origin);
    transform.translation = origin; // GPU候选中心与Bevy的草窗AABB必须在同一传播帧一起移动。
}

/// 镜头跟随不属于准备工作：直接查询已有子图窗口及其当前源框架，不轮询重建任务或扫描密度。
pub(in crate::scenes::dynamic_scene) fn follow_subscene_blade_windows(
    players: Query<&Transform, (With<SceneRuntimeObserver>, Without<DynamicBladeWindow>)>,
    cameras: Query<&Transform, (With<GpuBladeView>, Without<SceneRuntimeObserver>, Without<DynamicBladeWindow>)>,
    frames: Query<&feathrise_scene_core::residency::SceneSourceFrame>,
    mut windows: Query<(&DynamicBladeWindow, &mut GpuBladePatch, &mut Transform, &feathrise_scene_core::residency::SceneInstanceOwner),
        (Without<SceneRuntimeObserver>, Without<GpuBladeView>)>,
) {
    let Ok(player) = players.single() else { return; };
    let camera = cameras.single().map(|camera| camera.translation).unwrap_or(player.translation);
    for (window, mut patch, mut transform, owner) in &mut windows {
        let Ok(frame) = frames.get(owner.0) else { continue; };
        let player_position = frame.to_local(player.translation);
        let residency = frame.to_local(camera);
        let focus = Vec3::new(residency.x, player_position.y, residency.z);
        follow_window(window, &mut patch, &mut transform, focus, residency);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use feathrise_scene_core::residency::{SceneInstanceOwner, SceneSourceFrame};

    #[test]
    fn translated_and_rotated_sources_follow_every_camera_frame_with_matching_gpu_bounds() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TransformPlugin))
            .add_systems(PostUpdate, follow_subscene_blade_windows.before(bevy::transform::TransformSystems::Propagate));
        let player = app.world_mut().spawn((SceneRuntimeObserver, Transform::from_xyz(0.0, 2.0, 0.0))).id();
        let camera = app.world_mut().spawn((GpuBladeView::default(), Transform::IDENTITY)).id();
        let poses = [Transform::from_xyz(-54.724903, 28.95987, 0.0),
            Transform::from_xyz(200.0, 15.0, -70.0).with_rotation(Quat::from_rotation_y(1.2))];
        let windows: Vec<_> = poses.into_iter().map(|pose| {
            let frame = SceneSourceFrame::from_transform(pose);
            let source = app.world_mut().spawn((pose, frame)).id();
            let origin = Vec3::new(0.0, -80.0, 0.0);
            app.world_mut().spawn((DynamicBladeWindow { snap_cells: 16 },
                GpuBladePatch::square(origin, 112.0, 129, 7), Transform::from_translation(origin),
                SceneInstanceOwner(source), ChildOf(source))).id()
        }).collect(); // 同源实例使用各自坐标框架，窗口跟随不得混用另一实例的位置。
        let parent_patch = GpuBladePatch::square(Vec3::ZERO, 112.0, 129, 7);
        let parent = app.world_mut().spawn((DynamicBladeWindow { snap_cells: 16 }, parent_patch, Transform::IDENTITY)).id();
        for position in [Vec3::new(-30.0, 60.0, 35.0), Vec3::new(-90.0, 45.0, -25.0), Vec3::new(40.0, 90.0, 75.0)] {
            app.world_mut().get_mut::<Transform>(camera).unwrap().translation = position;
            app.update(); // 没有来源修订或准备任务，连续镜头运动仍须更新全部既有窗。
            for window in &windows {
                let world = app.world();
                let frame = world.get::<SceneSourceFrame>(world.get::<SceneInstanceOwner>(*window).unwrap().0).unwrap();
                let patch = *world.get::<GpuBladePatch>(*window).unwrap();
                let local_camera = frame.to_local(position);
                let local_player = frame.to_local(world.get::<Transform>(player).unwrap().translation);
                assert!(patch.focus().abs_diff_eq(Vec3::new(local_camera.x, local_player.y, local_camera.z), 0.0001));
                assert_eq!(patch.origin().xz(), patch.snapped_origin_xz(local_camera.xz(), 16));
                let global = world.get::<GlobalTransform>(*window).unwrap();
                assert!(global.translation().abs_diff_eq(frame.to_world(patch.origin()), 0.0001));
            }
            assert_eq!(*app.world().get::<GpuBladePatch>(parent).unwrap(), parent_patch); // 父场景仍由原跟随系统管理。
        }
    }
}
