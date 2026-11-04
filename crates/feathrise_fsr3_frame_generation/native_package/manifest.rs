//! 可信锁、原生 ABI 与逐文件身份校验；缓存里的 JSON 不能自己充当信任根。

use super::runtime_package;
use super::{NativePackage, TARGET};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path};

pub(super) const BRIDGE: &str = "rpg_fsr3_frame_generation_bridge.lib";
pub(super) const NATIVE_MANIFEST: &str = "native-package.json";
pub(super) const SOURCE_FILES: [&str; 7] = [
    "src/native/fsr3_frame_generation_bridge.cpp",
    "src/native/fsr3_present_images.h",
    "src/native/fsr3_present_submit.h",
    "src/native/fsr3_runtime_identity.h",
    "src/native/fsr3_ui_resources.h",
    "src/native.rs",
    "src/runtime_identity.rs",
];
pub(super) const MAX_ARCHIVE: u64 = 32 * 1024 * 1024;
pub(super) const MAX_EXPANDED: u64 = 64 * 1024 * 1024;
const MAX_JSON: u64 = 256 * 1024;

pub(super) struct Lock {
    pub package_id: String,
    pub sha256: String,
    pub size: u64,
    pub archive: String,
    native_manifest_sha256: String,
}

impl Lock {
    pub fn read(crate_root: &Path, target: &str) -> Result<Self, String> {
        let value = json(&crate_root.join("prebuilt/package.lock.json"))?;
        if value["schema_version"].as_u64() != Some(1)
            || target != TARGET
            || value["target"].as_str() != Some(TARGET)
            || value["crt"].as_str() != Some("dynamic")
        {
            return Err(format!(
                "FSR 预编译包只支持 {TARGET} / dynamic CRT，且必须使用已知锁协议"
            ));
        }
        let archive = string(&value, "archive")?.to_owned();
        if archive != "feathrise-fsr3-fg-windows-x86_64-msvc.zip" {
            return Err("未知 FSR 原生包文件名".into());
        }
        let size = value["size"]
            .as_u64()
            .filter(|s| *s > 0 && *s <= MAX_ARCHIVE)
            .ok_or("FSR ZIP 长度超出固定预算")?;
        Ok(Self {
            package_id: string(&value, "package_id")?.to_owned(),
            sha256: hash(&value, "sha256")?,
            size,
            archive,
            native_manifest_sha256: hash(&value, "native_manifest_sha256")?,
        })
    }
}

pub(super) fn names() -> impl Iterator<Item = &'static str> {
    runtime_package::FILE_NAMES
        .into_iter()
        .chain([BRIDGE, NATIVE_MANIFEST])
}

pub(super) fn inspect(
    directory: &Path,
    crate_root: &Path,
    lock: &Lock,
) -> Result<NativePackage, String> {
    let metadata = fs::symlink_metadata(directory).map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("FSR 缓存不是普通目录".into());
    }
    let entries = fs::read_dir(directory)
        .map_err(|e| e.to_string())?
        .take(9)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if entries.len() != 8 || entries.iter().any(|e| !names().any(|n| e.file_name() == n)) {
        return Err("FSR 原生包必须且只能含固定八文件".into());
    }
    let native_path = directory.join(NATIVE_MANIFEST);
    regular(&native_path)?;
    if runtime_package::sha256(&native_path)? != lock.native_manifest_sha256 {
        return Err("FSR native-package.json 与可信锁 SHA-256 不匹配".into());
    }
    let native = json(&native_path)?;
    if native["schema_version"].as_u64() != Some(1)
        || native["package_id"].as_str() != Some(&lock.package_id)
        || native["target"].as_str() != Some(TARGET)
        || native["crt"].as_str() != Some("dynamic")
        || native["bridge_abi"].as_u64() != Some(1)
    {
        return Err("FSR 原生包版本、目标或桥接 ABI 不匹配".into());
    }
    let files = native["files"]
        .as_object()
        .filter(|m| m.len() == 7)
        .ok_or("FSR 原生包 files 必须恰含六个运行文件与桥接库")?;
    let mut runtime_files = Vec::with_capacity(6);
    for name in names().filter(|name| *name != NATIVE_MANIFEST) {
        let path = directory.join(name);
        let metadata = regular(&path)?;
        let entry = files
            .get(name)
            .ok_or_else(|| format!("FSR 原生包缺少 {name} 身份"))?;
        if entry["size"].as_u64() != Some(metadata.len()) || metadata.len() > MAX_EXPANDED {
            return Err(format!("FSR 原生包 {name} 长度不匹配"));
        }
        let expected = hash(entry, "sha256")?;
        if runtime_package::sha256(&path)? != expected {
            return Err(format!("FSR 原生包 {name} SHA-256 不匹配"));
        }
        if name != BRIDGE {
            runtime_files.push((path, expected));
        }
    }
    let sources = native["source_hashes"]
        .as_object()
        .filter(|m| m.len() == SOURCE_FILES.len())
        .ok_or("FSR 原生包缺少完整桥接源码身份")?;
    for name in SOURCE_FILES {
        let expected = sources
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("缺少桥接源码 {name} 的身份"))?;
        let path = crate_root.join(name);
        let mut source = Vec::new();
        fs::File::open(&path)
            .map_err(|e| e.to_string())?
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut source)
            .map_err(|e| e.to_string())?;
        if source.len() > 2 * 1024 * 1024 {
            return Err("FSR 桥接源码超出预算".into());
        }
        let source = String::from_utf8(source)
            .map_err(|e| e.to_string())?
            .replace("\r\n", "\n");
        let actual: String = Sha256::digest(source.as_bytes())
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect();
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(format!(
                "FSR 预编译桥与当前 {name} 不匹配；维护者须重建并更新配套包，不能使用旧 ABI"
            ));
        }
    }
    let runtime = json(&directory.join("manifest.json"))?;
    runtime_package::validate_manifest(&runtime)?;
    for (index, field) in [
        (0, "dll_sha256"),
        (2, "license_sha256"),
        (3, "patch_sha256"),
        (4, "source_manifest_sha256"),
    ] {
        if !runtime_files[index]
            .1
            .eq_ignore_ascii_case(string(&runtime, field)?)
        {
            return Err(format!("FSR 运行来源 manifest 的 {field} 不匹配"));
        }
    }
    if runtime["dll_size"].as_u64() != Some(regular(&runtime_files[0].0)?.len()) {
        return Err("FSR 运行来源 DLL 长度不匹配".into());
    }
    let bridge_sha256 = hash(&native["files"][BRIDGE], "sha256")?;
    Ok(NativePackage {
        runtime: runtime_package::RuntimePackage {
            files: runtime_files,
        },
        bridge_sha256,
        bridge_library: directory.join(BRIDGE),
        identity: lock.sha256.clone(),
    })
}

pub(super) fn regular(path: &Path) -> Result<fs::Metadata, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!("FSR 包条目不是普通文件：{}", path.display()));
    }
    Ok(metadata)
}
fn json(path: &Path) -> Result<Value, String> {
    if regular(path)?.len() > MAX_JSON {
        return Err("FSR 清单超出固定预算".into());
    }
    serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{}：{e}", path.display()))
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("FSR 清单缺少 {key}"))
}
fn hash(value: &Value, key: &str) -> Result<String, String> {
    let hash = string(value, key)?;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{key} 不是完整 SHA-256"));
    }
    Ok(hash.to_ascii_uppercase())
}
