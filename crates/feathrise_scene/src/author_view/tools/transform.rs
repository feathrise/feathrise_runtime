//! 根据原手柄语义生成固定预算几何；两种视口共用外形，作者端继续独占拾取与事务。

use bevy::{gizmos::config::GizmoConfigGroup, prelude::*};
use feathrise_author_tools::{TransformPreview, AUTHOR_ROTATION_SEGMENTS,
    AUTHOR_PLANE_INNER_FRACTION, AUTHOR_PLANE_OUTER_FRACTION};

pub fn draw_transform<T: GizmoConfigGroup>(gizmos: &mut Gizmos<T>, preview: &TransformPreview) {
    match preview {
        TransformPreview::Move {
            spatial,
            origin, length, axes, active_axes, plane, plane_active, plane_cross,
            plane_color, color_active, axis_colors, anchor, marker_radius,
        } => {
            let origin = Vec3::from_array(*origin);
            let active = color(*color_active);
            for (index, direction) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().enumerate() {
                if axes & (1 << index) != 0 {
                    let tint = if active_axes & (1 << index) != 0 { active } else { color(axis_colors[index]) };
                    gizmos.arrow(origin, origin + direction * *length, tint)
                        .with_tip_length(*length * if *spatial { 0.10 } else { 0.14 });
                }
            }
            if *plane {
                let corners = plane_corners(origin, *length);
                let tint = if *plane_active { active } else { color(*plane_color) };
                for i in 0..4 { gizmos.line(corners[i], corners[(i + 1) % 4], tint); }
                if *plane_cross {
                    gizmos.line(corners[0], corners[2], tint);
                    gizmos.line(corners[1], corners[3], tint);
                }
            }
            if let Some(anchor) = anchor {
                gizmos.line(Vec3::from_array(*anchor), origin, active);
            }
            if let Some(radius) = marker_radius {
                gizmos.circle(
                    Isometry3d::new(origin, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                    *radius, active,
                ).resolution(16);
            }
        }
        TransformPreview::Rotate { origin, radius, yaw_degrees, before_yaw_degrees, color: tint } => {
            let origin = Vec3::from_array(*origin);
            let tint = color(*tint);
            // 与原投影/拾取的64段采样同向且同起点，不能改用默认圆环分辨率。
            let mut previous = origin + yaw_direction(0.0) * *radius;
            for index in 1..=AUTHOR_ROTATION_SEGMENTS {
                let angle = std::f32::consts::TAU * index as f32 / AUTHOR_ROTATION_SEGMENTS as f32;
                let next = origin + yaw_direction(angle.to_degrees()) * *radius;
                gizmos.line(previous, next, tint);
                previous = next;
            }
            if let Some(yaw) = yaw_degrees {
                gizmos.arrow(origin, origin + yaw_direction(*yaw) * *radius, tint).with_tip_length(*radius * 0.12);
            }
            if let Some(yaw) = before_yaw_degrees {
                gizmos.line(origin, origin + yaw_direction(*yaw) * *radius, Color::srgba(0.745, 0.529, 0.243, 0.45));
            }
        }
        TransformPreview::Scale { origin, right, up, half_size, color: tint, cube } => {
            let origin = Vec3::from_array(*origin);
            let tint = color(*tint);
            if *cube {
                gizmos.cube(Transform::from_translation(origin).with_scale(Vec3::splat(*half_size * 2.0)), tint);
            } else {
                let right = Vec3::from_array(*right) * *half_size;
                let up = Vec3::from_array(*up) * *half_size;
                let corners = [origin - right - up, origin + right - up, origin + right + up, origin - right + up];
                for i in 0..4 { gizmos.line(corners[i], corners[(i + 1) % 4], tint); }
                gizmos.line(corners[0], corners[2], tint);
                gizmos.line(corners[1], corners[3], tint);
            }
        }
    }
}

fn color([r, g, b, a]: [f32; 4]) -> Color { Color::linear_rgba(r, g, b, a) }

fn yaw_direction(degrees: f32) -> Vec3 {
    let angle = degrees.to_radians();
    Vec3::new(angle.cos(), 0.0, -angle.sin())
}

fn plane_corners(origin: Vec3, length: f32) -> [Vec3; 4] {
    // 与原作者plane_quad共用0.20/0.52比例；协议的length已经通过原可见/退化门。
    let inner = length * AUTHOR_PLANE_INNER_FRACTION;
    let outer = length * AUTHOR_PLANE_OUTER_FRACTION;
    [origin + Vec3::new(inner, 0.0, inner), origin + Vec3::new(outer, 0.0, inner),
        origin + Vec3::new(outer, 0.0, outer), origin + Vec3::new(inner, 0.0, outer)]
}
