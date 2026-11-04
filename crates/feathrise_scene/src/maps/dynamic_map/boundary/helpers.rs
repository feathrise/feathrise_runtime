//! 区域边界作者线段到Avian静态盒体参数的纯函数。

use bevy::prelude::*; // Quat、Transform与三维向量。
use feathrise_map_content::MapBoundarySegment; // 公共作者数据已经完成线段派生。

/// 一段可直接生成Avian Cuboid的完整几何参数。
#[derive(Clone, Debug, PartialEq)]
pub(super) struct BoundaryColliderSpec {
    /// 稳定诊断名称。
    pub(super) name: String,
    /// Cuboid完整X/Y/Z边长。
    pub(super) size: Vec3,
    /// 把局部X轴对齐作者线段的世界变换。
    pub(super) transform: Transform,
}

/// 把公共X/Z线段变换为一段竖直静态碰撞盒。
pub(super) fn collider_spec(segment: &MapBoundarySegment) -> Option<BoundaryColliderSpec> {
    let delta = segment.end - segment.start; // 二维差向量决定墙长和水平朝向。
    let length = delta.length(); // Avian cuboid接收完整边长而不是半长。
    let height = segment.top_y - segment.bottom_y; // 垂直跨度同样使用完整边长。
    if !length.is_finite()
        || length <= 0.0
        || !height.is_finite()
        || height <= 0.0
        || !segment.thickness.is_finite()
        || segment.thickness <= 0.0
    {
        return None; // F12开放草稿中的退化段不能进入物理世界。
    }
    let direction = Vec3::new(delta.x / length, 0.0, delta.y / length); // 世界二维Y对应三维Z。
    let midpoint = (segment.start + segment.end) * 0.5; // 水平中心避免端点旋转产生位移。
    let center_y = segment.bottom_y + height * 0.5; // 同号大高度不能先相加溢出；中心仍覆盖完整作者跨度。
    Some(BoundaryColliderSpec {
        name: format!(
            "DynamicMapBoundary_{}_{}",
            segment.path_id, segment.segment_index
        ), // 实体名同时指出路径和线段，物理调试可直接定位。
        size: Vec3::new(length + segment.thickness, height, segment.thickness), // 两端各延伸半个墙厚，使相邻折线段在拐角处可靠重叠封缝。
        transform: Transform::from_translation(Vec3::new(midpoint.x, center_y, midpoint.y))
            .with_rotation(Quat::from_rotation_arc(Vec3::X, direction)), // 无需手算左右手系yaw符号。
    })
}

#[cfg(test)]
mod tests {
    use super::*; // 回归只验证纯几何，不启动Avian插件。

    #[test] // 任意方向线段必须生成正确完整边长、中心和端点方向。
    fn diagonal_segment_builds_a_full_length_vertical_wall() {
        let segment = MapBoundarySegment {
            path_id: "main".to_owned(), // 稳定测试路径。
            segment_index: 2,           // 非零编号验证名称与组件不依赖顺序起点。
            start: Vec2::new(1.0, 2.0), // 世界X/Z起点。
            end: Vec2::new(4.0, 6.0),   // 三四五直角线段终点。
            bottom_y: -10.0,            // 墙底。
            top_y: 30.0,                // 墙顶。
            thickness: 2.0,             // 完整厚度。
        };
        let spec = collider_spec(&segment).expect("有效线段必须生成碰撞规格"); // 执行正式纯函数。
        assert_eq!(spec.size, Vec3::new(7.0, 40.0, 2.0)); // 五米主体加两端各一米封缝延伸，且不能误用半长。
        assert_eq!(spec.transform.translation, Vec3::new(2.5, 10.0, 4.0)); // 中心应为三轴端点中值。
        let aligned = spec.transform.rotation * Vec3::X; // 读取局部X经过旋转后的世界方向。
        assert!(aligned.abs_diff_eq(Vec3::new(0.6, 0.0, 0.8), 1.0e-5)); // 方向必须与X/Z线段一致。
    }
}
