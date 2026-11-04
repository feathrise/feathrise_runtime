//! 通用占用足迹和软恢复带计算。

use bevy::prelude::{Transform, Vec2, Vec3Swizzles}; // 足迹只在世界X/Z平面解析。

use crate::{VegetationFootprint, VegetationFootprintScale, VegetationFootprintShape}; // 公共形状、缩放口径和值类型由纯函数消费。

const DISTANCE_EPSILON: f32 = 1.0e-6; // 退化椭圆的除零保护阈值。

/// 返回世界点到原型实际足迹边界的有符号距离，内部为负。
pub fn footprint_signed_distance(
    footprint: VegetationFootprint,
    transform: Transform,
    world_xz: Vec2,
) -> f32 {
    let center = transform.translation.xz(); // 足迹原点使用实例世界平移。
    let world_delta = world_xz - center; // 先保留未旋转世界差值供旋转不变的圆形直接计算。
    let scale = match footprint.scale {
        VegetationFootprintScale::InheritTransform => transform.scale.xz().abs(), // 负镜像缩放只改变朝向，不改变占用尺寸。
        VegetationFootprintScale::WorldSpace => Vec2::ONE, // 固定世界足迹忽略仅用于外观变化的实例缩放。
    };
    match footprint.shape {
        VegetationFootprintShape::Circle { radius } => {
            world_delta.length() - radius * scale.max_element() // 圆形距离与绕Y外观朝向无关，避免无意义旋转引入数值漂移。
        }
        VegetationFootprintShape::Ellipse { half_extents } => {
            let local_delta = rotate(world_delta, -yaw_radians(transform)); // 非圆形先撤销实例绕Y旋转。
            ellipse_distance(
                local_delta,
                (half_extents * scale).max(Vec2::splat(DISTANCE_EPSILON)),
            )
        }
        VegetationFootprintShape::Rectangle { half_extents }
        | VegetationFootprintShape::MeshAabb { half_extents } => {
            let local_delta = rotate(world_delta, -yaw_radians(transform)); // 矩形投影必须进入实例局部水平坐标。
            rectangle_distance(local_delta, half_extents * scale) // Mesh AABB与矩形共享投影SDF。
        }
    }
}

/// 返回足迹对目标层密度的乘数：净空内为0，恢复带外为1。
pub fn footprint_influence(
    footprint: VegetationFootprint,
    transform: Transform,
    world_xz: Vec2,
) -> f32 {
    let distance = footprint_signed_distance(footprint, transform, world_xz); // 先计算实际轮廓外距离。
    let recovery_distance = distance - footprint.exclusion_margin; // 额外边距内继续保持完全排斥。
    if recovery_distance <= 0.0 {
        return 0.0; // 实际占用和margin组成硬净空。
    }
    if footprint.recovery_width <= DISTANCE_EPSILON {
        return 1.0; // 零恢复宽度在净空边界外立即恢复。
    }
    let t = (recovery_distance / footprint.recovery_width).clamp(0.0, 1.0); // 归一化软恢复位置。
    t * t * (3.0 - 2.0 * t) // smoothstep避免密度一阶突变形成方硬边。
}

fn yaw_radians(transform: Transform) -> f32 {
    let (yaw, _, _) = transform.rotation.to_euler(bevy::math::EulerRot::YXZ); // 只消费水平Y旋转。
    yaw // 俯仰和翻滚不改变地表投影配置。
}

fn rotate(value: Vec2, angle: f32) -> Vec2 {
    let (sin, cos) = angle.sin_cos(); // 一次三角计算旋转二维向量。
    Vec2::new(cos * value.x - sin * value.y, sin * value.x + cos * value.y) // 标准二维旋转矩阵。
}

fn rectangle_distance(point: Vec2, half_extents: Vec2) -> f32 {
    let delta = point.abs() - half_extents; // 各轴超出矩形边界的有符号距离。
    delta.max(Vec2::ZERO).length() + delta.max_element().min(0.0) // 标准矩形SDF同时处理边角和内部。
}

