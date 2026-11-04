//! 从显式提供的本地 ZIP 验证并准备 Windows FSR 原生配套包。

#[path = "native_package/manifest.rs"]
mod manifest;
#[cfg(test)]
#[path = "native_package/tests.rs"]
mod tests;
#[path = "native_package/transport.rs"]
mod transport;

use super::runtime_package::{self, RuntimePackage};
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

pub const TARGET: &str = "x86_64-pc-windows-msvc";
#[allow(dead_code)] // build.rs 用于 Cargo 输入跟踪，发行预检本身不消费此列表。
pub const SOURCE_FILES: [&str; 7] = manifest::SOURCE_FILES;
pub const INPUT_ENVS: [&str; 2] = ["FEATHRISE_FSR3_PACKAGE", "FEATHRISE_NATIVE_CACHE"];
const LOCK_WAIT: Duration = Duration::from_secs(30);
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct NativePackage {
    pub runtime: RuntimePackage,
    #[allow(dead_code)] // build.rs 负责实际链接；xtask 仅校验整包身份。
    pub bridge_library: PathBuf,
    #[allow(dead_code)] // 复制到 Cargo 私有输出后按同一已验身份复查，避免共享缓存发生变更。
    pub bridge_sha256: String,
    /// 可信仓库锁定的整个 ZIP 身份，同时覆盖 AMD DLL 和桥接静态库。
    pub identity: String,
}

struct Options {
    cache: PathBuf,
    local: Option<PathBuf>,
}

/// 调用方显式提供原生包路径，SDK 的锁继续固定 SHA、目标与 ABI。
pub fn acquire(crate_root: &Path, target: &str) -> Result<NativePackage, String> {
    let local = env::var_os(INPUT_ENVS[0]).map(PathBuf::from);
    let cache = env::var_os(INPUT_ENVS[1])
        .map(PathBuf::from)
        .or_else(|| env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Feathrise/native")))
        .ok_or("请设置 LOCALAPPDATA 或 FEATHRISE_NATIVE_CACHE 作为原生库缓存目录")?;
    acquire_with(crate_root, target, &Options { cache, local })
}

fn acquire_with(
    crate_root: &Path,
    target: &str,
    options: &Options,
) -> Result<NativePackage, String> {
    let lock = manifest::Lock::read(crate_root, target)?;
    let local = options.local.as_deref().ok_or_else(|| {
        format!(
            "缺少 {target} 的 FSR 原生包；请准备 {} 并将 FEATHRISE_FSR3_PACKAGE 设为本地 ZIP 路径",
            lock.archive
        )
    })?;
    let cache = options.cache.join("fsr3-fg");
    fs::create_dir_all(&cache).map_err(|e| format!("创建 FSR 缓存目录失败：{e}"))?;
    let directory = cache.join(&lock.sha256);
    // OS 文件锁在进程异常退出时也会释放；锁文件本身不是完成标记。
    let lock_path = cache.join(format!("{}.lock", lock.sha256));
    let guard = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| format!("打开 FSR 缓存锁失败：{e}"))?;
    let started = Instant::now();
    loop {
        match guard.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if started.elapsed() < LOCK_WAIT => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(format!("等待 FSR 缓存锁失败或超时：{error}")),
        }
    }
    if directory.try_exists().map_err(|e| e.to_string())? {
        // 热缓存也必须对应本次明确提供的归档，防止坏输入被旧缓存掩盖。
        transport::verify_archive(local, &lock)?;
        return manifest::inspect(&directory, crate_root, &lock).map_err(|e| {
            format!(
                "FSR 缓存校验失败：{e}；请移走损坏的专属目录 {} 后重试（不会自动改用源码编译）",
                directory.display()
            )
        });
    }
    let stage = Stage::create(&cache, &lock.sha256)?;
    let archive = stage.0.join("package.zip");
    transport::copy_archive(local, &archive, &lock)?;
    let payload = stage.0.join("payload");
    fs::create_dir(&payload).map_err(|e| e.to_string())?;
    transport::extract(&archive, &payload)?;
    manifest::inspect(&payload, crate_root, &lock)?;
    // 只有完整包通过全部身份校验后才原子发布，其他进程永远看不到半包。
    fs::rename(&payload, &directory).map_err(|e| format!("发布 FSR 原生缓存失败：{e}"))?;
    manifest::inspect(&directory, crate_root, &lock)
}

struct Stage(PathBuf);
impl Stage {
    fn create(parent: &Path, identity: &str) -> Result<Self, String> {
        for _ in 0..32 {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(
                ".{identity}.staging-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("创建 FSR 暂存目录失败：{e}")),
            }
        }
        Err("FSR 暂存目录冲突过多".into())
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        // 只清理由本实例 create_dir 成功取得所有权的直接子目录，不跟随陌生路径。
        let _ = fs::remove_dir_all(&self.0);
    }
}
