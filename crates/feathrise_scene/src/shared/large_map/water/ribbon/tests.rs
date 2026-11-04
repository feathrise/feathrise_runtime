//! 三维河流、急流与瀑布带状纯构建器回归。

use super::*; // 测试直接检查内部连续数组和同源互动足迹。
use feathrise_map_content::MapWaterAsset; // 整资产预算预检回归构造最小正式水域资产。

fn point(
    id: &str,                      // 当前测试控制点稳定ID。
    position: [f32; 3],            // 当前测试三维位置。
    depth: f32,                    // 当前测试作者深度。
    flow_speed: f32,               // 当前测试局部流速。
    outgoing: MapWaterSegmentKind, // 当前测试下游段样式。
) -> MapWaterSplinePoint {
    MapWaterSplinePoint {
        id: id.to_owned(), // 使用明确非空稳定点ID。
        position,          // 移交测试指定三维位置。
        width: 4.0,        // 四米宽正好形成固定最小横向两格。
        depth,             // 移交测试指定玩法深度。
        flow_speed,        // 移交测试指定局部流速。
        outgoing,          // 移交测试指定普通河流或急流样式。
    }
}

fn sloped_river() -> MapWaterRiver {
    MapWaterRiver {
        id: "sloped-river".to_owned(), // 诊断和统一marker稳定身份。
        label: "斜河".to_owned(),      // 标签不影响纯几何。
        source_layer_id: None,         // 纯构建测试无需跨水体连接。
        target_layer_id: None,         // 纯构建测试无需跨水体连接。
        points: vec![
            point(
                "start",
                [0.0, 4.0, 0.0],
                2.0,
                1.0,
                MapWaterSegmentKind::Rapid,
            ), // 上游急流起点。
            point(
                "end",
                [10.0, 2.0, 0.0],
                4.0,
                3.0,
                MapWaterSegmentKind::River,
            ), // 下游更低且更深。
        ],
        base_color_srgba: [0.1, 0.3, 0.5, 0.8], // 有限测试水色。
        perceptual_roughness: 0.25,             // 有限测试粗糙度。
    }
}

#[test]
fn sloped_river_keeps_meter_uv_flow_style_and_actual_height_footprint() {
    assert_eq!(RIBBON_STYLE_SENTINEL, 16.0); // Rust样条网格编码必须与生产WGSL高位身份哨兵保持一致。
    let built = build_river_water_surface(&sloped_river()).expect("合法斜河必须完成固定预算构建"); // 执行唯一公开河流纯构建入口。
    let footprint = built
        .mesh
        .ribbon_footprint
        .as_ref()
        .expect("河流必须携带三维互动足迹"); // 河流互动与网格截面同源。
    let sample = footprint
        .sample(Vec2::new(5.0, 0.0))
        .expect("河流中心XZ必须命中实际斜面"); // 在上下游中点查询真实水位与深度。

    assert!((sample.surface_local_y - 3.0).abs() < 0.05); // 斜河中心实际Y必须位于四米和两米之间。
    assert!((sample.author_depth - 3.0).abs() < 0.05); // 作者深度必须按同一三角形从二米插值到四米。
    assert!(
        built
            .mesh
            .uvs
            .windows(2)
            .all(|pair| pair[0][0] <= pair[1][0])
    ); // UV0.x按截面累计米保持非递减。
    assert!(built.mesh.uvs_b.as_ref().is_some_and(|values| {
        values
            .iter()
            .any(|uv| uv[1] == RIBBON_STYLE_SENTINEL + STYLE_RAPID) // 专用高位哨兵后的一才是急流样式。
    })); // 急流段必须写入UV1.y高位样条哨兵加样式一。
    assert!(
        built
            .mesh
            .tangents
            .as_ref()
            .is_some_and(|values| values.iter().all(|tangent| tangent[0] > 0.0))
    ); // 局部切线必须始终指向下游正X。
}

#[test]
fn curved_river_keeps_every_cross_section_horizontal() {
    let mut river = sloped_river(); // 从合法材质和身份开始替换为同时转弯、下降的三点中心线。
    river.points = vec![
        point(
            "start",
            [0.0, 6.0, 0.0],
            2.0,
            1.0,
            MapWaterSegmentKind::River,
        ),
        point(
            "bend",
            [8.0, 4.0, 6.0],
            3.0,
            2.0,
            MapWaterSegmentKind::Rapid,
        ),
        point(
            "end",
            [16.0, 2.0, 0.0],
            4.0,
            3.0,
            MapWaterSegmentKind::River,
        ),
    ]; // 弯道与坡度同时变化会暴露错误的河流平行传输横轴。
    let built = build_river_water_surface(&river).expect("合法弯曲斜河必须完成固定预算构建");

    assert!(built.mesh.positions.chunks_exact(3).all(|section| {
        section
            .iter()
            .all(|position| (position[1] - section[0][1]).abs() < 0.00001)
    })); // 四米宽河固定为每截面三顶点；Y×切线必须让每一整行共享同一水位。
    assert_eq!(built.mesh.normals.len(), built.mesh.positions.len()); // 每个实际带状顶点都必须收到三角累计法线。
    assert!(built.mesh.normals.iter().all(|normal| {
        let normal = Vec3::from_array(*normal);
        normal.is_finite() && (normal.length() - 1.0).abs() < 0.0001 && normal.y > 0.0
    })); // 实际绕序累计结果必须有限、归一化且保持河面朝上。
}