fn ellipse_distance(point: Vec2, radii: Vec2) -> f32 {
    let normalized = point / radii; // 椭圆隐式空间中的单位半径坐标。
    let normalized_length = normalized.length(); // 小于一表示点在椭圆内部。
    if normalized_length <= DISTANCE_EPSILON {
        return -radii.min_element(); // 中心到最近边界为最小半轴。
    }
    let gradient_length = (point / (radii * radii)).length().max(DISTANCE_EPSILON); // 隐式函数梯度近似真实米制距离。
    normalized_length * (normalized_length - 1.0) / gradient_length // 对边界附近精确并保持正确符号。
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2; // 验证矩形足迹随实例绕Y旋转。

    use bevy::prelude::Quat; // 构造测试水平旋转。

    use super::*; // 足迹测试直接覆盖公开距离与恢复乘数。

    #[test]
    fn circle_uses_actual_radius_margin_and_soft_recovery() {
        let footprint = VegetationFootprint {
            shape: VegetationFootprintShape::Circle { radius: 0.72 }, // 使用非整数半径锁定实际占用与边距正交。
            scale: VegetationFootprintScale::InheritTransform, // 单位实例缩放保持既有圆形语义。
            exclusion_margin: 1.78,                            // 额外边距把硬净空扩展到2.5米。
            recovery_width: 1.0,                               // 再用一米软恢复。
        };
        let transform = Transform::IDENTITY;
        assert_eq!(
            footprint_influence(footprint, transform, Vec2::X * 2.49),
            0.0
        );
        let middle = footprint_influence(footprint, transform, Vec2::X * 3.0);
        assert!((middle - 0.5).abs() < 0.001); // smoothstep中点严格为0.5。
        assert_eq!(
            footprint_influence(footprint, transform, Vec2::X * 3.5),
            1.0
        );
    }

    #[test]
    fn rectangle_applies_scale_rotation_and_margin() {
        let footprint = VegetationFootprint {
            shape: VegetationFootprintShape::Rectangle {
                half_extents: Vec2::new(2.0, 1.0), // 未旋转时X方向更长。
            },
            scale: VegetationFootprintScale::InheritTransform, // 本测试显式验证轮廓继承实例缩放。
            exclusion_margin: 0.5,                             // 额外半米硬净空。
            recovery_width: 0.0,                               // 边界外立即恢复。
        };
        let transform = Transform {
            rotation: Quat::from_rotation_y(FRAC_PI_2), // 旋转后长轴指向世界Z。
            scale: Vec2::new(2.0, 1.0).extend(1.0).xzy(), // X局部半轴再放大两倍。
            ..Transform::IDENTITY
        };
        assert_eq!(
            footprint_influence(footprint, transform, Vec2::new(0.0, 4.4)),
            0.0
        );
        assert_eq!(
            footprint_influence(footprint, transform, Vec2::new(0.0, 4.6)),
            1.0
        );
    }

    #[test]
    fn ellipse_and_mesh_aabb_keep_margin_separate_from_scaled_shape() {
        let ellipse = VegetationFootprint {
            shape: VegetationFootprintShape::Ellipse {
                half_extents: Vec2::new(2.0, 1.0), // 两个半轴验证非圆形距离。
            },
            scale: VegetationFootprintScale::InheritTransform, // 单位变换下两种缩放口径等价。
            exclusion_margin: 0.5,                             // 实际椭圆外额外半米硬净空。
            recovery_width: 0.5,                               // 随后一段半米软恢复。
        };
        assert_eq!(
            footprint_influence(ellipse, Transform::IDENTITY, Vec2::new(2.49, 0.0)),
            0.0
        ); // 长轴实际边界外0.49米仍在硬净空。
        assert_eq!(
            footprint_influence(ellipse, Transform::IDENTITY, Vec2::new(3.0, 0.0)),
            1.0
        ); // 实际边界、margin和恢复宽度合计一米后完整恢复。

        let mesh_aabb = VegetationFootprint {
            shape: VegetationFootprintShape::MeshAabb {
                half_extents: Vec2::new(1.0, 2.0), // 原型AABB投影的X/Z半尺寸。
            },
            scale: VegetationFootprintScale::InheritTransform, // Mesh AABB按实体外观缩放得到实际尺寸。
            exclusion_margin: 0.5, // 额外边框保持世界米制，不随实例缩放。
            recovery_width: 0.0,   // 边框外立即恢复便于锁定边界。
        };
        let transform = Transform {
            scale: bevy::prelude::Vec3::new(2.0, 3.0, 0.5), // X实际半尺寸变二米，Z仍为一米。
            ..Transform::IDENTITY
        };
        assert_eq!(
            footprint_influence(mesh_aabb, transform, Vec2::new(2.49, 0.0)),
            0.0
        ); // 缩放后实际X边界外0.49米仍在额外边框。
        assert_eq!(
            footprint_influence(mesh_aabb, transform, Vec2::new(2.51, 0.0)),
            1.0
        ); // margin没有被X缩放重复放大。
    }

    #[test]
    fn world_space_footprint_ignores_visual_instance_scale() {
        let footprint = VegetationFootprint {
            shape: VegetationFootprintShape::Circle { radius: 0.72 }, // 树干原型实际半径。
            scale: VegetationFootprintScale::WorldSpace, // 净空尺寸由世界米制策略固定。
            exclusion_margin: 1.78,                      // 硬净空总半径固定为2.5米。
            recovery_width: 1.0,                         // 再用固定一米恢复带。
        };
        let transform = Transform {
            scale: bevy::prelude::Vec3::splat(3.0), // 外观缩放不得把净空扩大三倍。
            ..Transform::IDENTITY
        };
        assert_eq!(
            footprint_influence(footprint, transform, Vec2::X * 3.5),
            1.0
        ); // 固定世界半径2.5加恢复一米后已经完整恢复。
    }
}
