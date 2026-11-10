//! 建筑预设目录兼容、别名和纯校验回归。

use super::*; // 测试直接锁定公开目录协议。

/// 构造一个不依赖真实 GLB 文件读取的合法测试预设。
fn preset(id: &str) -> BuildingPresetDescriptor {
    BuildingPresetDescriptor::from_asset(
        id.to_owned(),                        // 调用方控制稳定身份。
        format!("测试建筑{id}"),              // 使用非空作者显示名。
        "gltf/buildings/test.glb".to_owned(), // 纯校验只检查 assets 相对路径形状。
        ScenePlacementAssetTarget::Scene0,      // 基线使用完整场景层级。
    )
}

#[test]
fn aliases_resolve_to_the_current_building_identity() {
    let mut current = preset("house-current"); // 从完整合法项制造一次重命名。
    current.aliases.push("house-old".to_owned()); // 历史场景仍可能保存旧身份。
    let asset = BuildingPresetCatalogAsset {
        schema_version: BUILDING_PRESET_SCHEMA_VERSION, // 使用当前首版格式。
        presets: vec![current],                         // 单项足以验证规范化。
    };
    let resolved = ResolvedBuildingPresetCatalog::build(&asset).expect("别名目录应合法"); // 整份目录一次建立索引。
    assert_eq!(resolved.resolve("house-old").unwrap().id, "house-current"); // 旧引用必须返回当前规范项。
}

#[test]
fn invalid_geometry_and_alias_collisions_reject_the_whole_catalog() {
    let first = preset("first-house"); // 第一项占用规范身份。
    let mut second = preset("second-house"); // 第二项制造跨项别名冲突。
    second.aliases.push("first-house".to_owned()); // 别名不能覆盖另一规范 ID。
    let collision = BuildingPresetCatalogAsset {
        schema_version: BUILDING_PRESET_SCHEMA_VERSION, // 使用正确版本以单独测试身份冲突。
        presets: vec![first.clone(), second],           // 整份目录必须失败关闭。
    };
    assert!(ResolvedBuildingPresetCatalog::build(&collision).is_err()); // 不发布部分快照。

    let mut invalid = first; // 复用其余全部合法字段。
    invalid.footprint.half_extents[0] = 0.0; // 退化占地不能进入重叠索引。
    let geometry = BuildingPresetCatalogAsset {
        schema_version: BUILDING_PRESET_SCHEMA_VERSION, // 正确版本下应定位几何错误。
        presets: vec![invalid],                         // 单项失败也拒绝完整目录。
    };
    assert!(ResolvedBuildingPresetCatalog::build(&geometry).is_err()); // 旧有效快照可由调用方继续保留。
}

#[test]
fn building_preset_created_from_asset_round_trips_with_defaults() {
    let original = preset("round-trip-house"); // 使用生产构造器生成首个可编辑值。
    let json = serde_json::to_string(&original).expect("建筑预设必须可序列化"); // 模拟 F12 原子保存前编码。
    let restored: BuildingPresetDescriptor =
        serde_json::from_str(&json).expect("建筑预设必须可重新载入"); // 模拟 AssetServer 热更新读取。
    assert_eq!(restored, original); // 占地、入口、地基、净空和室内类别均不得丢失。
    assert!(restored.connections.is_empty()); // 旧 JSON 缺字段时明确兼容为空连接点集合。
}

#[test]
fn invalid_connection_identity_geometry_and_direction_reject_catalog() {
    let mut duplicate = preset("connection-house");
    duplicate.connections = vec![
        BuildingConnectionPointDescriptor {
            id: "wall-left".to_owned(),
            kind: "wall-end".to_owned(),
            position: [-2.0, 0.0],
            outward_yaw_degrees: 270.0,
        },
        BuildingConnectionPointDescriptor {
            id: "wall-left".to_owned(), // 同一预设内稳定点 ID 不能复用。
            kind: "wall-end".to_owned(),
            position: [2.0, 0.0],
            outward_yaw_degrees: 90.0,
        },
    ];
    let asset = BuildingPresetCatalogAsset {
        schema_version: BUILDING_PRESET_SCHEMA_VERSION,
        presets: vec![duplicate],
    };
    assert!(ResolvedBuildingPresetCatalog::build(&asset).is_err());

    let mut bad_kind = preset("bad-kind-house");
    bad_kind
        .connections
        .push(BuildingConnectionPointDescriptor {
            id: "end".to_owned(),
            kind: String::new(), // 空类型不能意外兼容所有点。
            position: [0.0, 0.0],
            outward_yaw_degrees: 0.0,
        });
    assert!(
        ResolvedBuildingPresetCatalog::build(&BuildingPresetCatalogAsset {
            schema_version: BUILDING_PRESET_SCHEMA_VERSION,
            presets: vec![bad_kind],
        })
        .is_err()
    );

    let mut bad_direction = preset("bad-direction-house");
    bad_direction
        .connections
        .push(BuildingConnectionPointDescriptor {
            id: "end".to_owned(),
            kind: "wall-end".to_owned(),
            position: [f32::NAN, 0.0], // 非有限位置必须先失败关闭。
            outward_yaw_degrees: 0.0,
        });
    assert!(
        ResolvedBuildingPresetCatalog::build(&BuildingPresetCatalogAsset {
            schema_version: BUILDING_PRESET_SCHEMA_VERSION,
            presets: vec![bad_direction],
        })
        .is_err()
    );

    let mut arbitrary_angle = preset("arbitrary-angle-house");
    arbitrary_angle
        .connections
        .push(BuildingConnectionPointDescriptor {
            id: "end".to_owned(),
            kind: "wall-end".to_owned(),
            position: [0.0, 0.0],
            outward_yaw_degrees: 45.0, // 有限任意角也不属于首期模块协议。
        });
    assert!(
        ResolvedBuildingPresetCatalog::build(&BuildingPresetCatalogAsset {
            schema_version: BUILDING_PRESET_SCHEMA_VERSION,
            presets: vec![arbitrary_angle],
        })
        .is_err()
    );
}

#[test]
fn connection_marker_uses_instance_quarter_turn_and_foundation_plane() {
    let mut module = preset("marker-house");
    module.foundation.model_base_y = 0.5;
    let connection = BuildingConnectionPointDescriptor {
        id: "right".to_owned(),
        kind: "wall-end".to_owned(),
        position: [2.0, 1.0],
        outward_yaw_degrees: 90.0,
    };
    let (position, outward) =
        module.world_connection_marker(&connection, [10.0, 3.0, 20.0], 90.0, [1.0, 2.0, 1.0]);
    assert!((position.x - 11.0).abs() < 0.001); // 局部 +Z 经九十度旋到世界 +X。
    assert!((position.y - 4.0).abs() < 0.001); // 地基基准随 Y 缩放后叠加根高度。
    assert!((position.z - 18.0).abs() < 0.001); // 局部 +X 经九十度旋到世界 -Z。
    assert!((outward - Vec3::NEG_Z).length() < 0.001); // 局部 +X 朝外随实例旋到世界 -Z。
}
