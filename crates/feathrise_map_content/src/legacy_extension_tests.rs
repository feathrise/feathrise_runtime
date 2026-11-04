//! 旧游戏扩展路径只兼容往返；普通地图装配不请求或重写角色文件。
use super::*;
use bevy::prelude::*;

#[test]
fn legacy_character_path_round_trips_without_loading_private_asset() {
    let path = "maps/MAP-legacy/characters.map_characters.json";
    let entry: MapCatalogEntry = serde_json::from_value(serde_json::json!({
        "id": "MAP-legacy", "display_name": "旧扩展地图", "folder": "maps/MAP-legacy/",
        "published": true, "characters_path": path
    })).unwrap();
    let reopened: MapCatalogEntry = serde_json::from_slice(&serde_json::to_vec(&entry).unwrap()).unwrap();
    assert_eq!(reopened.characters_path.as_deref(), Some(path));

    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), MapContentPlugin));
    app.update(); // 原公共Startup只创建目录及预设请求，不存在游戏角色加载器。
    let catalog = app.world_mut().resource_mut::<Assets<MapCatalogAsset>>().add(MapCatalogAsset {
        schema_version: MAP_SCHEMA_VERSION, maps: vec![reopened],
    });
    app.world_mut().resource_mut::<MapContentRegistry>().catalog = catalog.clone();
    app.world_mut().write_message(AssetEvent::<MapCatalogAsset>::Added { id: catalog.id() });
    app.update();
    let registry = app.world().resource::<MapContentRegistry>();
    assert_eq!(registry.handles(&entry.id).unwrap().characters_path.as_deref(), Some(path));
    assert!(app.world().resource::<AssetServer>().get_path_ids(path).is_empty());
}

#[test]
fn legacy_map_without_character_extension_does_not_gain_one_on_save() {
    let entry: MapCatalogEntry = serde_json::from_str(
        r#"{"id":"MAP-empty","display_name":"空地图","folder":"maps/MAP-empty/"}"#,
    ).unwrap();
    assert!(entry.characters_path.is_none());
    assert!(serde_json::to_value(entry).unwrap().get("characters_path").is_none());
}
