//! 作者事务的平台原子替换：短暂共享占用可重试，版本冲突和持续权限错误仍完整返回。
//!
//! 本模块只由文件事务的后台 IO 路径调用。Windows 杀毒/索引等短期持有文件时，最多等待
//! 310 ms，并在每次再次覆盖前执行原版本钩子；不改变权限、不删除目标后重命名、不降级复制。

use std::path::Path; // 原子替换源与目标已经由事务验证为同目录。

pub(super) fn atomic_replace(source: &Path, destination: &Path) -> Result<(), String> {
    atomic_replace_checked(source, destination, || Ok(())) // 回滚仍使用相同原子平台能力。
}

#[cfg(windows)] // Windows 文件句柄共享规则需要短暂重试。
pub(super) fn atomic_replace_checked(
    source: &Path,      // 已经完整刷新并关闭句柄的阶段文件或备份。
    destination: &Path, // 不能在替换前删除正式路径。
    before_attempt: impl FnMut() -> Result<(), String>, // 每次尝试均检查当前正式版本。
) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt; // Win32 要求以 NUL 结尾的 UTF-16。
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    }; // 维持原事务的替换与持久化语义。
    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>(); // 路径只编码一次。
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>(); // 重试复用原目标。
    retry_windows_replace(
        source,
        destination,
        before_attempt,
        || {
            let result = unsafe {
                // 两个路径缓冲在整个 API 调用期间有效。
                MoveFileExW(
                    source_wide.as_ptr(),
                    destination_wide.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            }; // 失败时必须立即获取错误码，不能被后续文件检查覆盖。
            if result != 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            } // 持续失败保留具体系统错误。
        },
        std::thread::sleep,
    ) // 只阻塞已有作者 IO 任务，不在 Bevy 帧关键路径调用。
}

#[cfg(not(windows))] // 其他平台保留原同目录 rename 路径，不增加重试语义。
pub(super) fn atomic_replace_checked(
    source: &Path,                                          // 完整阶段文件。
    destination: &Path,                                     // 同目录正式文件。
    mut before_attempt: impl FnMut() -> Result<(), String>, // 与 Windows 使用相同版本检查合同。
) -> Result<(), String> {
    before_attempt()?; // 失败必须零覆盖。
    std::fs::rename(source, destination)
        .map_err(|error| replacement_error(source, destination, error)) // 文件系统提供原子 rename。
}

fn replacement_error(source: &Path, destination: &Path, error: std::io::Error) -> String {
    format!(
        "原子替换{}失败（源{}）：{error}",
        destination.display(),
        source.display()
    ) // 保留原诊断与 OS 错误。
}

#[cfg(any(windows, test))] // 非 Windows 测试通过注入错误验证相同的重试状态机。
fn retry_windows_replace(
    source: &Path,                                          // 错误诊断中的阶段文件。
    destination: &Path,                                     // 错误诊断中的正式文件。
    mut before_attempt: impl FnMut() -> Result<(), String>, // 等待后重新核对版本。
    mut replace: impl FnMut() -> std::io::Result<()>,       // 单次真实 MoveFileExW 或测试故障注入。
    mut wait: impl FnMut(std::time::Duration), // 测试无需实际等待，生产只在 IO 线程等待。
) -> Result<(), String> {
    const DELAYS_MS: [u64; 5] = [10, 20, 40, 80, 160]; // 总等待 310 ms，不无限等待外部占用。
    for attempt in 0..=DELAYS_MS.len() {
        // 首次调用加五次短暂重试。
        before_attempt()?; // 版本变化立即退出，不能覆盖等待期间的新内容。
        match replace() {
            // 每次都执行同一个原子操作。
            Ok(()) => return Ok(()), // 真正发布成功后不再等待。
            Err(error) => {
                // 区分短暂共享类错误与其他错误。
                let sharing_conflict = matches!(error.raw_os_error(), Some(5 | 32 | 33)); // ACCESS_DENIED、SHARING_VIOLATION、LOCK_VIOLATION。
                if sharing_conflict && let Some(delay) = DELAYS_MS.get(attempt) {
                    // 持续拒绝只耗尽短窗口。
                    wait(std::time::Duration::from_millis(*delay)); // 后台让出 CPU 等待其他句柄释放。
                } else {
                    // 永久权限错误最终仍通过同一事务失败和回滚路径返回。
                    return Err(replacement_error(source, destination, error)); // 不改 ACL、不跳过错误、不偷偷复制覆盖。
                }
            }
        }
    }
    unreachable!("最后一次失败在循环内返回，成功立即退出") // 有限尝试状态机穷尽所有出口。
}

#[cfg(test)] // 注入错误与真实 Windows 句柄共同验证重试边界。
mod tests {
    use super::*; // 直接覆盖正式原子替换实现。
    use std::cell::Cell; // 闭包之间记录调用次数，无共享线程状态。

