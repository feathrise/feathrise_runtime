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
/// 缓存路径必须解析到工程内同名SHA文件；仅后台执行有界读取，拒绝链接逃逸和内容被替换。
pub fn read_blob(project_root: &Path, blob: &DomainBlob) -> Result<Vec<u8>, String> {
    blob.validate()?;
    let root = project_root.canonicalize().map_err(|e| e.to_string())?;
    let path = root
        .join(&blob.path)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !path.starts_with(&root) || !fs::metadata(&path).map_err(|e| e.to_string())?.is_file() {
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
    let root = project_root.canonicalize().map_err(|e| e.to_string())?;
    let directory = root.join(DOMAIN_CACHE).join(domain);
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    if !directory
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(&root)
    {
        return Err("场景领域缓存目录越界".into());
    }
    let blob = DomainBlob {
        path: format!("{DOMAIN_CACHE}/{domain}/{hash}.json"),
        sha256: hash,
        bytes: bytes.len() as u64,
    };
    let target = root.join(&blob.path);
    if target.exists() {
        read_blob(&root, &blob)?;
        return Ok(blob);
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
