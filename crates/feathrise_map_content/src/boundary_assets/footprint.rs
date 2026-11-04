//! 空气墙地表辅助线的共享几何，确保F1编辑器样式与F12轮廓一致。
use bevy::prelude::{Vec2, Vec3}; // 只计算作者辅助线，不读取ECS或修改地形。

/// 游戏与编辑器使用相同的端点贴地规则；越界或无高度时回退地图基准面。
pub fn boundary_display_point(
    heights: Option<&[u16]>,            // 当前地图或草稿的共享高度数据。
    manifest: &crate::MapManifestAsset, // 高度编码与世界坐标范围。
    point: [f32; 2],                    // 作者轮廓端点。
) -> Vec3 {
    let height = heights
        .and_then(|heights| {
            crate::sample_heightfield_world(
                heights,                       // 不复制整张高度图。
                manifest.terrain.resolution,   // 顶点分辨率。
                point,                         // 世界X/Z。
                manifest.world_min,            // 世界最小角。
                manifest.world_max,            // 世界最大角。
                manifest.terrain.base_height,  // 高度零点。
                manifest.terrain.height_scale, // 编码跨度。
            )
        })
        .unwrap_or(manifest.terrain.base_height); // 与原编辑器回退规则一致。
    Vec3::new(point[0], height + 0.35, point[1]) // 避免轮廓与地表深度重合。
}

/// 返回中心线和延伸封缝后的两侧边线；退化草稿不产生轮廓。
pub fn boundary_footprint_lines(
    start: [f32; 2], // 作者线起点。
    end: [f32; 2],   // 作者线终点。
    thickness: f32,  // 完整墙厚。
) -> Option<[([f32; 2], [f32; 2]); 3]> {
    let a = Vec2::from_array(start); // 世界X/Z起点。
    let b = Vec2::from_array(end); // 世界X/Z终点。
    let delta = b - a; // 作者线的方向。
    let length = delta.length(); // 归一化前验证长度。
    if !length.is_finite() || length <= 0.0 || !thickness.is_finite() || thickness <= 0.0 {
        return None; // 无效线段不生成异常顶点。
    }
    let direction = delta / length; // 单位切向。
    let half_width = thickness * 0.5; // 两端和两侧各偏移半墙厚。
    let normal = Vec2::new(-direction.y, direction.x) * half_width; // 墙的横向半宽。
    let a = a - direction * half_width; // 起点封缝延伸。
    let b = b + direction * half_width; // 终点封缝延伸。
    Some([
        (start, end),                                       // 中心线严格保留作者端点。
        ((a + normal).to_array(), (b + normal).to_array()), // 左侧真实轮廓。
        ((a - normal).to_array(), (b - normal).to_array()), // 右侧真实轮廓。
    ])
}

#[cfg(test)]
mod tests {
    use super::*; // 用相同纯函数验证游戏与编辑器的轮廓来源。
    #[test]
    fn air_wall_display_point_follows_ground_instead_of_wall_height() {
        let manifest = serde_json::from_str(
            r#"{
            "schema_version":1,"id":"MAP-footprint","display_name":"轮廓测试",
            "runtime_scene":"DynamicMapScene","world_min":[0,0],"world_max":[10,10],
            "default_spawn":[0,0,0],"asset_roots":[],
            "terrain":{"kind":"heightfield","glb_path":"terrain.glb","height_path":"height.r16",
                "resolution":[2,2],"base_height":20,"height_scale":100}
        }"#,
        )
        .unwrap(); // 固定小高度场，不需要Bevy运行时。
        let flat_top = [u16::MAX; 4]; // 编码对应120米地表。
        let point = boundary_display_point(Some(&flat_top), &manifest, [5.0, 5.0]); // 走正式共用采样。
        assert!((point.y - 120.35).abs() < 0.001); // 轮廓仅抬高0.35米。
        assert_eq!([point.x, point.z], [5.0, 5.0]); // 水平端点保持原位。
        let fallback = boundary_display_point(None, &manifest, [5.0, 5.0]); // 未就绪时沿用编辑器回退。
        assert!((fallback.y - 20.35).abs() < 0.001); // 不读墙底/墙顶高度。
    }
    #[test]
    fn air_wall_footprint_has_three_lines_and_preserves_author_endpoints() {
        let lines = boundary_footprint_lines([0.0, 0.0], [10.0, 0.0], 2.0).unwrap(); // 十米墙。
        assert_eq!(lines[0], ([0.0, 0.0], [10.0, 0.0])); // 中心线不按物理块拆分。
        assert_eq!(lines[1], ([-1.0, 1.0], [11.0, 1.0])); // 一侧包含端点延伸。
        assert_eq!(lines[2], ([-1.0, -1.0], [11.0, -1.0])); // 另一侧镜像。
        assert!(boundary_footprint_lines([0.0; 2], [0.0; 2], 2.0).is_none()); // 退化段不会污染显示。
    }
}