#[test]
fn smooth_normals_accumulate_the_actual_non_planar_triangles() {
    let positions = vec![
        [0.0, 0.0, 0.0], // 两片非共面三角共享的第一角。
        [0.0, 0.0, 1.0], // 上游第二角。
        [1.0, 0.0, 0.0], // 下游第一角。
        [1.0, 1.0, 1.0], // 抬高的下游第二角制造不同面法线。
    ];
    let indices = vec![0, 2, 3, 0, 3, 1]; // 使用正式带状网格相同的对角线与绕序。
    let normals =
        smooth_vertex_normals(&positions, &indices).expect("非退化实际三角必须生成平滑法线");
    let first = Vec3::from_array(positions[0]);
    let first_face =
        (Vec3::from_array(positions[2]) - first).cross(Vec3::from_array(positions[3]) - first);
    let second_face =
        (Vec3::from_array(positions[3]) - first).cross(Vec3::from_array(positions[1]) - first);
    let expected = (first_face + second_face).normalize(); // 共享角应采用两片实际三角的面积向量和。

    assert!(Vec3::from_array(normals[0]).dot(expected) > 0.99999); // 不允许退回截面统一法线或未加权平均。
}

#[test]
fn near_vertical_waterfall_builds_curtain_without_interaction_footprint() {
    let waterfall = MapWaterfall {
        id: "vertical-fall".to_owned(), // 稳定瀑布身份。
        label: "直瀑".to_owned(),       // 标签不影响纯几何。
        source: None, // 端点完整性由MapWaterAsset统一校验，本测试只验证网格纯函数。
        target: None, // 端点完整性由MapWaterAsset统一校验，本测试只验证网格纯函数。
        points: vec![
            point("top", [2.0, 8.0, 3.0], 1.0, 2.0, MapWaterSegmentKind::River), // 幕布顶部。
            point(
                "bottom",
                [2.0, -4.0, 3.0],
                1.0,
                4.0,
                MapWaterSegmentKind::River,
            ), // 同XZ近竖直落水点。
        ],
        foam_strength: 0.75,                    // 顶底泡沫亮度使用明确作者值。
        base_color_srgba: [0.2, 0.4, 0.7, 0.7], // 有限测试水幕颜色。
        perceptual_roughness: 0.18,             // 有限测试水幕粗糙度。
    };
    let built = build_waterfall_water_surface(&waterfall).expect("近竖直瀑布必须建立稳定幕布标架"); // 执行唯一公开瀑布构建入口。

    assert_eq!(built.descriptor.kind, WaterSurfaceKind::Waterfall); // 统一marker类别必须准确。
    assert!(!built.descriptor.interaction_enabled); // 瀑布必须显式禁用玩家互动。
    assert!(built.mesh.ribbon_footprint.is_none()); // 瀑布不能分配或安装斜河互动足迹。
    assert!(built.mesh.triangle_count() > 0); // 合法瀑布必须生成可渲染三角形。
    assert!(built.mesh.uvs_b.as_ref().is_some_and(|values| {
        values
            .iter()
            .all(|uv| uv[1] == RIBBON_STYLE_SENTINEL + STYLE_WATERFALL) // 全幕布必须携带哨兵后的瀑布样式二。
    })); // 所有幕布顶点必须进入瀑布样式二。
}

#[test]
fn river_sections_preserve_the_full_path_beyond_the_old_budget() {
    let mut river = sloped_river(); // 从合法斜河开始只扩大真实三维段长。
    river.points[1].position = [5_000.0, 2.0, 0.0]; // 两米间距需要超过二千零四十八个截面。

    let built = build_river_water_surface(&river).unwrap();
    assert!(built.mesh.positions.len() > MAX_RIVER_RIBBON_SECTIONS * 3);
    assert!(built.mesh.uvs.last().unwrap()[0] >= 5_000.0); // 末端没有被截断或降低采样精度。
}

#[test]
fn whole_asset_preflight_accepts_an_overlong_two_point_river() {
    let mut river = sloped_river(); // 两点路径满足schema点数预算，但实际米制采样仍可能超过网格硬限制。
    river.points[1].position = [5_000.0, 2.0, 0.0]; // 两米采样间距需要超过二千零四十八个截面。
    let asset = MapWaterAsset {
        schema_version: 1, // 测试资产使用当前正式格式。
        map_id: feathrise_map_content::MapId::parse("MAP-test").expect("测试地图ID必须有效"),
        layers: Vec::new(),     // 无端点河流不需要静水层。
        rivers: vec![river],    // 只保留目标超长两点河流。
        waterfalls: Vec::new(), // 不让瀑布影响失败来源。
    };

    validate_spline_water_build_budget(&asset).expect("长河必须通过与正式后台构建一致的保存校验");
}
