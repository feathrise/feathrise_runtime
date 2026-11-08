//! DLSS帧生成矩阵语义、输入身份与纹理用途回归。

use super::constants::to_streamline_matrix; // 直接验证生产ABI转换。
use super::*; // 验证ABI关键纯函数。

fn apply_streamline(point: Vec4, matrix: Mat4) -> Vec4 {
    // 独立模拟SDK的行向量乘法，避免测试复用Glam乘法。
    let rows = to_streamline_matrix(matrix); // 每四项为SDK的一行。
    Vec4::from_array(std::array::from_fn(|column| {
        // 每个输出分量是输入行向量与矩阵列的点积。
        (0..4).map(|row| point[row] * rows[row * 4 + column]).sum() // 保留平移及齐次w参与。
    })) // 返回齐次坐标，不提前透视除法。
} // SDK乘法夹具结束。

#[test] // 非对称透视和平移可检出错误的额外转置。
fn streamline_preserves_projected_translated_points() {
    // 验证几何语义而非数组排列。
    let projection = bevy::math::proj::perspective_infinite_reverse(1.1, 16.0 / 9.0, 0.1); // Bevy反向Z透视。
    let world_from_view = Mat4::from_translation(Vec3::new(3.0, 2.0, 7.0)); // 非零相机位置。
    let clip_from_world = projection * world_from_view.inverse(); // Glam当前投影。
    let point = Vec4::new(4.0, 3.0, -5.0, 1.0); // 非轴对称测试点。
    assert!(apply_streamline(point, clip_from_world).abs_diff_eq(clip_from_world * point, 0.00001)); // 两种乘法约定投影相同。
} // 投影测试结束。

#[test] // 跨帧复合矩阵必须保持变换方向与逆变换一致。
fn streamline_preserves_clip_history_and_inverse() {
    // 模拟同时移动和转动的相机。
    let projection = bevy::math::proj::perspective_infinite_reverse(1.1, 16.0 / 9.0, 0.1); // 稳定反向Z投影。
    let current_pose =
        Mat4::from_translation(Vec3::new(3.0, 2.0, 7.0)) * Mat4::from_rotation_y(0.2); // 当前世界姿态。
    let previous_pose =
        Mat4::from_translation(Vec3::new(2.0, 1.0, 6.0)) * Mat4::from_rotation_y(0.1); // 上一世界姿态。
    let current_clip = projection * current_pose.inverse(); // 当前世界到裁剪。
    let previous_clip = projection * previous_pose.inverse(); // 上一世界到裁剪。
    let history = previous_clip * current_clip.inverse(); // 当前裁剪经世界到上一裁剪。
    let point = Vec4::new(4.0, 3.0, -5.0, 1.0); // 非零齐次测试点。
    let current = current_clip * point; // 当前裁剪坐标。
    let previous = apply_streamline(current, history); // SDK跨帧重投影。
    assert!(previous.abs_diff_eq(previous_clip * point, 0.0001)); // 验证复合变换次序。
    assert!(apply_streamline(previous, history.inverse()).abs_diff_eq(current, 0.0001)); // 验证逆变换返回原坐标。
} // 历史矩阵测试结束。

#[test]
fn hudless_usage_preserves_render_sampling_and_sdk_copy_source() {
    let mapped = map_image_usage(
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC, // GPU编码和SDK立即复制的真实用途。
        vk::ImageAspectFlags::COLOR,
    );
    assert!(mapped.contains(vk::ImageUsageFlags::TRANSFER_SRC)); // SDK可复制本帧颜色。
    assert!(mapped.contains(vk::ImageUsageFlags::SAMPLED)); // Streamline可读取颜色。
    assert!(mapped.contains(vk::ImageUsageFlags::COLOR_ATTACHMENT)); // 场景颜色由GPU编码写入。
} // 测试结束。
