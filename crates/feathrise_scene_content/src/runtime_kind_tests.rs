//! 场景清单正式运行类型与作者字段的往返校验。

use crate::{SceneManifestAsset, SceneRuntimeKind};

fn manifest(runtime: &str) -> SceneManifestAsset {
    serde_json::from_value(serde_json::json!({
        "schema_version": 2, "id": "MAP-runtime-kind", "display_name": "通用场景",
        "runtime_scene": runtime, "world_min": [0, 0], "world_max": [10, 10],
        "default_spawn": [0, 2, 0], "terrains": [], "notes": "原作者备注"
    })).unwrap()
}

#[test]
fn public_name_is_the_only_supported_runtime_kind() {
    let kind = SceneRuntimeKind::DataDriven;
    assert_eq!(kind.as_str(), "data_driven");
    assert_eq!(SceneRuntimeKind::parse(kind.as_str()), Some(kind));
    assert!(manifest(kind.as_str()).is_data_driven());
}

#[test]
fn unknown_and_retired_names_are_not_valid_data_driven_scenes() {
    for name in ["", "DynamicMapScene", "KaifengScene", "YellowRiverScene", "FutureRuntime", "data_driven "] {
        assert_eq!(SceneRuntimeKind::parse(name), None);
        assert!(!manifest(name).is_data_driven());
    }
}

#[test]
fn reading_and_saving_preserves_runtime_name_and_author_fields() {
    let name = "data_driven";
    let before = manifest(name);
    let bytes = serde_json::to_vec(&before).unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(saved["runtime_scene"], name);
    let after: SceneManifestAsset = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(after.runtime_scene, before.runtime_scene);
    assert_eq!(after.id, before.id);
    assert_eq!(after.notes, before.notes);
    assert!(after.is_data_driven());
}

#[test]
#[ignore = "需显式提供 FEATHRISE_SCENE_COMPATIBILITY_PATHS_JSON，普通公开测试不读取外部工程"]
fn externally_selected_scenes_read_and_roundtrip_without_rewriting_sources() {
    let list_path = std::env::var_os("FEATHRISE_SCENE_COMPATIBILITY_PATHS_JSON")
        .expect("请提供包含场景清单绝对路径数组的JSON文件");
    let paths: Vec<std::path::PathBuf> = serde_json::from_slice(
        &std::fs::read(list_path).expect("兼容清单路径列表必须可读"),
    ).expect("兼容清单必须是JSON路径数组");
    assert!(!paths.is_empty(), "不能把零份场景视为兼容验收通过");
    for path in paths {
        assert!(path.is_absolute(), "验收输入须显式指定绝对路径：{}", path.display());
        let bytes = std::fs::read(&path).unwrap();
        let json = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
        let before: SceneManifestAsset = serde_json::from_slice(json)
            .unwrap_or_else(|error| panic!("场景{}读取失败：{error}", path.display()));
        assert!(before.is_data_driven(), "场景{}运行类型不受支持", path.display());
        crate::validate_terrain_objects(&before)
            .unwrap_or_else(|error| panic!("场景{}地形无效：{error}", path.display()));
        crate::validate_scene_instances(&before.id, &before.subscenes)
            .unwrap_or_else(|error| panic!("场景{}子图引用无效：{error}", path.display()));
        let serialized = serde_json::to_vec(&before).unwrap();
        let reopened: SceneManifestAsset = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(reopened.runtime_scene, before.runtime_scene);
        assert_eq!(reopened.id, before.id);
        assert_eq!(reopened.display_name, before.display_name);
        assert_eq!(reopened.notes, before.notes);
        assert_eq!(serde_json::to_value(&reopened).unwrap(), serde_json::to_value(&before).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "验收不得写回{}", path.display());
        println!("兼容读取与内存往返通过：{} ({})", before.id, path.display());
    }
}
