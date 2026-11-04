//! FSR 时间历史的相机不连续检测。

use crate::types::{Fsr3CameraHistoryPose, Fsr3Upscaling}; // 公开 reset 与私有上一帧姿态。
use bevy::{camera::RenderTarget, prelude::*}; // 窗口相机过滤、投影和全局变换。

const HISTORY_TELEPORT_DISTANCE: f32 = 12.0; // 与项目 TAA 传送历史判定保持一致。
const HISTORY_CUT_ANGLE: f32 = std::f32::consts::FRAC_PI_4; // 四十五度单帧旋转视为切镜。

/// 相机发生同实体传送、大角度切镜或投影变化时只请求一次历史重置。
#[allow(clippy::type_complexity)] // 姿态、投影、请求和私有历史必须在一个原子查询中比较。
pub(crate) fn reset_fsr3_history_on_camera_discontinuity(
    mut commands: Commands, // 首帧建立轻量姿态，关闭后清理私有历史。
    mut cameras: Query<
        (
            Entity,
            &RenderTarget,
            &GlobalTransform,
            Ref<Projection>,
            &mut Fsr3Upscaling,
            Option<&mut Fsr3CameraHistoryPose>,
        ),
        With<Camera3d>,
    >, // FSR 组件已要求全部渲染附件。
    stale_history: Query<Entity, (With<Fsr3CameraHistoryPose>, Without<Fsr3Upscaling>)>,
) {
    for entity in &stale_history {
        commands.entity(entity).remove::<Fsr3CameraHistoryPose>(); // 关闭后不保留旧会话姿态。
    }
    for (entity, target, transform, projection, mut fsr, history) in &mut cameras {
        if !matches!(target, RenderTarget::Window(_)) {
            continue; // 纹理目标不属于全局 FSR 候选。
        }
        let (_, rotation, translation) = transform.to_scale_rotation_translation(); // 读取传播后的最终镜头姿态。
        let current = Fsr3CameraHistoryPose {
            translation,
            rotation,
        }; // 固定 O(1) 相机快照。
        let Some(mut history) = history else {
            commands.entity(entity).insert(current); // 新会话由组件默认 reset 清空空历史。
            continue;
        };
        let teleported =
            history.translation.distance(current.translation) > HISTORY_TELEPORT_DISTANCE; // 大位移历史无效。
        let camera_cut = history.rotation.angle_between(current.rotation) > HISTORY_CUT_ANGLE; // 大角度历史无效。
        let projection_changed = projection.is_changed() && !projection.is_added(); // FOV/投影切换历史无效。
        if (teleported || camera_cut || projection_changed) && !fsr.reset {
            fsr.reset = true; // 只在真实不连续且尚未请求时写一次。
        }
        if *history != current {
            *history = current; // 稳定姿态帧不写组件。
        }
    }
}
