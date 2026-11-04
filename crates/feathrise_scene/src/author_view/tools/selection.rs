//! 根据完整仿射半轴生成十二条选择框边；旋转、负缩放和剪切不退化成错误轴对齐框。

use bevy::{gizmos::config::GizmoConfigGroup, prelude::*};
use feathrise_author_tools::SelectionPreview;

pub fn draw_selection<T: GizmoConfigGroup>(gizmos: &mut Gizmos<T>, preview: &SelectionPreview) {
    let corners = corners(preview);
    let [r, g, b, a] = preview.color;
    let color = Color::linear_rgba(r, g, b, a);
    for i in 0..8 {
        for bit in [1, 2, 4] {
            if i & bit == 0 {
                gizmos.line(corners[i], corners[i | bit], color);
            }
        }
    }
}

fn corners(preview: &SelectionPreview) -> [Vec3; 8] {
    let center = Vec3::from_array(preview.center);
    let axes = preview.half_axes.map(Vec3::from_array);
    std::array::from_fn(|i| {
        center + axes[0] * if i & 1 == 0 { -1.0 } else { 1.0 }
            + axes[1] * if i & 2 == 0 { -1.0 } else { 1.0 }
            + axes[2] * if i & 4 == 0 { -1.0 } else { 1.0 }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflected_and_sheared_bounds_keep_all_original_corners() {
        let preview = SelectionPreview {
            target: "terrain:fixture".into(),
            center: [3.0, 4.0, 5.0],
            half_axes: [[-2.0, 0.0, 1.0], [1.0, 3.0, 0.0], [0.0, 0.0, 4.0]],
            color: [1.0; 4],
        };
        let corners = corners(&preview);
        assert_eq!(corners[0], Vec3::new(4.0, 1.0, 0.0));
        assert_eq!(corners[7], Vec3::new(2.0, 7.0, 10.0));
        assert_eq!(corners[1] - corners[0], Vec3::new(-4.0, 0.0, 2.0));
    }
}
