//! 三维水网络作者数据、稳定引用和高低水位约束回归。

use super::*; // 直接锁定网络预算、引用和高低水位约束。
use crate::{MapWaterAsset, MapWaterLayer}; // 混合资产仍从父模块统一validate入口验收。

#[test]
fn water_documents_round_trip_beyond_all_former_capacity_thresholds() {
    let mut value = asset();
    value.layers = (0..12)
        .map(|i| {
            let mut layer = layer(&format!("layer-{i}"), 2.);
            layer.resolution = [8192, 4097]; // 没有旧4096纹素门；像素实际分配另由后台负责。
            layer
        })
        .collect();
    value.layers[0].water_level = 100.;
    value.layers[1].water_level = 62.; // 瀑布端点仍须与实际静水水位连续。
    value.rivers = (0..40)
        .map(|r| {
            let mut river = river();
            river.id = format!("river-{r}");
            river.source_layer_id = None;
            river.points = (0..80)
                .map(|i| {
                    point(
                        &format!("point-{i}"),
                        [i as f32 * 2., 100. - i as f32, r as f32 * 20.],
                    )
                })
                .collect();
            river
        })
        .collect();
    value.waterfalls = (0..40)
        .map(|f| {
            let mut fall = waterfall();
            fall.id = format!("fall-{f}");
            fall.source = Some(MapWaterEndpoint::StillWater {
                layer_id: "layer-0".into(),
            });
            fall.target = Some(MapWaterEndpoint::StillWater {
                layer_id: "layer-1".into(),
            });
            fall.points = (0..20)
                .map(|i| {
                    point(
                        &format!("point-{i}"),
                        [f as f32 * 20., 100. - i as f32 * 2., 500.],
                    )
                })
                .collect();
            fall
        })
        .collect();
    value.validate(&value.map_id).unwrap();
    let bytes = serde_json::to_vec(&value).unwrap();
    let restored: MapWaterAsset = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(restored, value);
    restored.validate(&restored.map_id).unwrap();
    value.rivers[0].points[1].id = value.rivers[0].points[0].id.clone();
    assert!(value.validate(&value.map_id).is_err()); // 取消容量门不等于允许重复身份。
}

fn layer(id: &str, water_level: f32) -> MapWaterLayer {
    MapWaterLayer {
        id: id.to_owned(),    // 每张测试静水使用明确且全局唯一的稳定ID。
        label: id.to_owned(), // 标签不参与网络引用，可直接复用测试ID。
        mask_source_path: format!("maps/MAP-test/water/{id}.r8"), // 合法作者源路径。
        mask_texture_path: format!("maps/MAP-test/water/{id}.ktx2"), // 合法运行时路径。
        resolution: [512, 512], // 使用正式默认R8分辨率。
        canvas_bounds: None,
        water_level,          // 高低两层提供可验证的真实连接水位。
        still_kind: MapStillWaterKind::Lake, // 旧层默认迁移为湖泊。
        preset_id: String::new(), // 网络测试沿用兼容外观，不依赖全局水体预设目录。
        flow_direction: [1.0, 0.0], // 保留既有静水流向默认。
        flow_speed: 1.0,      // 保留既有静水流速默认。
        base_color_srgba: default_water_color(), // 复用正式默认水色。
        perceptual_roughness: default_water_roughness(), // 复用正式默认粗糙度。
    }
}

fn point(id: &str, position: [f32; 3]) -> MapWaterSplinePoint {
    MapWaterSplinePoint {
        id: id.to_owned(),                    // 测试点使用明确稳定身份。
        position,                             // 每个调用方指定可辨别的三维位置。
        width: 12.0,                          // 十二米河宽处于正式校验范围内。
        depth: 2.0,                           // 两米深度适合测试玩家水体语义。
        flow_speed: 1.0,                      // 使用现有水面默认流速。
        outgoing: MapWaterSegmentKind::River, // 普通河段作为安全测试默认值。
    }
}

fn river() -> MapWaterRiver {
    MapWaterRiver {
        id: "river-spline".to_owned(), // 与旧静水层ID保持全局唯一。
        label: "样条河流".to_owned(),  // 中文标签验证不参与稳定引用。
        source_layer_id: Some("high-water".to_owned()), // 河流从两米高静水连续流出。
        target_layer_id: None,         // 下游交给瀑布继续连接。
        points: vec![
            point("river-start", [0.0, 2.0, 0.0]), // 上游控制点。
            point("river-end", [16.0, 1.0, 0.0]),  // 下游控制点形成非零长段。
        ],
        base_color_srgba: default_water_color(), // 复用正式默认水色。
        perceptual_roughness: default_water_roughness(), // 复用正式默认粗糙度。
    }
}

fn waterfall() -> MapWaterfall {
    MapWaterfall {
        id: "waterfall".to_owned(),   // 瀑布拥有独立全局水体ID。
        label: "落差水幕".to_owned(), // 标签只用于编辑器显示。
        source: Some(MapWaterEndpoint::RiverEnd {
            river_id: "river-spline".to_owned(), // 从测试河流末端开始。
        }),
        target: Some(MapWaterEndpoint::StillWater {
            layer_id: "low-water".to_owned(), // 瀑布落入负三米低位静水层。
        }),
        points: vec![
            point("fall-start", [16.0, 1.0, 0.0]), // 瀑布口位置。
            point("fall-end", [16.0, -3.0, 0.0]),  // 落水区位置形成明确落差。
        ],
        foam_strength: default_waterfall_foam_strength(), // 使用正式默认泡沫强度。
        base_color_srgba: default_water_color(),          // 复用正式默认水色。
        perceptual_roughness: default_water_roughness(),  // 复用正式默认粗糙度。
    }
}

