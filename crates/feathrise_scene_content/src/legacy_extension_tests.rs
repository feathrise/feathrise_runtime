//! 旧游戏扩展路径只兼容往返；普通场景装配不请求或重写角色文件。
use super::*;
use bevy::prelude::*;

#[test]
fn legacy_character_path_round_trips_without_loading_private_asset() {
    let path = "scenes/MAP-legacy/characters.scene_characters.json";
    let entry: SceneCatalogEntry = serde_json::from_value(serde_json::json!({
        "id": "MAP-legacy", "display_name": "旧扩展场景", "folder": "scenes/MAP-legacy/",
        "published": true, "characters_path": path
    })).unwrap();
    let reopened: SceneCatalogEntry = serde_json::from_slice(&serde_json::to_vec(&entry).unwrap()).unwrap();
    assert_eq!(reopened.characters_path.as_deref(), Some(path));

    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), SceneContentPlugin));
    app.update(); // 原公共Startup只创建目录及预设请求，不存在游戏角色加载器。
    let catalog = app.world_mut().resource_mut::<Assets<SceneCatalogAsset>>().add(SceneCatalogAsset {
        schema_version: SCENE_SCHEMA_VERSION, scenes: vec![reopened],
    });
    app.world_mut().resource_mut::<SceneContentRegistry>().catalog = catalog.clone();
    app.world_mut().write_message(AssetEvent::<SceneCatalogAsset>::Added { id: catalog.id() });
    app.update();
    let registry = app.world().resource::<SceneContentRegistry>();
    assert_eq!(registry.handles(&entry.id).unwrap().characters_path.as_deref(), Some(path));
    assert!(app.world().resource::<AssetServer>().get_path_ids(path).is_empty());
}

#[test]
fn legacy_scene_without_character_extension_does_not_gain_one_on_save() {
    let entry: SceneCatalogEntry = serde_json::from_str(
        r#"{"id":"MAP-empty","display_name":"空场景","folder":"scenes/MAP-empty/"}"#,
    ).unwrap();
    assert!(entry.characters_path.is_none());
    assert!(serde_json::to_value(entry).unwrap().get("characters_path").is_none());
}
