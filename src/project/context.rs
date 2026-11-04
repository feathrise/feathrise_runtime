//! 工程的不可变路径身份；工程显式提供编译入口，不包含验证DLL的ABI或预检算法。

use bevy::prelude::Resource;
use serde_json::Value;
use std::{
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};

const DESCRIPTOR_LIMIT: u64 = 64 * 1024;
const EXPORT_MARKER: &str = "feathrise-export.json";
const ROOT_LIMIT: usize = 16;

#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct ProjectContext {
    pub root: PathBuf,
    pub id: String,
    pub name: String,
    pub asset_roots: Vec<PathBuf>,
}

impl ProjectContext {
    /// env!(CARGO_MANIFEST_DIR/PACKAGE_NAME)必须在工程main展开后传入，不能在共享库内展开。
    pub fn load(manifest_dir: &Path, package: &str) -> Result<Self, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("工程程序路径不可读：{error}"))?;
        Self::load_with_executable(manifest_dir, package, &executable)
    }

    fn load_with_executable(manifest_dir: &Path, package: &str, executable: &Path) -> Result<Self, String> {
        // 显式导出标记先于编译路径解析；原源码目录被移动或删除后，完整包仍只使用EXE旁的身份与资产。
        // 标记一旦存在便成为当前运行来源，损坏或身份不符不能静默回退开发工程。
        if let Some(project) = Self::from_exported_executable(executable, package)? {
            return Ok(project);
        }
        let compiled = manifest_dir
            .canonicalize()
            .map_err(|error| format!("工程编译目录无效：{error}"))?;
        let manifest = checked_existing(&compiled, Path::new("Cargo.toml"), false)?;
        let mut ancestor = Some(compiled.as_path());
        for _ in 0..16 {
            let Some(root) = ancestor else { break };
            match fs::symlink_metadata(root.join("editor-project.json")) {
                Ok(_) => return Self::from_descriptor(root, Some(&manifest), package),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(format!("工程清单不可读：{error}")),
            }
            ancestor = root.parent();
        }
        Err("工程编译入口的16层祖先内缺少editor-project.json，拒绝使用SDK或启动目录资产".into())
    }

    fn from_exported_executable(executable: &Path, package: &str) -> Result<Option<Self>, String> {
        let root = executable.parent().ok_or("工程程序缺少父目录")?
            .canonicalize().map_err(|error| format!("工程程序目录不可读：{error}"))?;
        match fs::symlink_metadata(root.join(EXPORT_MARKER)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("工程导出标记不可读：{error}")),
            Ok(_) => (),
        }
        let marker = checked_existing(&root, Path::new(EXPORT_MARKER), false)?;
        let marker = read_small_json(&marker, DESCRIPTOR_LIMIT)?;
        if marker["format"].as_u64() != Some(1) || marker["kind"] != "feathrise-export" {
            return Err("工程导出标记必须声明feathrise-export v1".into());
        }
        let expected_id = text(&marker, "project_id", 128)?;
        let project = Self::from_descriptor(&root, None, package)?;
        if project.id != expected_id {
            return Err("工程导出标记与运行描述的工程身份不一致".into());
        }
        Ok(Some(project))
    }

    pub fn asset_root(&self) -> &Path {
        &self.asset_roots[0]
    }

    fn from_descriptor(root: &Path, manifest: Option<&Path>, package: &str) -> Result<Self, String> {
        let path = checked_existing(root, Path::new("editor-project.json"), false)?;
        let value = read_small_json(&path, DESCRIPTOR_LIMIT)?;
        // 正式入口使用工程格式和saved-world能力版号；旧验证DLL的sdk_abi字段不支配游戏代码装配。
        if value["format"].as_u64() != Some(2)
            || value["game_entry"]["kind"] != "saved-world"
            || value["game_entry"]["version"].as_u64() != Some(1)
        {
            return Err("工程必须声明v2清单和saved-world v1正式运行入口".into());
        }
        let id = text(&value, "id", 128)?;
        let name = text(&value, "name", 256)?;
        if text(&value, "package", 128)? != package {
            return Err("最近工程清单的package不属于调用方工程包".into());
        }
        let cargo = Path::new(text(&value, "cargo_manifest", 1024)?);
        // 导出包仍须保留合法的入口元数据，但不分发或读取Cargo源码；开发入口继续核对实际清单归属。
        validate_relative(cargo, false)?;
        if let Some(manifest) = manifest {
            if checked_existing(root, cargo, false)? != manifest {
                return Err("最近工程清单不拥有调用方Cargo入口".into());
            }
        }
        let paths = value["asset_roots"]
            .as_array()
            .ok_or("工程缺少asset_roots数组")?;
        if paths.is_empty() || paths.len() > ROOT_LIMIT {
            return Err("工程必须明确登记1至16个资产根".into());
        }
        let data = checked_directory(root, Path::new(text(&value, "editor_data", 1024)?), false)?;
        let mut asset_roots: Vec<PathBuf> = Vec::with_capacity(paths.len());
        for relative in paths {
            let root = checked_directory(
                root,
                Path::new(relative.as_str().ok_or("资产根必须是相对目录字符串")?),
                true,
            )?;
            if contains(&root, &data) || contains(&data, &root) {
                return Err("资产根与作者数据目录不得相互包含".into());
            }
            if asset_roots
                .iter()
                .any(|previous| contains(previous, &root) && contains(&root, previous))
            {
                return Err("工程资产根重复".into());
            }
            asset_roots.push(root);
        }
        Ok(Self {
            root: root.to_owned(),
            id: id.to_owned(),
            name: name.to_owned(),
            asset_roots,
        })
    }
}

