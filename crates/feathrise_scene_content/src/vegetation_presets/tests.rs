//! SDK中性目录回归：稳定身份、别名、Blade/Mesh协议与无限数量，不读取原游戏素材。
use super::*;

/// 合成目录保留通用稳定ID以覆盖兼容协议；实际模型只有自建四面体。
fn fixture_catalog_asset() -> VegetationPresetCatalogAsset {
    serde_json::from_str(include_str!("../../tests/fixtures/vegetation_presets.json"))
        .expect("中性植被夹具必须符合当前JSON格式")
}

/// 从一份已校验数据预设复制任意稳定身份，目录数量测试不依赖隐藏 Rust 适配器。
fn editable_preset(template: &VegetationPresetDescriptor, id: &str) -> VegetationPresetDescriptor {
    let mut preset = template.clone(); // 完整LOD、外观与运动来自自行合成的合法模板。
    preset.id = id.to_owned(); // 测试规范身份由调用方决定。
    preset.aliases.clear(); // 每个测试项从无历史别名的独立命名空间开始。
    preset.label = format!("测试预设{id}"); // 非空作者名称满足目录校验。
    preset.output_id = format!("{id}-output"); // 默认正式输出保持唯一。
    preset.runtime_layer_id = None; // 由新稳定 ID 确定性派生层身份。
    preset.prototype_id = None; // Blade 模板不创建 Mesh 原型身份。
    preset.seed = None; // 由新稳定 ID 确定性派生散布种子。
    preset
}

#[test]
fn neutral_catalog_exercises_blade_mesh_motion_wind_and_derivation_protocol() {
    let asset = fixture_catalog_asset(); // 公共测试自行构造七项，不携带游戏发布目录。
    let resolved = ResolvedVegetationPresetCatalog::build(&asset)
        .expect("中性植被夹具必须通过完整原子校验");

    assert_eq!(resolved.presets().len(), 7); // 四种Blade和三种Mesh覆盖不同数据分支。
    let water = resolved
        .resolve("hn02_wetland_grass")
        .expect("旧湿地草身份必须继续解析到水草预设");
    assert_eq!(water.id, "hn02_water_grass"); // 新场景只写当前规范身份。
    assert!(matches!(
        &water.runtime,
        VegetationPresetRuntime::Blade {
            motion: BladePresetMotion::WaterFlow { .. },
            ..
        }
    )); // 水草的四档外观与水流运动都由同一份可编辑目录数据声明。

    let tree = resolved.resolve("hn01_tree").expect("中性树木预设必须存在");
    let VegetationPresetRuntime::Mesh {
        lods: tree_lods, ..
    } = &tree.runtime
    else {
        panic!("树木必须是完整数据 Mesh");
    };
    assert_eq!(tree_lods.len(), 4); // 四档LOD来自目录数据，而非Rust内置注册表。
    assert!(matches!(
        tree_lods[0].parts[0].wind.as_ref().map(|wind| wind.part),
        Some(MeshPresetWindPart::Stem)
    ));
    assert!(matches!(
        tree_lods[0].parts[1].wind.as_ref().map(|wind| wind.part),
        Some(MeshPresetWindPart::Foliage)
    )); // 树干与树冠不再依赖适配器内部隐式分支。

    let shrub = resolved
        .resolve("hn01_shrub")
        .expect("中性灌木预设必须存在");
    let VegetationPresetRuntime::Mesh {
        lods: shrub_lods, ..
    } = &shrub.runtime
    else {
        panic!("灌木必须是完整数据 Mesh");
    };
    assert!(matches!(
        &shrub_lods.last().unwrap().parts[0].mesh_derivation,
        MeshPresetMeshDerivation::DispersedTriangles { max_triangles } if *max_triangles == 40
    )); // 灌木超远代理预算属于可编辑目录数据。

    let rock = resolved.resolve("hn01_rock").expect("中性岩石预设必须存在");
    let VegetationPresetRuntime::Mesh {
        lods: rock_lods, ..
    } = &rock.runtime
    else {
        panic!("岩石必须是完整数据 Mesh");
    };
    assert!(matches!(
        &rock_lods.last().unwrap().parts[0].mesh_derivation,
        MeshPresetMeshDerivation::ConvexHull { max_sample_points } if *max_sample_points == 384
    )); // 岩石超远凸包也可由F12调整而不改Rust注册代码。
}

