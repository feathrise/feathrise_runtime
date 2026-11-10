//! 已保存工程的轻量游戏入口：只读场景目录和所选清单，不扫描全部场景摆放或运行作者审计。

use crate::project::{ProjectContext, context};
use bevy::prelude::*;
use feathrise_scene_content::{
    MAX_SCENE_STARTUP_BYTES, SCENE_CATALOG_ASSET_PATH, SCENE_SCHEMA_VERSION,
    SCENE_STARTUP_FILE_NAME, SceneCatalogAsset, SceneId, SceneManifestAsset, SceneStartup,
};
#[cfg(feature = "validation")]
use feathrise_scene_content::SceneCatalogEntry;
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

#[derive(Resource)]
pub(crate) struct SavedGameStartup {
    pub scene_id: SceneId,
    pub name: String,
    pub spawn: Vec3,
    // 仅显式运行验收需要目录条目核对来源；正式启动只保留场景身份、名称与出生点。
    #[cfg(feature = "validation")]
    pub entry: SceneCatalogEntry,
}

/// 空工程可以启动空窗口；只有没有场景也没有显式选择时才为空，坏启动项仍报告错误。
pub(crate) fn load_optional(
    project: &ProjectContext,
    saved_play: bool,
) -> Result<Option<SavedGameStartup>, String> {
    let requested = saved_play
        .then(|| std::env::var("EDITOR_SAVED_PLAY_SCENE").ok())
        .flatten();
    resolve_startup(project, requested.as_deref(), &mut || Ok(()))
}

/// 导出在后台核对独立程序将使用的已保存启动场景，空工程同样是有效结果。
pub fn validate_saved_startup_with_check(
    project: &ProjectContext,
    mut check: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    resolve_startup(project, None, &mut check).map(|_| ())
}

/// 宿主后台试玩复用游戏自己的目标校验；不读另一场景或预设，不写入作者文件。
pub fn validate_saved_scene(project: &ProjectContext, scene_id: &str) -> Result<(), String> {
    validate_saved_scene_with_check(project, scene_id, || Ok(()))
}

/// 目标清单可能很大；调用方的取消牌在每64KiB文件读取与目录身份检查之间生效。
pub fn validate_saved_scene_with_check(
    project: &ProjectContext,
    scene_id: &str,
    mut check: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    resolve_startup(project, Some(scene_id), &mut check).map(|_| ())
}

fn resolve_startup(
    project: &ProjectContext,
    requested: Option<&str>,
    check: &mut impl FnMut() -> Result<(), String>,
) -> Result<Option<SavedGameStartup>, String> {
    check()?;
    let catalog = read_catalog(project, check)?;
    check()?;
    // 显式试玩只覆盖选择，工程中存在的启动文档仍必须有效。
    let saved = saved_selection(project)?;
    check()?;
    let entry = match requested.or(saved.as_ref().and_then(SceneStartup::selected)) {
        Some(id) => {
            SceneId::parse(id)?;
            catalog
                .scenes
                .into_iter()
                .find(|scene| scene.id.as_str() == id)
                .ok_or("已保存启动场景不属于本工程场景目录")?
        }
        None if catalog.scenes.is_empty() => return Ok(None),
        None => catalog
            .scenes
            .into_iter()
            .find(|scene| scene.published)
            .ok_or("工程没有已保存启动场景，请先创建/保存场景")?,
    };
    let path = context::checked_existing(
        project.asset_root(),
        Path::new(&entry.manifest_path()),
        false,
    )?;
    let manifest: SceneManifestAsset = read_json_checked(&path, check)?;
    if manifest.id != entry.id
        || manifest.schema_version != SCENE_SCHEMA_VERSION
        || !manifest.is_data_driven()
    {
        return Err("启动场景清单身份或版本不一致".into());
    }
    if !manifest.default_spawn.iter().all(|value| value.is_finite()) {
        return Err("启动场景起始位置包含非有限数值".into());
    }
    check()?;
    Ok(Some(SavedGameStartup {
        scene_id: entry.id.clone(),
        name: manifest.display_name,
        spawn: Vec3::from_array(manifest.default_spawn),
        #[cfg(feature = "validation")]
        entry,
    }))
}

