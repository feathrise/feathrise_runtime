//! 已保存工程的轻量游戏入口：只读地图目录和所选清单，不扫描全部地图摆放或运行作者审计。

use crate::project::{ProjectContext, context};
use bevy::prelude::*;
use feathrise_map_content::{
    MAP_CATALOG_ASSET_PATH, MAP_SCHEMA_VERSION, MapCatalogAsset, MapId,
    MapManifestAsset,
};
#[cfg(feature = "validation")]
use feathrise_map_content::MapCatalogEntry;
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

const SAVED_SELECTION_FILE: &str = "scene.layout.json";
const SAVED_SELECTION_LIMIT: u64 = 64 * 1024;

#[derive(Resource)]
pub(crate) struct SavedGameStartup {
    pub map_id: MapId,
    pub name: String,
    pub spawn: Vec3,
    // 仅显式运行验收需要目录条目核对来源；正式启动只保留地图身份、名称与出生点。
    #[cfg(feature = "validation")]
    pub entry: MapCatalogEntry,
}

/// 空工程可以启动空窗口；只有没有地图也没有显式选择时才为空，坏启动图仍报告错误。
pub(crate) fn load_optional(
    project: &ProjectContext,
    saved_play: bool,
) -> Result<Option<SavedGameStartup>, String> {
    let requested = saved_play
        .then(|| std::env::var("EDITOR_SAVED_PLAY_MAP").ok())
        .flatten();
    if requested.is_none()
        && saved_selection(project)?.is_none()
        && read_catalog(project, &mut || Ok(()))?.maps.is_empty()
    {
        return Ok(None);
    }
    load_with_map(project, requested).map(Some)
}

fn load_with_map(
    project: &ProjectContext,
    requested: Option<String>,
) -> Result<SavedGameStartup, String> {
    load_with_map_checked(project, requested, &mut || Ok(()))
}

/// 宿主后台试玩复用游戏自己的目标校验；不读另一地图或预设，不写入作者文件。
pub fn validate_saved_map(project: &ProjectContext, map_id: &str) -> Result<(), String> {
    validate_saved_map_with_check(project, map_id, || Ok(()))
}

/// 目标清单可能很大；调用方的取消牌在每64KiB文件读取与目录身份检查之间生效。
pub fn validate_saved_map_with_check(
    project: &ProjectContext,
    map_id: &str,
    mut check: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    load_with_map_checked(project, Some(map_id.to_owned()), &mut check).map(|_| ())
}

fn load_with_map_checked(
    project: &ProjectContext,
    requested: Option<String>,
    check: &mut impl FnMut() -> Result<(), String>,
) -> Result<SavedGameStartup, String> {
    check()?;
    let catalog = read_catalog(project, check)?;
    let id = requested
        .or(saved_selection(project)?)
        .or_else(|| {
            catalog
                .maps
                .iter()
                .find(|map| map.published)
                .map(|map| map.id.0.clone())
        })
        .ok_or("工程没有已保存启动地图，请先创建/保存地图")?;
    MapId::parse(&id)?;
    let entry = catalog
        .maps
        .into_iter()
        .find(|map| map.id.0 == id)
        .ok_or("已保存启动图不属于本工程地图目录")?;
    let path = context::checked_existing(
        project.asset_root(),
        Path::new(&entry.manifest_path()),
        false,
    )?;
    let manifest: MapManifestAsset = read_json_checked(&path, check)?;
    if manifest.id != entry.id
        || manifest.schema_version != MAP_SCHEMA_VERSION
        || !manifest.is_data_driven()
    {
        return Err("启动地图清单身份或版本不一致".into());
    }
    if !manifest.default_spawn.iter().all(|value| value.is_finite()) {
        return Err("启动地图起始位置包含非有限数值".into());
    }
    check()?;
    Ok(SavedGameStartup {
        map_id: entry.id.clone(),
        name: manifest.display_name,
        spawn: Vec3::from_array(manifest.default_spawn),
        #[cfg(feature = "validation")]
        entry,
    })
}

