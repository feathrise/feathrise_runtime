//! 雨向投影纯函数：与真实雨速度一致，像素对齐且无世界几何扫描。
use super::constants::{DEPTH, RESOLUTION, SPAN, UPSTREAM};
use bevy::camera::ScalingMode; // 固定米制正交窗口。
use bevy::prelude::*; // 相机投影和线性代数。 // 生产与采样唯一预算。
pub(super) fn projection() -> OrthographicProjection {
    // 精确固定窗口，不依赖游戏分辨率。
    OrthographicProjection {
        // 显式area也供首次camera_system之前的矩阵使用。
        near: 0.0,  // 捕获起点不遗漏薄屋顶。
        far: DEPTH, // 只覆盖局部雨向深度。
        area: Rect::from_center_size(Vec2::ZERO, Vec2::splat(SPAN)), // 固定正方形。
        scaling_mode: ScalingMode::Fixed {
            width: SPAN,
            height: SPAN,
        }, // 相机更新保持同一范围。
        ..OrthographicProjection::default_3d()  // 保留默认中心原点和单位缩放。
    } // 返回投影。
} // 投影构造结束。
pub(super) fn capture_view(center: Vec3, velocity: Vec3) -> Option<(Transform, Mat4)> {
    // 非法输入不发布NaN。
    if !center.is_finite()
        || !velocity.is_finite()
        || !velocity.length_squared().is_finite()
        || velocity.length_squared() < 0.001
    {
        return None;
    } // 极端有限值溢出也明确失败回退。
    let direction = velocity.normalize(); // 与雨丝实际风向对齐。
    let rotation = Transform::IDENTITY
        .looking_to(direction, Vec3::NEG_Z)
        .rotation; // 雨向接近竖直，负Z提供稳定上轴。
    let local = rotation.inverse() * center; // 在雨向平面而非世界XZ对齐。
    let pixel = SPAN / RESOLUTION as f32; // 固定世界像素宽度。
    let snapped = (local / pixel).round() * pixel; // 避免亚像素移动令屋檐闪烁。
    let origin = rotation * snapped - direction * UPSTREAM; // 上游捕获完整局部建筑。
    let transform = Transform::from_translation(origin).with_rotation(rotation); // 相机没有父节点。
    let clip = Mat4::orthographic_rh(-SPAN * 0.5, SPAN * 0.5, -SPAN * 0.5, SPAN * 0.5, DEPTH, 0.0); // Bevy反向深度近1远0。
    Some((transform, clip * transform.to_matrix().inverse())) // 同批返回真实视图和投影。
} // 纯函数结束。
