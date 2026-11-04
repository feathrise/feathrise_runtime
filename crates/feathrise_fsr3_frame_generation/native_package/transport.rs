//! 有界复制与严格平铺解包；原生归档始终来自显式本地路径。

use super::manifest::{self, Lock, MAX_ARCHIVE, MAX_EXPANDED};
use super::runtime_package;
use std::{
    collections::HashSet,
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

pub(super) fn copy_archive(source: &Path, destination: &Path, lock: &Lock) -> Result<(), String> {
    let metadata = manifest::regular(source).map_err(|error| {
        format!(
            "无法读取 FEATHRISE_FSR3_PACKAGE {}：{error}",
            source.display()
        )
    })?;
    if metadata.len() != lock.size {
        return Err("FSR 本地 ZIP 长度与可信锁不匹配".into());
    }
    let mut input = fs::File::open(source).map_err(|e| e.to_string())?;
    stream(&mut input, destination, lock.size)?;
    verify_archive(destination, lock)
}

fn stream(input: &mut impl Read, destination: &Path, limit: u64) -> Result<(), String> {
    if limit > MAX_ARCHIVE {
        return Err("FSR ZIP 超出固定预算".into());
    }
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|e| e.to_string())?;
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let count = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > limit {
            return Err("FSR ZIP 数据超过可信锁长度，已停止获取".into());
        }
        output
            .write_all(&buffer[..count])
            .map_err(|e| e.to_string())?;
    }
    if total != limit {
        return Err("FSR ZIP 复制不完整，未发布缓存".into());
    }
    output.sync_all().map_err(|e| e.to_string())
}
pub(super) fn verify_archive(path: &Path, lock: &Lock) -> Result<(), String> {
    let metadata = manifest::regular(path).map_err(|error| {
        format!(
            "无法读取 FEATHRISE_FSR3_PACKAGE {}：{error}",
            path.display()
        )
    })?;
    if metadata.len() != lock.size {
        return Err("FSR 本地 ZIP 长度与可信锁不匹配".into());
    }
    if runtime_package::sha256(path)? != lock.sha256 {
        return Err("FSR ZIP SHA-256 与仓库可信锁不匹配".into());
    }
    Ok(())
}

pub(super) fn extract(archive: &Path, directory: &Path) -> Result<(), String> {
    let mut file = fs::File::open(archive).map_err(|e| e.to_string())?;
    audit_directory(&mut file)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("FSR ZIP 无效：{e}"))?;
    if archive.len() != 8 {
        return Err("FSR ZIP 必须且只能含固定八文件".into());
    }
    let mut names = HashSet::new();
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        let name = entry.name().to_owned();
        if !manifest::names().any(|n| n == name)
            || !names.insert(name.clone())
            || entry.is_dir()
            || entry.is_symlink()
            || entry.encrypted()
            || entry.enclosed_name().as_deref() != Some(Path::new(&name))
        {
            return Err("FSR ZIP 含非白名单、重复、加密或不安全路径".into());
        }
        total = total
            .checked_add(entry.size())
            .ok_or("FSR ZIP 解压长度溢出")?;
        if total > MAX_EXPANDED {
            return Err("FSR ZIP 解压内容超出固定预算".into());
        }
        let expected = entry.size();
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join(name))
            .map_err(|e| e.to_string())?;
        let mut buffer = vec![0_u8; 64 * 1024];
        let mut written = 0_u64;
        loop {
            let count = entry.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            written += count as u64;
            if written > expected {
                return Err("FSR ZIP 条目长度异常".into());
            }
            output
                .write_all(&buffer[..count])
                .map_err(|e| e.to_string())?;
        }
        if written != expected {
            return Err("FSR ZIP 条目被截断".into());
        }
        output.sync_all().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// zip 的名称索引会合并重复条目，因此先检查物理中央目录，再使用其解压/CRC 实现。
/// 发行协议固定为无注释、无分卷的八项 ZIP32；这里无需接受通用 ZIP 的其他变体。
fn audit_directory(file: &mut fs::File) -> Result<(), String> {
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if !(22..=MAX_ARCHIVE).contains(&length) {
        return Err("FSR ZIP 长度无效".into());
    }
    let mut end = [0_u8; 22];
    file.seek(SeekFrom::End(-22)).map_err(|e| e.to_string())?;
    file.read_exact(&mut end).map_err(|e| e.to_string())?;
    let u16_at =
        |bytes: &[u8], offset: usize| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    let u32_at = |bytes: &[u8], offset: usize| {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("固定头长度"))
    };
    if &end[..4] != b"PK\x05\x06"
        || u16_at(&end, 4) != 0
        || u16_at(&end, 6) != 0
        || u16_at(&end, 8) != 8
        || u16_at(&end, 10) != 8
        || u16_at(&end, 20) != 0
    {
        return Err("FSR ZIP 物理目录必须为无分卷、无注释的固定八项".into());
    }
    let start = u64::from(u32_at(&end, 16));
    let size = u64::from(u32_at(&end, 12));
    if start.checked_add(size) != Some(length - 22) {
        return Err("FSR ZIP 中央目录边界无效".into());
    }
    file.seek(SeekFrom::Start(start))
        .map_err(|e| e.to_string())?;
    let mut names = HashSet::new();
    for _ in 0..8 {
        let position = file.stream_position().map_err(|e| e.to_string())?;
        let mut header = [0_u8; 46];
        if position + 46 > length - 22 {
            return Err("FSR ZIP 中央目录被截断".into());
        }
        file.read_exact(&mut header).map_err(|e| e.to_string())?;
        if &header[..4] != b"PK\x01\x02" {
            return Err("FSR ZIP 中央目录条目无效".into());
        }
        let name_length = u64::from(u16_at(&header, 28));
        let extra = u64::from(u16_at(&header, 30)) + u64::from(u16_at(&header, 32));
        if name_length > 128 || position + 46 + name_length + extra > length - 22 {
            return Err("FSR ZIP 中央目录长度超出固定预算".into());
        }
        let mut name = vec![0; name_length as usize];
        file.read_exact(&mut name).map_err(|e| e.to_string())?;
        let name = String::from_utf8(name).map_err(|e| e.to_string())?;
        if !manifest::names().any(|n| n == name) || !names.insert(name) {
            return Err("FSR ZIP 物理目录含非白名单或重复名称".into());
        }
        file.seek(SeekFrom::Current(extra as i64))
            .map_err(|e| e.to_string())?;
    }
    if file.stream_position().map_err(|e| e.to_string())? != length - 22 {
        return Err("FSR ZIP 物理目录不是完整八项".into());
    }
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    Ok(())
}