fn text<'a>(value: &'a Value, key: &str, maximum: usize) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .filter(|text| {
            !text.trim().is_empty() && text.len() <= maximum && !text.chars().any(char::is_control)
        })
        .ok_or_else(|| format!("工程字段{key}必须是{maximum}字节内的非空普通文本"))
}

pub(crate) fn read_small_json(path: &Path, limit: u64) -> Result<Value, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !ordinary(&metadata) || !metadata.is_file() || metadata.len() > limit {
        return Err(format!(
            "配置必须是{limit}字节内的普通JSON文件：{}",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|error| error.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("配置读取期间超过约定容量".into());
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("配置JSON无效：{}：{error}", path.display()))
}

fn validate_relative(relative: &Path, allow_empty: bool) -> Result<(), String> {
    if !allow_empty && relative.as_os_str().is_empty() {
        return Err("工程相对路径不能为空".into());
    }
    for part in relative.components() {
        let Component::Normal(name) = part else {
            return Err("工程路径不能含绝对路径、父跳转或当前目录片段".into());
        };
        let name = name.to_str().ok_or("工程路径必须是UTF-8名称")?;
        if name.contains(':') || name.ends_with(['.', ' ']) || name.chars().any(char::is_control) {
            return Err("工程路径包含不安全名称".into());
        }
    }
    Ok(())
}

pub(crate) fn ordinary(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    !metadata.file_type().is_symlink()
}

fn checked_directory(root: &Path, relative: &Path, required: bool) -> Result<PathBuf, String> {
    validate_relative(relative, false)?;
    let mut path = root.to_owned();
    for part in relative.components() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if ordinary(&metadata) && metadata.is_dir() => (),
            Ok(_) => return Err("工程目录不能经过文件、符号链接或junction".into()),
            Err(error) if !required && error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(format!("工程目录不可访问：{}：{error}", path.display())),
        }
    }
    Ok(path)
}

pub(crate) fn checked_existing(
    root: &Path,
    relative: &Path,
    directory: bool,
) -> Result<PathBuf, String> {
    validate_relative(relative, directory)?;
    let metadata = fs::symlink_metadata(root).map_err(|error| error.to_string())?;
    if !ordinary(&metadata)
        || !metadata.is_dir()
        || root.canonicalize().map_err(|error| error.to_string())? != root
    {
        return Err("工程根真实路径已变化或不再是普通目录".into());
    }
    let mut path = root.to_owned();
    for part in relative.components() {
        path.push(part);
        if !ordinary(
            &fs::symlink_metadata(&path)
                .map_err(|error| format!("工程文件不可访问：{}：{error}", path.display()))?,
        ) {
            return Err("工程文件不能经过符号链接或junction".into());
        }
    }
    let canonical = path.canonicalize().map_err(|error| error.to_string())?;
    if !canonical.starts_with(root)
        || (directory && !canonical.is_dir())
        || (!directory && !canonical.is_file())
    {
        return Err("工程文件不属于当前登记根".into());
    }
    Ok(canonical)
}

