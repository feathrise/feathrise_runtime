// helpers.rs — Billboard 文字纯函数（无 ECS 依赖，可单独复用）

use bevy::asset::RenderAssetUsages;
use bevy::camera::{OrthographicProjection, Projection, ScalingMode};
use bevy::mesh::Indices;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;

/// 构造与离屏纹理等比的正交投影，使 Text2d 1:1 映射到纹理像素。
pub fn billboard_projection(texture_size: UVec2) -> Projection {
    Projection::Orthographic(OrthographicProjection {
        scaling_mode: ScalingMode::Fixed {
            width: texture_size.x as f32,
            height: texture_size.y as f32,
        },
        ..OrthographicProjection::default_2d()
    })
}

/// 由「有效采样像素宽度」换算 Billboard 世界宽度（保持与世界高度同比例）。
///
/// `world_height` 对应 `texture_height` 像素，故每像素世界尺寸 = world_height/texture_height。
pub fn mesh_width_for_pixel_width(pixel_width: f32, texture_height: f32, world_height: f32) -> f32 {
    pixel_width.max(1.0) * (world_height / texture_height.max(1.0))
}

/// 由「有效采样像素宽度」计算需要采样的中心 UV 窗口 `(u_min, u_max)`。
pub fn centered_uv_window(pixel_width: f32, texture_width: f32) -> (f32, f32) {
    let clamped_width = pixel_width.clamp(1.0, texture_width.max(1.0));
    let uv_span = (clamped_width / texture_width.max(1.0)).clamp(0.0, 1.0);
    let half_span = uv_span * 0.5;
    (0.5 - half_span, 0.5 + half_span)
}

/// 生成一个按给定 UV 窗口采样的 Billboard 四边形网格。
pub fn build_billboard_mesh(mesh_width: f32, world_height: f32, u_min: f32, u_max: f32) -> Mesh {
    let half_width = mesh_width * 0.5;
    let half_height = world_height * 0.5;

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [-half_width, -half_height, 0.0],
            [half_width, -half_height, 0.0],
            [half_width, half_height, 0.0],
            [-half_width, half_height, 0.0],
        ],
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        vec![
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ],
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[u_min, 1.0], [u_max, 1.0], [u_max, 0.0], [u_min, 0.0]],
    );
    mesh.insert_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]));
    mesh
}

#[cfg(test)]
mod tests {
    use super::{centered_uv_window, mesh_width_for_pixel_width};

    #[test]
    fn mesh_width_matches_effective_pixel_width() {
        // 384px 文字、128px 纹理高、0.24m 世界高 → 0.72m 世界宽
        let width = mesh_width_for_pixel_width(384.0, 128.0, 0.24);
        assert!((width - 0.72).abs() < 0.0001);
    }

    #[test]
    fn uv_window_stays_centered() {
        let (u_min, u_max) = centered_uv_window(384.0, 1024.0);
        assert!((u_min - 0.3125).abs() < 0.0001);
        assert!((u_max - 0.6875).abs() < 0.0001);
    }
}