fn read_catalog(
    project: &ProjectContext,
    check: &mut impl FnMut() -> Result<(), String>,
) -> Result<SceneCatalogAsset, String> {
    let path = context::checked_existing(
        project.asset_root(),
        Path::new(SCENE_CATALOG_ASSET_PATH),
        false,
    )?;
    let catalog: SceneCatalogAsset = read_json_checked(&path, check)?;
    if catalog.schema_version != SCENE_SCHEMA_VERSION {
        return Err("正式场景目录版本不兼容".into());
    }
    let mut ids = BTreeSet::new();
    for entry in &catalog.scenes {
        check()?;
        SceneId::parse(&entry.id.0)?;
        if !ids.insert(entry.id.0.as_str()) {
            return Err(format!("重复场景ID：{}", entry.id));
        }
    }
    Ok(catalog)
}

/// 沿用生产流式JSON/BOM语义，只在文件流下层加取消检查；不复制清单或维护另一份场景校验。
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

fn saved_selection(project: &ProjectContext) -> Result<Option<SceneStartup>, String> {
    match std::fs::symlink_metadata(project.root.join(SCENE_STARTUP_FILE_NAME)) {
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("已保存启动场景不可读：{error}")),
    }
    let path = context::checked_existing(&project.root, Path::new(SCENE_STARTUP_FILE_NAME), false)?;
    let bytes = context::read_small_file(&path, MAX_SCENE_STARTUP_BYTES as u64)?;
    SceneStartup::parse(&bytes, &project.id).map(Some)
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
            fs::create_dir_all(asset_root.join("scenes/SELECTED")).unwrap();
            fs::write(asset_root.join(SCENE_CATALOG_ASSET_PATH), serde_json::to_vec(&json!({"schema_version":2,"scenes":[
                {"id":"SELECTED","display_name":"已保存图","folder":"scenes/SELECTED","published":true},
                {"id":"OTHER","display_name":"未选坏图","folder":"scenes/OTHER","published":false}
            ]})).unwrap()).unwrap();
            fs::write(asset_root.join("scenes/SELECTED").join(feathrise_scene_content::SCENE_MANIFEST_FILE_NAME), serde_json::to_vec(&json!({
                "schema_version":2,"id":"SELECTED","display_name":"已保存图","runtime_scene":"data_driven",
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
    fn saved_play_accepts_scene_runtime_and_rejects_legacy_names_without_rewriting() {
        let fixture = Fixture::new();
        let path = fixture.0.asset_root().join("scenes/SELECTED")
            .join(feathrise_scene_content::SCENE_MANIFEST_FILE_NAME);
        let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for (name, accepted) in [("data_driven", true), ("DynamicMapScene", false),
            ("KaifengScene", false), ("FutureRuntime", false)] {
            value["runtime_scene"] = name.into();
            let bytes = serde_json::to_vec(&value).unwrap();
            fs::write(&path, &bytes).unwrap();
            assert_eq!(validate_saved_scene(&fixture.0, "SELECTED").is_ok(), accepted, "{name}");
            assert_eq!(load_optional(&fixture.0, false).is_ok(), accepted, "{name}");
            assert_eq!(fs::read(&path).unwrap(), bytes); // F12校验和独立启动均只有读权限语义。
        }
    }

    #[test]
    fn selected_scene_startup_does_not_open_unrelated_scenes_presets_or_placements() {
        let fixture = Fixture::new();
        // 没有任何预设、placements、portals或OTHER图文件；这些由生产加载器按实际需求处理。
        let path = fixture
            .0
            .asset_root()
            .join("scenes/SELECTED")
            .join(feathrise_scene_content::SCENE_MANIFEST_FILE_NAME);
        fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(&vec![b' '; 2 * 1024 * 1024 + 1])
            .unwrap();
        assert_eq!(
            load_optional(&fixture.0, false).unwrap().unwrap().scene_id.0,
            "SELECTED"
        );
        assert!(validate_saved_scene(&fixture.0, "OTHER").is_err());
        assert!(validate_saved_scene(&fixture.0, "ANOTHER-PROJECT").is_err());
    }

    #[test]
    fn saved_selection_is_local_and_override_never_rewrites_it() {
        let fixture = Fixture::new();
        let file = fixture.0.root.join(SCENE_STARTUP_FILE_NAME);
        let bytes = serde_json::to_vec(
            &json!({"format":2,"project_id":fixture.0.id,"revision":1,"startup_scene_id":"OTHER"}),
        )
        .unwrap();
        fs::write(&file, &bytes).unwrap();
        assert!(load_optional(&fixture.0, false).is_err());
        validate_saved_scene(&fixture.0, "SELECTED").unwrap();
        assert_eq!(fs::read(&file).unwrap(), bytes);
        fs::write(&file, b"invalid JSON").unwrap();
        assert!(load_optional(&fixture.0, false).is_err());
        assert!(validate_saved_scene(&fixture.0, "SELECTED").is_err());
        fs::write(
            &file,
            br#"{"format":2,"project_id":"different-project","revision":1,"startup_scene_id":"SELECTED"}"#,
        )
        .unwrap();
        assert!(load_optional(&fixture.0, false).is_err());
    }

    #[test]
    fn saved_selection_rejects_duplicate_ids_and_previous_document_versions() {
        let fixture = Fixture::new();
        let file = fixture.0.root.join(SCENE_STARTUP_FILE_NAME);
        for document in [
            br#"{"format":2,"project_id":"startup-fixture","revision":1,"startup_scene_id":"OTHER","startup_scene_id":"SELECTED"}"#.as_slice(),
            br#"{"format":1,"project_id":"startup-fixture","revision":1,"startup_scene_id":"SELECTED"}"#,
        ] {
            fs::write(&file, document).unwrap();
            assert!(load_optional(&fixture.0, false).is_err());
            assert_eq!(fs::read(&file).unwrap(), document);
        }
    }

    #[test]
    fn export_startup_validation_matches_runtime_without_a_selection() {
        let fixture = Fixture::new();
        let project = &fixture.0;
        let path = project.asset_root().join(SCENE_CATALOG_ASSET_PATH);
        let mut catalog: SceneCatalogAsset = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        catalog.scenes.swap(0, 1); // 先遇到没有正式清单的草稿，也必须选随后发布的场景。
        fs::write(&path, serde_json::to_vec(&catalog).unwrap()).unwrap();
        validate_saved_startup_with_check(project, || Ok(())).unwrap();
        assert_eq!(
            load_optional(project, false).unwrap().unwrap().scene_id.as_str(),
            "SELECTED"
        );

        catalog.scenes.truncate(1);
        fs::write(&path, serde_json::to_vec(&catalog).unwrap()).unwrap();
        assert!(validate_saved_startup_with_check(project, || Ok(())).is_err());
        assert!(load_optional(project, false).is_err());

        catalog.scenes.clear();
        fs::write(&path, serde_json::to_vec(&catalog).unwrap()).unwrap();
        validate_saved_startup_with_check(project, || Ok(())).unwrap();
        assert!(load_optional(project, false).unwrap().is_none());

        fs::write(&path, br#"{"schema_version":1,"maps":[]}"#).unwrap();
        assert!(validate_saved_startup_with_check(project, || Ok(())).is_err());
        assert!(load_optional(project, false).is_err());
    }

    #[test]
    fn cancelling_large_target_json_can_retry_without_modifying_the_saved_selection() {
        let fixture = Fixture::new();
        let path = fixture
            .0
            .asset_root()
            .join("scenes/SELECTED")
            .join(feathrise_scene_content::SCENE_MANIFEST_FILE_NAME);
        fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(&vec![b' '; 2 * 1024 * 1024 + 1])
            .unwrap();
        let saved = serde_json::to_vec(
            &json!({"format":2,"project_id":fixture.0.id,"revision":1,"startup_scene_id":"SELECTED"}),
        )
        .unwrap();
        fs::write(fixture.0.root.join(SCENE_STARTUP_FILE_NAME), &saved).unwrap();
        let mut checks = 0;
        let error = validate_saved_scene_with_check(&fixture.0, "SELECTED", || {
            checks += 1;
            if checks > 20 {
                Err("目标清单取消回归".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(error.contains("目标清单取消回归"));
        validate_saved_scene(&fixture.0, "SELECTED").unwrap();
        assert_eq!(
            fs::read(fixture.0.root.join(SCENE_STARTUP_FILE_NAME)).unwrap(),
            saved
        );
    }
}