fn asset() -> MapWaterAsset {
    let map_id = MapId::parse("MAP-test").expect("测试地图ID必须有效");
    MapWaterAsset {
        schema_version: 1, // 新增字段仍属于向后兼容的第一版格式。
        map_id,            // 全部引用属于同一地图私有文件。
        layers: vec![layer("high-water", 2.0), layer("low-water", -3.0)], // 合法网络从高位静水经下坡河流和瀑布进入低位静水。
        rivers: vec![river()],         // 提供一条合法的两点三维河流。
        waterfalls: vec![waterfall()], // 提供端点完整的两点瀑布。
    }
}

#[test]
fn mixed_water_network_validates_stable_references_and_non_degenerate_paths() {
    let asset = asset(); // 构造静水、河流和瀑布组成的完整最小网络。

    assert!(asset.validate(&asset.map_id).is_ok()); // 合法混合水体网络必须进入后台构建。
}

#[test]
fn water_network_rejects_dangling_endpoints_and_degenerate_control_points() {
    let mut asset = asset(); // 每个失败分支都从同一份合法网络开始恢复。

    asset.waterfalls[0].target = Some(MapWaterEndpoint::StillWater {
        layer_id: "missing".to_owned(), // 悬空引用必须在启动资产加载前拒绝。
    });
    assert!(asset.validate(&asset.map_id).is_err());

    asset.waterfalls[0] = waterfall(); // 恢复合法端点后单独验证样条退化。
    asset.rivers[0].points[1].position = asset.rivers[0].points[0].position; // 制造零长段。
    assert!(asset.validate(&asset.map_id).is_err());

    asset.rivers[0] = river(); // 恢复合法河流后验证下游逆坡。
    asset.rivers[0].points[1].position[1] =
        asset.rivers[0].points[0].position[1] + MAP_WATER_DOWNHILL_EPSILON_METERS * 2.0;
    assert!(asset.validate(&asset.map_id).is_err());

    asset.rivers[0] = river(); // 恢复合法河流后验证瀑布落差。
    asset.waterfalls[0].points[1].position = [17.0, 0.95, 0.0]; // 三维段非退化但Y落差不足十厘米。
    assert!(asset.validate(&asset.map_id).is_err());

    asset.waterfalls[0] = waterfall(); // 恢复合法瀑布后验证端点完整性。
    asset.waterfalls[0].source = None; // 正式瀑布不能以半编辑状态进入运行时。
    assert!(asset.validate(&asset.map_id).is_err());
}

#[test]
fn water_network_rejects_waterfall_with_local_uphill_segment() {
    let mut asset = asset(); // 保留合法端点、材质和首尾四米总落差，只制造中间局部逆坡。
    asset.waterfalls[0].points.insert(
        1,
        point("fall-uphill", [17.0, 1.5, 0.0]), // 从一米高瀑布口先抬升至一点五米，再落至负三米。
    );

    let error = asset
        .validate(&asset.map_id)
        .expect_err("首尾总落差合法也不能掩盖瀑布中间逆坡");
    assert!(error.contains("瀑布waterfall控制点必须按水面Y不升高")); // 正式诊断必须明确指出逐段落水顺序。
}

#[test]
fn water_network_rejects_misaligned_still_and_river_connections() {
    let mut river_asset = asset(); // 每个分支从连接完全连续的高低水位网络开始。
    river_asset.rivers[0].points[0].position[1] -= 0.01; // 河流仍保持下坡，但首点不再等于高位静水水位。
    let river_error = river_asset
        .validate(&river_asset.map_id)
        .expect_err("河流首点Y与引用静水错位必须拒绝");
    assert!(river_error.contains("上游控制点Y必须与静水体high-water水位连续"));

    let mut river_endpoint_asset = asset(); // 恢复合法河流后单独制造瀑布与河尾的三维错位。
    river_endpoint_asset.waterfalls[0].points[0].position[0] += 0.01; // 水位不变但X错位，验证河端必须完整匹配XYZ。
    let river_endpoint_error = river_endpoint_asset
        .validate(&river_endpoint_asset.map_id)
        .expect_err("瀑布首点与河尾XYZ错位必须拒绝");
    assert!(river_endpoint_error.contains("上游控制点与引用水体端点不连续"));

    let mut still_endpoint_asset = asset(); // 恢复合法河尾连接后单独制造瀑布与低位静水的Y错位。
    still_endpoint_asset.waterfalls[0].points[1].position[1] += 0.01; // 首尾仍有充足落差，只违反目标静水水位连续性。
    let still_endpoint_error = still_endpoint_asset
        .validate(&still_endpoint_asset.map_id)
        .expect_err("瀑布末点Y与引用静水错位必须拒绝");
    assert!(still_endpoint_error.contains("下游控制点与引用水体端点不连续"));
}