    #[test] // 短暂共享冲突后再次验证再提交。
    fn transient_sharing_conflicts_retry_only_after_rechecking_version() {
        let checks = Cell::new(0); // 每次替换前必须重新校验。
        let attempts = Cell::new(0); // 第三次原子调用模拟成功。
        let mut waits = Vec::new(); // 不实际阻塞测试线程。
        retry_windows_replace(
            Path::new("stage"),
            Path::new("target"),
            || {
                checks.set(checks.get() + 1);
                Ok(()) // 记录前置版本检查。
            },
            || {
                attempts.set(attempts.get() + 1); // 记录真实原子尝试。
                if attempts.get() < 3 {
                    Err(std::io::Error::from_raw_os_error(32))
                } else {
                    Ok(())
                } // 两次共享冲突后释放。
            },
            |delay| waits.push(delay.as_millis()),
        )
        .unwrap(); // 替换成功才允许返回成功。
        assert_eq!(attempts.get(), 3); // 重试不是直接报告成功。
        assert_eq!(checks.get(), 3); // 每一次覆盖尝试都重新验证。
        assert_eq!(waits, [10, 20]); // 只等待实际需要的短增量。
    }

    #[test] // 持续拒绝保留系统错误，不改变权限或无限等待。
    fn persistent_access_denied_is_bounded_and_preserves_the_error() {
        let attempts = Cell::new(0); // 统计真实尝试次数。
        let mut waited = 0; // 总等待应为 310 ms。
        let error = retry_windows_replace(
            Path::new("stage"),
            Path::new("target"),
            || Ok(()),
            || {
                attempts.set(attempts.get() + 1); // 永久拒绝始终失败。
                Err(std::io::Error::from_raw_os_error(5)) // Win32 ACCESS_DENIED。
            },
            |delay| waited += delay.as_millis(),
        )
        .unwrap_err(); // 不能掩盖权限错误。
        assert_eq!(attempts.get(), 6); // 首次加五次短暂重试。
        assert_eq!(waited, 310); // 有限后台等待。
        assert!(error.contains("target") && error.contains("stage") && error.contains('5')); // 具体路径和系统码仍可诊断。
    }

    #[test] // 重试期间发生外部修改不能直接覆盖新版本。
    fn version_conflict_after_wait_prevents_the_next_replace() {
        let checks = Cell::new(0); // 第二次检查模拟磁盘版本已经改变。
        let attempts = Cell::new(0); // 第二次原子调用不得发生。
        let error = retry_windows_replace(
            Path::new("stage"),
            Path::new("target"),
            || {
                checks.set(checks.get() + 1); // 校验发生在每一次重试之前。
                if checks.get() > 1 {
                    Err("版本冲突：保留外部新内容".into())
                } else {
                    Ok(())
                } // 明确拒绝覆盖。
            },
            || {
                attempts.set(attempts.get() + 1); // 只有第一次尝试进入系统调用。
                Err(std::io::Error::from_raw_os_error(33)) // 初次文件锁冲突。
            },
            |_| {},
        )
        .unwrap_err(); // 测试不需要真实睡眠。
        assert_eq!(checks.get(), 2); // 等待后确实重查版本。
        assert_eq!(attempts.get(), 1); // 不覆写外部新内容。
        assert!(error.contains("版本冲突")); // 冲突诊断优先于先前共享错误。
    }

    #[test] // 路径丢失等错误不能被共享重试吞掉。
    fn unrelated_os_errors_fail_without_delay() {
        let error = retry_windows_replace(
            Path::new("stage"),
            Path::new("target"),
            || Ok(()),
            || {
                Err(std::io::Error::from_raw_os_error(2)) // FILE_NOT_FOUND 不是共享冲突。
            },
            |_| panic!("其他错误必须立即返回"),
        )
        .unwrap_err(); // 重试会使测试失败。
        assert!(error.contains('2')); // 保留具体系统错误。
    }

    #[cfg(windows)] // Windows 共享删除语义需要在真实文件系统验证。
    #[test]
    fn real_windows_reader_can_release_before_atomic_retry_succeeds() {
        use std::os::windows::fs::OpenOptionsExt; // 显式构造禁止共享删除的临时读者。
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE}; // 刻意不设置 FILE_SHARE_DELETE。
        let tag = super::super::SAVE_TRANSACTION_COUNTER
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed); // 并行测试唯一目录。
        let root =
            std::env::temp_dir().join(format!("bevy-author-retry-{}-{tag}", std::process::id())); // 仅拥有本测试目录。
        std::fs::create_dir(&root).unwrap(); // 不覆盖未知旧目录。
        let source = root.join("stage.json"); // 已生成新版本。
        let target = root.join("target.json"); // 正式旧版本。
        std::fs::write(&source, b"new").unwrap(); // 阶段文件句柄立即关闭。
        std::fs::write(&target, b"old").unwrap(); // 正式文件句柄立即关闭。
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&target)
            .unwrap(); // 模拟短暂索引读者。
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(35)); // 仅测试后台读者短暂持有。
            drop(reader); // 显式释放阻挡 MoveFileExW 的句柄。
        }); // 主测试线程执行真实替换。
        let result = atomic_replace_checked(&source, &target, || {
            if std::fs::read(&target).unwrap() == b"old" {
                Ok(())
            } else {
                Err("版本冲突".into())
            } // 每次重试仍保留预期版本规则。
        }); // 发布始终是单次系统原子操作。
        release.join().unwrap(); // 清理前确保没有测试遗留读者。
        assert!(result.is_ok(), "{result:?}"); // 共享冲突消失后可以成功。
        assert_eq!(std::fs::read(&target).unwrap(), b"new"); // 正式路径包含完整新文件。
        assert!(!source.exists()); // 原子移动成功后阶段路径消失。
        std::fs::remove_dir_all(&root).unwrap(); // 只清理本测试创建的唯一目录。
    }
}
