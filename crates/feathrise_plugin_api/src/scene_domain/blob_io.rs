//! 场景作者跨库共享的有界不可变blob读写；只允许在后台调用，不依赖Bevy或编辑器源码。
use super::{DomainBlob, DOMAIN_CACHE, MAX_DOMAIN_BLOB_BYTES};
use sha2::{Digest, Sha256};
use std::{fs, io::{Read, Write}, path::Path};
#[cfg(test)]
mod tests;
fn bytes_hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}
fn next_generation() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    metadata.is_symlink()
}

/// 逐级确认普通目录再创建下一级，避免重定向的 target 或旧缓存接收新的交换文件。
fn check_cache_directory(root: &Path, relative: &Path, create: bool) -> Result<(), String> {
    let mut path = root.to_path_buf();
    for component in relative.components() {
        path.push(component);
        let metadata = match fs::symlink_metadata(&path) {
            Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                match fs::create_dir(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.to_string()),
                }
                fs::symlink_metadata(&path)
            }
            metadata => metadata,
        }
        .map_err(|e| e.to_string())?;
        if !metadata.is_dir() || is_link(&metadata) {
            return Err("场景领域缓存目录不属于当前工程".into());
        }
    }
    Ok(())
}

/// 新旧缓存都要求工程内同名 SHA 文件，拒绝链接和替换后的内容。只在后台读取有界正文。
pub fn read_blob(project_root: &Path, blob: &DomainBlob) -> Result<Vec<u8>, String> {
    blob.validate()?;
    let root = project_root.canonicalize().map_err(|e| e.to_string())?;
    let relative = Path::new(&blob.path);
    check_cache_directory(&root, relative.parent().ok_or("场景领域缓存缺少父目录")?, false)?;
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || is_link(&metadata) {
        return Err("场景领域缓存不属于当前工程".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_DOMAIN_BLOB_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != blob.bytes
        || bytes.len() as u64 > MAX_DOMAIN_BLOB_BYTES
        || bytes_hash(&bytes) != blob.sha256
    {
        return Err("场景领域缓存大小或SHA-256与回执不一致".into());
    }
    Ok(bytes)
}
pub fn write_blob(project_root: &Path, domain: &str, bytes: &[u8]) -> Result<DomainBlob, String> {
    if domain.is_empty()
        || !domain
            .bytes()
            .all(|v| v.is_ascii_alphanumeric() || b"._-".contains(&v))
        || bytes.len() as u64 > MAX_DOMAIN_BLOB_BYTES
    {
        return Err("场景领域缓存身份或大小无效".into());
    }
    let hash = bytes_hash(bytes);
    let blob = DomainBlob {
        path: format!("{DOMAIN_CACHE}/{domain}/{hash}.json"),
        sha256: hash,
        bytes: bytes.len() as u64,
    };
    // 先核对完整回执再触碰文件系统，拒绝 "." 和 ".." 等会改写缓存根的领域名称。
    blob.validate()?;
    let root = project_root.canonicalize().map_err(|e| e.to_string())?;
    let relative = Path::new(DOMAIN_CACHE).join(domain);
    check_cache_directory(&root, &relative, true)?;
    let directory = root.join(relative);
    let target = root.join(&blob.path);
    match fs::symlink_metadata(&target) {
        Ok(_) => {
            read_blob(&root, &blob)?;
            return Ok(blob);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    // SHA路径只发布完整内容；中断只能留下独占stage，不能把半个文件误当不可变历史版本。
    let stage = directory.join(format!(
        ".{}-{}-{}.stage",
        blob.sha256,
        std::process::id(),
        next_generation()
    ));
    let result = (|| -> Result<(), String> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage)
            .map_err(|e| e.to_string())?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        match fs::rename(&stage, &target) {
            Ok(()) => Ok(()),
            Err(_) if target.exists() => read_blob(&root, &blob).map(|_| ()),
            Err(error) => Err(error.to_string()),
        }
    })();
    if stage.exists() {
        let _ = fs::remove_file(&stage);
    }
    result?;
    Ok(blob)
}
