//! 无窗口工具和编辑器共用的作者锁、路径边界、流式摘要与JSON读取。

use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};

/// 所有角色预设及地图角色作者操作共享同一个资产根锁，避免引用检查与写入竞态。
pub struct ContentAuthorLock {
    _file: File,
}

impl ContentAuthorLock {
    pub fn acquire(asset_root: &Path) -> Result<Self, String> {
        let path = content_author_lock_path(asset_root)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("打开作者锁失败：{error}"))?;
        file.try_lock()
            .map_err(|error| format!("另一作者正在修改地图或角色预设，请稍后重试：{error}"))?;
        Ok(Self { _file: file })
    }
}

pub fn content_author_lock_path(asset_root: &Path) -> Result<PathBuf, String> {
    let canonical = asset_root
        .canonicalize()
        .map_err(|error| format!("资产根不可用：{error}"))?;
    let key = asset_path_collision_key(&canonical);
    Ok(std::env::temp_dir().join(format!(
        "bevy-content-author-{}.lock",
        bytes_hash(key.as_bytes())
    )))
}

/// 先验证词法路径，再逐级检查已存在父路径，拒绝符号链接越出资产根。
pub fn safe_content_path(asset_root: &Path, relative: &str) -> Result<PathBuf, String> {
    let root = asset_root
        .canonicalize()
        .map_err(|error| format!("资产根不可用：{error}"))?;
    let path = Path::new(relative);
    if path.is_absolute()
        || relative.contains(['\\', '\0'])
        || relative.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || part.contains(':')
                || part.ends_with(['.', ' '])
        })
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!("作者路径必须位于assets内：{relative}"));
    }
    let mut candidate = root.clone();
    for component in path.components() {
        candidate.push(component);
        if candidate.exists()
            && !candidate
                .canonicalize()
                .map_err(|error| error.to_string())?
                .starts_with(&root)
        {
            return Err(format!("作者路径通过链接越出assets：{relative}"));
        }
    }
    Ok(candidate)
}

pub fn asset_path_collision_key(path: &Path) -> String {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                None | Some(Component::ParentDir) if !path.has_root() => normalized.push(".."),
                _ => {} // 绝对路径不能越过盘符或根目录。
            },
            other => normalized.push(other.as_os_str()),
        }
    }
    let text = normalized
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase();
    if let Some(unc) = text.strip_prefix("//?/unc/") {
        format!("//{unc}") // Windows canonicalize的UNC扩展拼写与普通UNC指向同一目标。
    } else {
        text.strip_prefix("//?/").unwrap_or(&text).to_owned() // 规范化盘符扩展前缀，避免公共服务与旧root/../../资产路径绕过去重。
    }
}

pub fn bytes_hash(bytes: &[u8]) -> String {
    hex(Sha256::digest(bytes).as_slice())
}

pub fn file_hash(path: &Path) -> Result<String, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok("missing".into()),
        Err(error) => return Err(format!("读取{}失败：{error}", path.display())),
    };
    let mut hasher = Sha256::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
    }
    Ok(hex(hasher.finalize().as_slice()))
}

struct HashWriter(Sha256);

impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn content_hash(value: &(impl Serialize + ?Sized)) -> Result<String, String> {
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, value).map_err(|error| error.to_string())?;
    Ok(hex(writer.0.finalize().as_slice()))
}

pub fn pretty_json_hash(value: &(impl Serialize + ?Sized)) -> Result<String, String> {
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer_pretty(&mut writer, value).map_err(|error| error.to_string())?;
    writer.write_all(b"\n").map_err(|error| error.to_string())?;
    Ok(hex(writer.0.finalize().as_slice()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn read_json_file<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let file = File::open(path).map_err(|error| format!("读取{}失败：{error}", path.display()))?;
    let mut reader = BufReader::new(file);
    if reader
        .fill_buf()
        .map_err(|error| error.to_string())?
        .starts_with(&[0xef, 0xbb, 0xbf])
    {
        reader.consume(3);
    }
    serde_json::from_reader(reader).map_err(|error| format!("解析{}失败：{error}", path.display()))
}

pub fn compare_content_hash(expected: &str, actual: &str) -> Result<(), String> {
    if expected == actual {
        Ok(())
    } else {
        Err(format!(
            "版本冲突：期望{expected}，磁盘为{actual}；请保留草稿后重新加载"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authoring_keys_merge_absolute_and_parent_component_spellings() {
        let root = std::env::temp_dir();
        let plain = root.join("maps/a/characters.map_characters.json");
        let alternate = root.join("maps/other/../a/./characters.map_characters.json");
        assert_eq!(
            asset_path_collision_key(&plain),
            asset_path_collision_key(&alternate)
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_extended_paths_share_the_same_transaction_target_key() {
        assert_eq!(
            asset_path_collision_key(Path::new(r"\\?\C:\workspace\assets\maps\a.json")),
            asset_path_collision_key(Path::new(r"C:\workspace\assets\maps\a.json"))
        );
        assert_eq!(
            asset_path_collision_key(Path::new(r"\\?\UNC\server\share\a.json")),
            asset_path_collision_key(Path::new(r"\\server\share\a.json"))
        );
    }
}