fn read_catalog(
    project: &ProjectContext,
    check: &mut impl FnMut() -> Result<(), String>,
) -> Result<MapCatalogAsset, String> {
    let path = context::checked_existing(
        project.asset_root(),
        Path::new(MAP_CATALOG_ASSET_PATH),
        false,
    )?;
    let catalog: MapCatalogAsset = read_json_checked(&path, check)?;
    if catalog.schema_version != MAP_SCHEMA_VERSION {
        return Err("正式地图目录版本不兼容".into());
    }
    let mut ids = BTreeSet::new();
    for entry in &catalog.maps {
        check()?;
        MapId::parse(&entry.id.0)?;
        if !ids.insert(entry.id.0.as_str()) {
            return Err(format!("重复地图ID：{}", entry.id));
        }
    }
    Ok(catalog)
}

/// 沿用生产流式JSON/BOM语义，只在文件流下层加取消检查；不复制清单或维护另一份地图校验。
fn read_json_checked<T: serde::de::DeserializeOwned>(
    path: &Path,
    check: &mut impl FnMut() -> Result<(), String>,
) -> Result<T, String> {
    check()?;
    let file = File::open(path).map_err(|error| format!("读取{}失败：{error}", path.display()))?;
    let mut reader = BufReader::with_capacity(64 * 1024, CheckedReader { file, check });
    if reader
        .fill_buf()
        .map_err(|error| error.to_string())?
        .starts_with(&[0xef, 0xbb, 0xbf])
    {
        reader.consume(3);
    }
    serde_json::from_reader(reader).map_err(|error| format!("解析{}失败：{error}", path.display()))
}

struct CheckedReader<'a, F> {
    file: File,
    check: &'a mut F,
}

impl<F: FnMut() -> Result<(), String>> Read for CheckedReader<'_, F> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        (self.check)().map_err(std::io::Error::other)?;
        self.file.read(buffer)
    }
}

