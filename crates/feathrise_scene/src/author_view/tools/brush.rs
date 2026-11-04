//! 原作者笔刷的固定几何；编辑器和工程视口只交换姿态、尺寸及地表来源。

use bevy::{gizmos::config::GizmoConfigGroup, prelude::*};
use feathrise_author_tools::{BrushPreview, BrushShape};
use feathrise_map_content::MapId;

use crate::ClipmapVegetationBrush;

/// 调用者先核验稳定作者来源，再显式传入本World真实快照的GPU采用身份和缺页状态。
/// DTO不携带其它World的GPU句柄，本函数绝不以平面圆替代未就绪或另一块地形的覆盖。
pub fn clipmap_brush(preview: &BrushPreview, revision: [u64; 3], missing_pages: [UVec4; 16]) -> Option<ClipmapVegetationBrush> {
    let surface = preview.clipmap.as_ref()?;
    Some(ClipmapVegetationBrush {
        map_id: MapId(surface.map_id.clone()),
        revision,
        missing_pages,
        center: Vec3::from_array(preview.center).xz(),
        radius: preview.radius,
        strength: preview.strength.unwrap_or(0.0),
        color: linear_color(preview.color),
    })
}

fn linear_color([r, g, b, a]: [f32; 4]) -> LinearRgba {
    LinearRgba::new(r, g, b, a)
}

/// 每个轮廓的线段数量固定，与地图、地形页及盖印距离无关。
pub fn draw_brush<T: GizmoConfigGroup>(gizmos: &mut Gizmos<T>, preview: &BrushPreview) {
    if preview.clipmap.is_some() {
        return; // 真实高度由现有 Clipmap GPU 材质绘制，不能重复显示悬空平圈。
    }
    let center = Vec3::from_array(preview.center);
    let rotation = Quat::from_array(preview.rotation);
    let radius = preview.radius;
    let half_height = preview.half_height;
    let color = linear_color(preview.color);
    if !center.is_finite()
        || !rotation.is_finite()
        || !radius.is_finite()
        || !half_height.is_finite()
        || radius < 0.0
        || half_height < 0.0
    {
        return;
    }
    if preview.shape == BrushShape::Planar {
        // DTO 保留来源姿态；圆自身的 Z 法线只在统一绘制边界转换一次。
        let axis_x = rotation * Vec3::X;
        let axis_z = rotation * Vec3::Z;
        let center = center + rotation * Vec3::Y * 0.08;
        let circle_rotation = rotation * Quat::from_rotation_arc(Vec3::Z, Vec3::Y);
        gizmos
            .circle(Isometry3d::new(center, circle_rotation), radius, color)
            .resolution(64);
        if let Some(strength) = preview.strength {
            gizmos
                .circle(
                    Isometry3d::new(center, circle_rotation),
                    radius * strength,
                    color,
                )
                .resolution(48);
        }
        let cross = (radius * 0.12).clamp(0.3, 8.0);
        gizmos.line(center - axis_x * cross, center + axis_x * cross, color);
        gizmos.line(center - axis_z * cross, center + axis_z * cross, color);
        return;
    }
    draw_volume(
        gizmos,
        preview.shape,
        center,
        rotation,
        radius,
        half_height,
        color,
    );
    if preview.flatten {
        // 整平十字保留原工具的锁定高度，不跟随已削低的命中点下沉。
        let target = Color::srgb(1.0, 0.55, 0.15);
        for axis in [rotation * Vec3::X, rotation * Vec3::Z] {
            gizmos.line(center - axis * radius, center + axis * radius, target);
        }
        if preview.shape != BrushShape::Box {
            let circle_rotation = rotation * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
            gizmos
                .circle(Isometry3d::new(center, circle_rotation), radius, target)
                .resolution(48);
        }
    }
}

fn draw_volume<T: GizmoConfigGroup>(
    gizmos: &mut Gizmos<T>,
    shape: BrushShape,
    center: Vec3,
    rotation: Quat,
    radius: f32,
    half_height: f32,
    color: LinearRgba,
) {
    match shape {
        BrushShape::Sphere => {
            for plane in [
                Quat::IDENTITY,
                Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
                Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            ] {
                gizmos
                    .circle(Isometry3d::new(center, rotation * plane), radius, color)
                    .resolution(48);
            }
        }
        BrushShape::Box => {
            let extent = Vec3::new(radius, half_height, radius);
            for axis in 0..3 {
                for a in [-1.0, 1.0] {
                    for b in [-1.0, 1.0] {
                        let mut low = extent;
                        low[axis] = -extent[axis];
                        low[(axis + 1) % 3] *= a;
                        low[(axis + 2) % 3] *= b;
                        let mut high = low;
                        high[axis] = extent[axis];
                        gizmos.line(center + rotation * low, center + rotation * high, color);
                    }
                }
            }
        }
        BrushShape::Cylinder => {
            let circle_rotation = rotation * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
            for sign in [-1.0, 1.0] {
                gizmos
                    .circle(
                        Isometry3d::new(
                            center + rotation * Vec3::Y * half_height * sign,
                            circle_rotation,
                        ),
                        radius,
                        color,
                    )
                    .resolution(48);
            }
            for offset in [
                Vec3::X * radius,
                -Vec3::X * radius,
                Vec3::Z * radius,
                -Vec3::Z * radius,
            ] {
                gizmos.line(
                    center + rotation * (offset - Vec3::Y * half_height),
                    center + rotation * (offset + Vec3::Y * half_height),
                    color,
                );
            }
        }
        BrushShape::Planar => {} // 平面包含强度内圈及十字，由上层统一绘制。
    }
}

#[cfg(test)]
mod tests;