fn contains(parent: &Path, child: &Path) -> bool {
    #[cfg(windows)]
    {
        let parent: Vec<_> = parent
            .components()
            .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
            .collect();
        let child: Vec<_> = child
            .components()
            .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
            .collect();
        child.starts_with(&parent)
    }
    #[cfg(not(windows))]
    child.starts_with(parent)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ExportFixture(PathBuf);

    impl ExportFixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "runtime-export-context-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            ));
            fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }

        fn descriptor(&self, root: &Path) -> Value {
            fs::create_dir_all(root.join("assets")).unwrap();
            fs::create_dir_all(root.join("secondary/textures")).unwrap();
            let value = serde_json::json!({
                "format":2,"id":"export-fixture","name":"独立导出 工程","package":"game_fixture",
                "cargo_manifest":"source/Cargo.toml","asset_roots":["assets","secondary"],
                "editor_data":".editor","game_entry":{"kind":"saved-world","version":1},
                "game_view":"fixed_top_down",
            });
            fs::write(root.join("editor-project.json"), serde_json::to_vec(&value).unwrap()).unwrap();
            value
        }

        fn exported(&self, root: &Path) {
            self.descriptor(root);
            fs::write(root.join(EXPORT_MARKER), serde_json::to_vec(&serde_json::json!({
                "format":1,"kind":"feathrise-export","project_id":"export-fixture",
                "manifest":"export-manifest.json",
            })).unwrap()).unwrap();
        }

        fn developer(&self, root: &Path) -> PathBuf {
            self.descriptor(root);
            fs::create_dir(root.join("source")).unwrap();
            fs::write(root.join("source/Cargo.toml"), b"[package]\nname='game_fixture'\nversion='0.1.0'\n").unwrap();
            root.join("source")
        }
    }

    impl Drop for ExportFixture {
        fn drop(&mut self) {
            let temporary = std::env::temp_dir().canonicalize().unwrap();
            assert!(self.0.starts_with(temporary));
            assert!(self.0.file_name().unwrap().to_string_lossy().starts_with("runtime-export-context-"));
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn exported_context_uses_program_directory_without_source_or_cargo() {
        let fixture = ExportFixture::new();
        let output = fixture.0.join("独立包 中文");
        fixture.exported(&output);
        fs::write(output.join("assets/same.json"), b"primary").unwrap();
        fs::write(output.join("secondary/same.json"), b"secondary").unwrap();
        let context = ProjectContext::load_with_executable(
            &fixture.0.join("already-removed-source"), "game_fixture", &output.join("工程.exe"),
        ).unwrap();
        assert_eq!(context.root, output);
        assert_eq!(context.id, "export-fixture");
        assert_eq!(context.asset_roots, [output.join("assets"), output.join("secondary")]);
        assert_eq!(crate::project::GameView::load(&context).unwrap(), crate::project::GameView::FixedTopDown);
        assert_eq!(fs::read(context.asset_root().join("same.json")).unwrap(), b"primary");
        assert_eq!(fs::read(context.asset_roots[1].join("same.json")).unwrap(), b"secondary");
        assert!(!output.join("source").exists());
    }

    #[test]
    fn absent_export_marker_preserves_developer_cargo_identity() {
        let fixture = ExportFixture::new();
        let developer = fixture.0.join("developer");
        let compiled = fixture.developer(&developer);
        let binary = fixture.0.join("binary");
        fs::create_dir(&binary).unwrap();
        let context = ProjectContext::load_with_executable(&compiled, "game_fixture", &binary.join("game.exe")).unwrap();
        assert_eq!(context.root, developer);
        assert!(ProjectContext::load_with_executable(&compiled, "other_package", &binary.join("game.exe")).is_err());
        assert!(ProjectContext::load_with_executable(&fixture.0.join("absent-source"), "game_fixture", &binary.join("game.exe")).is_err());
    }

    #[test]
    fn invalid_export_marker_cannot_fall_back_to_the_existing_source_project() {
        let fixture = ExportFixture::new();
        let developer = fixture.0.join("developer");
        let compiled = fixture.developer(&developer);
        let output = fixture.0.join("output");
        fixture.exported(&output);
        let invalid = [
            b"invalid JSON".as_slice(),
            br#"{"format":2,"kind":"feathrise-export","project_id":"export-fixture"}"#,
            br#"{"format":1,"kind":"other","project_id":"export-fixture"}"#,
            br#"{"format":1,"kind":"feathrise-export","project_id":"another-project"}"#,
            br#"{"format":1,"kind":"feathrise-export","project_id":""}"#,
        ];
        for bytes in invalid {
            fs::write(output.join(EXPORT_MARKER), bytes).unwrap();
            assert!(ProjectContext::load_with_executable(&compiled, "game_fixture", &output.join("game.exe")).is_err());
        }
    }

    #[test]
    fn exported_descriptor_keeps_package_and_registered_root_validation() {
        let fixture = ExportFixture::new();
        let output = fixture.0.join("output");
        fixture.exported(&output);
        let descriptor = fixture.descriptor(&output);
        for (field, value) in [
            ("asset_roots", serde_json::json!(["../elsewhere"])),
            ("asset_roots", serde_json::json!(["missing-assets"])),
            ("asset_roots", serde_json::json!(["assets", "assets"])),
            ("package", serde_json::json!("other-package")),
            ("cargo_manifest", serde_json::json!("../outside/Cargo.toml")),
            ("game_entry", serde_json::json!({"kind":"saved-world","version":2})),
        ] {
            let mut invalid = descriptor.clone();
            invalid[field] = value;
            fs::write(output.join("editor-project.json"), serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert!(ProjectContext::load_with_executable(&fixture.0.join("absent"), "game_fixture", &output.join("game.exe")).is_err());
        }
    }

    #[test]
    fn exported_program_uses_its_moved_package_after_the_source_directory_is_removed() {
        let fixture = ExportFixture::new();
        let developer = fixture.0.join("developer");
        let compiled = fixture.developer(&developer);
        let output = fixture.0.join("output");
        fixture.exported(&output);
        fs::write(output.join("assets/probe.json"), b"independent-assets").unwrap();
        // 两个目标均由本夹具独占，删除开发源码和移动整个包，模拟交付后的目录身份变化。
        assert!(developer.starts_with(&fixture.0));
        fs::remove_dir_all(&developer).unwrap();
        let moved = fixture.0.join("moved 独立包");
        fs::rename(&output, &moved).unwrap();
        let context = ProjectContext::load_with_executable(&compiled, "game_fixture", &moved.join("game.exe")).unwrap();
        assert_eq!(context.root, moved);
        assert_eq!(fs::read(context.asset_root().join("probe.json")).unwrap(), b"independent-assets");
        assert!(!compiled.exists());
    }

    #[test]
    fn exported_marker_must_be_a_bounded_regular_json_file() {
        let fixture = ExportFixture::new();
        let output = fixture.0.join("output");
        fixture.exported(&output);
        let marker = output.join(EXPORT_MARKER);
        fs::write(&marker, vec![b' '; DESCRIPTOR_LIMIT as usize + 1]).unwrap();
        assert!(ProjectContext::load_with_executable(&fixture.0.join("absent"), "game_fixture", &output.join("game.exe")).is_err());
        fs::remove_file(&marker).unwrap();
        fs::create_dir(&marker).unwrap();
        assert!(ProjectContext::load_with_executable(&fixture.0.join("absent"), "game_fixture", &output.join("game.exe")).is_err());
    }

    #[test]
    fn explicit_caller_identity_is_not_the_shared_sdk_or_a_nearby_project() {
        let root = std::env::temp_dir().join(format!(
            "runtime-context-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        for name in ["A", "B"] {
            let project = root.join(name);
            fs::create_dir_all(project.join("assets")).unwrap();
            fs::write(
                project.join("Cargo.toml"),
                b"[package]\nname='game_fixture'\nversion='0.1.0'\n",
            )
            .unwrap();
            fs::write(project.join("editor-project.json"), serde_json::to_vec(&serde_json::json!({
                "format":2,"id":name,"name":name,"package":"game_fixture","cargo_manifest":"Cargo.toml",
                "asset_roots":["assets"],"editor_data":".editor","game_entry":{"kind":"saved-world","version":1}
            })).unwrap()).unwrap();
        }
        let a = ProjectContext::load(&root.join("A"), "game_fixture").unwrap();
        let b = ProjectContext::load(&root.join("B"), "game_fixture").unwrap();
        assert_ne!(a.root, b.root);
        assert!(a.asset_root().starts_with(&a.root));
        assert!(ProjectContext::load(&root.join("A"), "wrong_package").is_err());
        assert!(checked_existing(a.asset_root(), Path::new("../Cargo.toml"), false).is_err());
        let temporary = std::env::temp_dir().canonicalize().unwrap();
        assert!(root.starts_with(temporary));
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("runtime-context-")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