fn saved_selection(project: &ProjectContext) -> Result<Option<String>, String> {
    match std::fs::symlink_metadata(project.root.join(SAVED_SELECTION_FILE)) {
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("已保存启动场景不可读：{error}")),
    }
    let path = context::checked_existing(&project.root, Path::new(SAVED_SELECTION_FILE), false)?;
    let value = context::read_small_json(&path, SAVED_SELECTION_LIMIT)?;
    if value["format"].as_u64() != Some(1)
        || value["project_id"].as_str() != Some(project.id.as_str())
    {
        return Err("已保存启动场景格式不兼容或不属于当前工程".into());
    }
    // 与作者保存的flatten extra.production_map兼容；不读取旧原型对象布局，不继承32对象门。
    match value.get("production_map") {
        None => Ok(None),
        Some(value) => {
            let id = value.as_str().ok_or("production_map必须是地图ID字符串")?;
            MapId::parse(id)?;
            Ok(Some(id.to_owned()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{fs, io::Write};

    struct Fixture(ProjectContext);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "runtime-startup-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let asset_root = root.join("assets");
            fs::create_dir_all(asset_root.join("maps/SELECTED")).unwrap();
            fs::write(asset_root.join(MAP_CATALOG_ASSET_PATH), serde_json::to_vec(&json!({"schema_version":1,"maps":[
                {"id":"SELECTED","display_name":"已保存图","folder":"maps/SELECTED","published":true},
                {"id":"OTHER","display_name":"未选坏图","folder":"maps/OTHER","published":false}
            ]})).unwrap()).unwrap();
            fs::write(asset_root.join("maps/SELECTED").join(feathrise_map_content::MAP_MANIFEST_FILE_NAME), serde_json::to_vec(&json!({
                "schema_version":1,"id":"SELECTED","display_name":"已保存图","runtime_scene":"DynamicMapScene",
                "world_min":[-32.,-32.],"world_max":[32.,32.],"default_spawn":[0.,2.,6.],"terrains":[],"asset_roots":["scene"]
            })).unwrap()).unwrap();
            Self(ProjectContext {
                root,
                id: "startup-fixture".into(),
                name: "启动回归".into(),
                asset_roots: vec![asset_root],
            })
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let temporary = std::env::temp_dir().canonicalize().unwrap();
            assert!(self.0.root.starts_with(temporary));
            assert!(
                self.0
                    .root
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("runtime-startup-")
            );
            fs::remove_dir_all(&self.0.root).unwrap();
        }
    }

    #[test]
    fn saved_play_accepts_public_and_legacy_runtime_names_without_rewriting_files() {
        let fixture = Fixture::new();
        let path = fixture.0.asset_root().join("maps/SELECTED")
            .join(feathrise_map_content::MAP_MANIFEST_FILE_NAME);
        let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for (name, accepted) in [("data_driven", true), ("DynamicMapScene", true),
            ("KaifengScene", false), ("FutureRuntime", false)] {
            value["runtime_scene"] = name.into();
            let bytes = serde_json::to_vec(&value).unwrap();
            fs::write(&path, &bytes).unwrap();
            assert_eq!(validate_saved_map(&fixture.0, "SELECTED").is_ok(), accepted, "{name}");
            assert_eq!(load_with_map(&fixture.0, None).is_ok(), accepted, "{name}");
            assert_eq!(fs::read(&path).unwrap(), bytes); // F12校验和独立启动均只有读权限语义。
        }
    }

    #[test]
    fn selected_map_startup_does_not_open_unrelated_maps_presets_or_placements() {
        let fixture = Fixture::new();
        // 没有任何预设、placements、portals或OTHER图文件；这些由生产加载器按实际需求处理。
        let path = fixture
            .0
            .asset_root()
            .join("maps/SELECTED")
            .join(feathrise_map_content::MAP_MANIFEST_FILE_NAME);
        fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(&vec![b' '; 2 * 1024 * 1024 + 1])
            .unwrap();
        assert_eq!(
            load_with_map(&fixture.0, None).unwrap().map_id.0,
            "SELECTED"
        );
        assert!(load_with_map(&fixture.0, Some("OTHER".into())).is_err());
        assert!(load_with_map(&fixture.0, Some("ANOTHER-PROJECT".into())).is_err());
    }

    #[test]
    fn saved_selection_is_local_and_override_never_rewrites_it() {
        let fixture = Fixture::new();
        let file = fixture.0.root.join(SAVED_SELECTION_FILE);
        let bytes = serde_json::to_vec(
            &json!({"format":1,"project_id":fixture.0.id,"production_map":"OTHER"}),
        )
        .unwrap();
        fs::write(&file, &bytes).unwrap();
        assert!(load_with_map(&fixture.0, None).is_err());
        assert_eq!(
            load_with_map(&fixture.0, Some("SELECTED".into()))
                .unwrap()
                .map_id
                .0,
            "SELECTED"
        );
        assert_eq!(fs::read(&file).unwrap(), bytes);
        fs::write(&file, b"invalid JSON").unwrap();
        assert!(load_with_map(&fixture.0, None).is_err());
        fs::write(
            &file,
            br#"{"format":1,"project_id":"different-project","production_map":"SELECTED"}"#,
        )
        .unwrap();
        assert!(load_with_map(&fixture.0, None).is_err());
    }

    #[test]
    fn cancelling_large_target_json_can_retry_without_modifying_the_saved_selection() {
        let fixture = Fixture::new();
        let path = fixture
            .0
            .asset_root()
            .join("maps/SELECTED")
            .join(feathrise_map_content::MAP_MANIFEST_FILE_NAME);
        fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(&vec![b' '; 2 * 1024 * 1024 + 1])
            .unwrap();
        let saved = serde_json::to_vec(
            &json!({"format":1,"project_id":fixture.0.id,"production_map":"SELECTED"}),
        )
        .unwrap();
        fs::write(fixture.0.root.join(SAVED_SELECTION_FILE), &saved).unwrap();
        let mut checks = 0;
        let error = validate_saved_map_with_check(&fixture.0, "SELECTED", || {
            checks += 1;
            if checks > 20 {
                Err("目标清单取消回归".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(error.contains("目标清单取消回归"));
        validate_saved_map(&fixture.0, "SELECTED").unwrap();
        assert_eq!(
            fs::read(fixture.0.root.join(SAVED_SELECTION_FILE)).unwrap(),
            saved
        );
    }
}
