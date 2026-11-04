//! DLSS帧生成公共常量与相机历史数学；与GPU标签和SDK状态事务分离。

use super::{CameraHistory, ProjectionParameters}; // 复用渲染域历史和作者投影快照。
use crate::native::NativeConstants; // 唯一ABI常量类型。
use bevy::{
    prelude::*,
    render::{render_resource::Extent3d, view::ExtractedView},
}; // 矩阵、向量和视图尺寸。

pub(super) fn build_constants(
    view: &ExtractedView,             // 当前无抖动投影与世界姿态。
    projection: ProjectionParameters, // 作者近远裁面与FOV。
    output_size: Extent3d,            // 最终输出尺寸。
    history: CameraHistory,           // 上一成功标签帧。
    jitter: Vec2,                     // 当前实际深度采样像素偏移，与DLSS SR约定一致。
) -> NativeConstants {
    let camera_view_to_clip = view.clip_from_view; // ExtractedView投影尚未应用TemporalJitter。
    let clip_to_camera_view = camera_view_to_clip.inverse(); // 透视逆矩阵。
    let world_from_view = view.world_from_view.to_matrix(); // 当前世界姿态。
    let world_from_clip = world_from_view * clip_to_camera_view; // 当前裁剪到世界。
    let position = view.world_from_view.translation(); // 当前世界相机位置。
    let output = UVec2::new(output_size.width, output_size.height); // 用于尺寸断续判断。
    let projection_changed =
        history.valid && matrix_difference(history.projection, camera_view_to_clip) > 0.001; // FOV或投影换代。
    let teleported = history.valid && history.position.distance(position) > 10.0; // 大位移不允许跨帧插值。
    let reset = !history.valid || history.output_size != output || projection_changed || teleported; // 所有断续统一清历史。
    let clip_to_prev_clip = if reset {
        Mat4::IDENTITY // reset帧不引用未知上一视图。
    } else {
        history.clip_from_world * world_from_clip // 当前裁剪→世界→上一裁剪。
    };
    let prev_clip_to_clip = clip_to_prev_clip.inverse(); // SDK要求同时提供逆变换。
    let transform = view.world_from_view; // GlobalTransform提供正交世界基向量。
    NativeConstants {
        camera_view_to_clip: to_streamline_matrix(camera_view_to_clip), // 列向量变换转换为SDK行向量变换。
        clip_to_camera_view: to_streamline_matrix(clip_to_camera_view), // 逆投影使用相同数学约定。
        clip_to_prev_clip: to_streamline_matrix(clip_to_prev_clip), // 保留当前裁剪到上一裁剪的变换语义。
        prev_clip_to_clip: to_streamline_matrix(prev_clip_to_clip), // 逆跨帧变换也转换为行向量矩阵。
        jitter_offset: jitter.to_array(), // 独立报告深度采样抖动；矩阵和运动向量仍不含抖动。
        mvec_scale: [-1.0, -1.0], // Bevy为当前减上一UV，NGX需要反向位移；与SR的负内部尺寸缩放保持一致。
        camera_pos: position.to_array(),
        camera_up: transform.up().as_vec3().to_array(),
        camera_right: transform.right().as_vec3().to_array(),
        camera_forward: transform.forward().as_vec3().to_array(),
        camera_near: projection.near,
        camera_far: projection.far,
        camera_fov: projection.fov,
        camera_aspect_ratio: projection.aspect_ratio,
        reset: u8::from(reset),
        _padding: [0; 3],
    } // 完整公共常量返回。
} // build_constants结束。

pub(super) fn next_history(view: &ExtractedView, output_size: Extent3d) -> CameraHistory {
    // 成功帧后推进历史。
    let projection = view.clip_from_view; // 无抖动投影。
    let world_from_view = view.world_from_view.to_matrix(); // 当前相机世界姿态。
    CameraHistory {
        clip_from_world: projection * world_from_view.inverse(), // 下帧当前到上一变换来源。
        position: view.world_from_view.translation(),            // 瞬移检测来源。
        projection,                                              // FOV和裁面变化检测来源。
        output_size: UVec2::new(output_size.width, output_size.height), // resize检测来源。
        valid: true,                                             // 只有完整SDK标签成功才有效。
    }
} // next_history结束。

pub(super) fn to_streamline_matrix(matrix: Mat4) -> [f32; 16] {
    // 同时转换向量乘法和存储约定。
    // Streamline使用行向量乘矩阵；Glam使用矩阵乘列向量。数学转置后按行存储，字节恰好等于原列数组。
    matrix.to_cols_array() // 不能再transpose，否则仅改存储布局却改变了投影、平移和跨帧变换。
} // to_streamline_matrix结束。

fn matrix_difference(left: Mat4, right: Mat4) -> f32 {
    // 固定十六元素最大绝对差，无分配。
    left.to_cols_array()
        .into_iter()
        .zip(right.to_cols_array())
        .map(|(left, right)| (left - right).abs())
        .fold(0.0, f32::max)
} // matrix_difference结束。

pub(super) fn constants_are_finite(constants: &NativeConstants) -> bool {
    // SDK拒绝NaN/Infinity前由宿主快速阻断。
    constants
        .camera_view_to_clip
        .iter()
        .all(|value| value.is_finite())
        && constants
            .clip_to_camera_view
            .iter()
            .all(|value| value.is_finite())
        && constants
            .clip_to_prev_clip
            .iter()
            .all(|value| value.is_finite())
        && constants
            .prev_clip_to_clip
            .iter()
            .all(|value| value.is_finite())
        && constants.camera_pos.iter().all(|value| value.is_finite())
        && constants.camera_up.iter().all(|value| value.is_finite())
        && constants.camera_right.iter().all(|value| value.is_finite())
        && constants
            .camera_forward
            .iter()
            .all(|value| value.is_finite())
        && constants.jitter_offset.iter().all(|value| value.is_finite()) // 非法采样偏移不能交给SDK。
        && constants.camera_near.is_finite()
        && constants.camera_far.is_finite()
        && constants.camera_fov.is_finite()
        && constants.camera_aspect_ratio.is_finite() // 全部必需值必须有限。
} // constants_are_finite结束。