#[test]
fn catalog_protocol_rejects_removed_registered_runtime_tags() {
    let asset = fixture_catalog_asset(); // 以合法第二版夹具提供其余全部字段。
    for runtime_type in ["registered_blade", "registered_mesh"] {
        let mut value = serde_json::to_value(&asset).expect("正式目录必须可序列化");
        value["presets"][0]["runtime"] = serde_json::json!({
            "type": runtime_type,
            "adapter": "removed_adapter"
        }); // 旧标签不得被静默忽略或降级成另一种数据后端。
        assert!(
            serde_json::from_value::<VegetationPresetCatalogAsset>(value).is_err(),
            "已删除的{runtime_type}协议必须在反序列化阶段失败"
        );
    }
}

#[test]
fn catalog_has_no_artificial_preset_count_limit() {
    let template = fixture_catalog_asset()
        .presets
        .into_iter()
        .find(|preset| preset.id == "hn01_ordinary_grass")
        .expect("中性普通草数据模板必须存在");
    let asset = VegetationPresetCatalogAsset {
        schema_version: VEGETATION_PRESET_SCHEMA_VERSION, // 使用当前正式格式。
        presets: (0..1_000)
            .map(|index| editable_preset(&template, &format!("preset-{index}")))
            .collect(), // 一千项直接验证目录不再依赖固定 Rust 数组或产品上限。
    };
    let resolved = ResolvedVegetationPresetCatalog::build(&asset)
        .expect("一千个唯一开发者预设必须可被完整解析");
    assert_eq!(resolved.presets().len(), 1_000); // 所有项保持开发者声明顺序。
    assert!(resolved.resolve("preset-999").is_some()); // 高序号项与首项同等可查询。
}

#[test]
fn aliases_resolve_to_current_id_and_hashes_are_stable() {
    let template = fixture_catalog_asset().presets.remove(0); // 使用完整数据Blade作为测试模板。
    let mut preset = editable_preset(&template, "water-grass"); // 当前规范 ID 使用英文稳定名。
    preset.aliases.push("old-water-grass".to_owned()); // 模拟开发者重命名后的历史引用。
    let asset = VegetationPresetCatalogAsset {
        schema_version: VEGETATION_PRESET_SCHEMA_VERSION, // 使用当前正式格式。
        presets: vec![preset.clone()],                    // 单项足以验证规范化。
    };
    let resolved = ResolvedVegetationPresetCatalog::build(&asset).expect("别名目录合法");
    assert_eq!(resolved.resolve("old-water-grass").unwrap().id, preset.id); // 旧场景解析到当前项。
    assert_eq!(
        stable_vegetation_identity("layer", "water-grass"),
        stable_vegetation_identity("layer", "water-grass")
    ); // 同一命名空间与 ID 跨调用逐位一致。
    assert_ne!(
        stable_vegetation_identity("layer", "water-grass"),
        stable_vegetation_identity("prototype", "water-grass")
    ); // 层与原型不会因同名而共享技术身份。
}

#[test]
fn catalog_rejects_alias_and_runtime_identity_collisions_atomically() {
    let template = fixture_catalog_asset().presets.remove(0); // 两个冲突项共享合法数据Blade结构。
    let first = editable_preset(&template, "first"); // 第一项使用自动身份。
    let mut second = editable_preset(&template, "second"); // 第二项制造显式冲突。
    second.aliases.push("first".to_owned()); // 历史别名占用另一规范 ID。
    let alias_collision = VegetationPresetCatalogAsset {
        schema_version: VEGETATION_PRESET_SCHEMA_VERSION, // 使用当前正式格式。
        presets: vec![first.clone(), second],             // 整份目录应被拒绝。
    };
    assert!(ResolvedVegetationPresetCatalog::build(&alias_collision).is_err()); // 不产生部分解析快照。

    let mut third = editable_preset(&template, "third"); // 使用另一规范 ID。
    third.runtime_layer_id = Some(first.resolved_runtime_layer_id()); // 显式占用第一项层身份。
    let runtime_collision = VegetationPresetCatalogAsset {
        schema_version: VEGETATION_PRESET_SCHEMA_VERSION, // 使用当前正式格式。
        presets: vec![first, third],                      // 两项显示身份不同但运行时身份相同。
    };
    assert!(ResolvedVegetationPresetCatalog::build(&runtime_collision).is_err()); // 注册表不得后写覆盖。
}
