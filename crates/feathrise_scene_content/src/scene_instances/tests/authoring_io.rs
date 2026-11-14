//! 正式保存路径从工程资产根重开子场景清单，核对空容器与旧版本拒绝。

use super::*;
use crate::{SCENE_SCHEMA_VERSION, SceneCatalogAsset, SceneCatalogEntry, SceneTerrainDescriptor};
use std::{fs, time::{SystemTime, UNIX_EPOCH}};

#[test]
fn saved_subscene_accepts_explicit_empty_container_and_rejects_legacy_manifest() {
    let root = std::env::temp_dir().join(format!("feathrise-subscene-{}-{}", std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir(&root).unwrap();
    let folder = "scenes/SCENE-child";
    fs::create_dir_all(root.join(folder)).unwrap();
    let child_entry = SceneCatalogEntry {
        id: id("SCENE-child"), display_name: "子场景".into(), folder: folder.into(),
        published: false, placements_glb: false, surface_path: None, vegetation_path: None,
        water_path: None, boundary_path: None, characters_path: None,
    };
    let path = root.join(child_entry.manifest_path());
    let catalog = SceneCatalogAsset { schema_version: SCENE_SCHEMA_VERSION, scenes: vec![child_entry] };
    let mut parent = manifest_fixture();
    parent.id = id("SCENE-parent");
    parent.terrain = SceneTerrainDescriptor::empty();
    parent.terrains = Some(Arc::from([]));
    let mut child = parent.clone();
    child.id = id("SCENE-child");
    parent.subscenes = Arc::from([instance("sub-1", "SCENE-child", 3.0, 0.0)]);

    fs::write(&path, serde_json::to_vec(&child).unwrap()).unwrap();
    let expanded = validate_saved_scene_instances(&root, &catalog, &parent);
    child.schema_version = SCENE_SCHEMA_VERSION - 1;
    fs::write(&path, serde_json::to_vec(&child).unwrap()).unwrap();
    let legacy = validate_saved_scene_instances(&root, &catalog, &parent);
    fs::remove_file(path).unwrap();
    fs::remove_dir(root.join(folder)).unwrap();
    fs::remove_dir(root.join("scenes")).unwrap();
    fs::remove_dir(root).unwrap();

    let expanded = expanded.unwrap();
    assert_eq!(expanded.len(), 2);
    assert!(expanded[1].container);
    assert_eq!(expanded[1].source_scene_id, child.id);
    assert_eq!(expanded[1].transform.translation, [3.0, 0.0, 0.0]);
    assert!(legacy.unwrap_err().contains(&format!("需要版本 {SCENE_SCHEMA_VERSION}")));
}
