//! 模型草窗只更新固定焦点；实际源姿态由原placement层级传播，密度与逐株GPU输出无需重建。
use super::resources::ModelWindow;
use bevy::{prelude::*, transform::helper::TransformHelper};
use feathrise_environment::vegetation::blade::{GpuBladePatch, GpuBladeView};

pub(super) fn follow(
    players: Query<&Transform, With<crate::SceneRuntimeObserver>>,
    cameras: Query<&Transform, (With<GpuBladeView>, Without<crate::SceneRuntimeObserver>)>,
    transforms: TransformHelper,
    mut windows: Query<(&ModelWindow, &mut GpuBladePatch)>,
) {
    let Ok(player) = players.single() else {
        return;
    };
    let camera = cameras
        .single()
        .map(|camera| camera.translation)
        .unwrap_or(player.translation);
    for (window, mut patch) in &mut windows {
        let Ok(frame) = transforms.compute_global_transform(window.source) else {
            continue;
        };
        let inverse = frame.to_matrix().inverse();
        let eye = inverse.transform_point3(camera);
        let player = inverse.transform_point3(player.translation);
        patch.set_focus(Vec3::new(eye.x, player.y, eye.z));
    }
}
