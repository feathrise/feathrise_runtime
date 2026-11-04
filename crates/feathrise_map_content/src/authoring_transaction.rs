//! 地图保存的跨文件回滚事务。
//!
//! 【功能设计】事务先收齐并校验全部正式目标，再逐项生成内容并写入同目录唯一stage；大型
//! KTX2/GLB写入stage后立即释放；同批外部派生只能读取已经完成的较早stage，并把结果直接
//! 写入自己的stage，不让大型Blend重新进入Rust内存。全部stage成功后，旧目标以硬链接或
//! 受检复制形成backup，正式发布与回滚都使用平台原子替换，AssetServer永远不会观察到
//! “旧目标已移走、新目标尚未到位”的路径缺失窗口。

mod atomic;
mod staging; // 目标/依赖校验与逐项stage生成不混入正式发布和回滚状态机。 // 平台原子替换与 Windows 短暂共享冲突重试，始终在作者 IO 任务内运行。

use std::fs::{File, OpenOptions}; // 正式旧目标复制backup时使用独占写句柄。
use std::path::{Path, PathBuf}; // stage、backup与正式目标同目录，避免跨卷非原子移动。
// 同进程并行地图任务也不会复用事务名。
use std::sync::atomic::{AtomicU64, Ordering};

use atomic::{atomic_replace, atomic_replace_checked};
pub use staging::{PreparedWrite, encode_json_bytes}; // 保存批次与目录辅助只通过冻结写项和统一JSON编码接口进入事务。
use staging::{StagedWrite, cleanup_stages, prepare_staged_targets, stage_one, validate_targets}; // 父模块只编排stage完成集合、正式replace与逆序回滚。 // 重试前继续验证原版本，不把等待变成无条件覆盖。

static SAVE_TRANSACTION_COUNTER: AtomicU64 = AtomicU64::new(0); // 事务级唯一后缀只用于内部文件。

/// 事务失败同时携带曾经发布后又回滚的正式路径，供主线程纠正AssetServer中间缓存。
pub struct TransactionFailure {
    message: String,                // 包含原始失败与回滚完整性诊断。
    rollback_targets: Vec<PathBuf>, // 只列出本事务确实原子发布过的正式目标。
    rollback_complete: bool,        // 正式目标是否完整恢复到事务开始前，不能靠解析诊断字符串判断。
}

impl TransactionFailure {
    fn before_publish(message: String) -> Self {
        Self {
            message,                      // 纯预检、编码或stage失败的原始诊断。
            rollback_targets: Vec::new(), // 正式路径从未变化，不需要AssetServer重载。
            rollback_complete: true,      // 发布未开始，磁盘天然仍是完整旧批次。
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn rollback_targets(&self) -> &[PathBuf] {
        &self.rollback_targets
    }

    pub fn rollback_complete(&self) -> bool {
        self.rollback_complete
    }
}

/// 生产提交不注入故障；测试通过同一核心路径观察并精确中断第N个replace。
pub fn commit_prepared_writes(writes: Vec<PreparedWrite<'_>>) -> Result<(), TransactionFailure> {
    commit_prepared_writes_with(writes, |_, _| Ok(()))
}

/// 让全局预设等作者域复用地图保存的跨文件stage、回滚和原子替换实现。
pub fn commit_owned_files(files: Vec<(PathBuf, Vec<u8>)>) -> Result<(), String> {
    let writes = files
        .into_iter()
        .map(|(path, bytes)| PreparedWrite::new(path, bytes))
        .collect();
    commit_prepared_writes(writes).map_err(|failure| failure.message().to_owned())
}

/// 每次正式原子替换尝试前执行只读校验；共享冲突重试也重新检查，失败回滚较早目标。
pub fn commit_prepared_writes_checked<F>(
    writes: Vec<PreparedWrite<'_>>,
    before_replace: F,
) -> Result<(), TransactionFailure>
where
    F: FnMut(usize, &Path) -> Result<(), String>,
{
    commit_prepared_writes_with(writes, before_replace)
}

fn commit_prepared_writes_with<F>(
    writes: Vec<PreparedWrite<'_>>,
    mut before_replace: F,
) -> Result<(), TransactionFailure>
where
    F: FnMut(usize, &Path) -> Result<(), String>,
{
    validate_targets(&writes).map_err(TransactionFailure::before_publish)?; // 路径别名在任何编码或文件变更前拒绝。
    let transaction_id = SAVE_TRANSACTION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tag = format!("{}-{transaction_id}", std::process::id()); // 进程号便于人工识别异常退出遗留物。
    let mut staged =
        prepare_staged_targets(&writes, &tag).map_err(TransactionFailure::before_publish)?; // 只计算内部路径，不创建任何文件。

    for (index, write) in writes.into_iter().enumerate() {
        let source_stage = write.source_target().map(|source_target| {
            staged[..index]
                .iter() // 只允许读取已经完成的较早stage。
                .find(|item| item.target == source_target) // validate_targets已证明依赖唯一且位于前方。
                .expect("stage依赖顺序已在任何文件创建前验证")
                .stage
                .clone() // 结束不可变借用后才能把当前StagedWrite可变交给stage_one。
        });
        if let Err(error) = stage_one(write, &mut staged[index], source_stage.as_deref()) {
            cleanup_stages(&staged); // 编码或staging失败时正式目标从未改变。
            return Err(TransactionFailure::before_publish(error));
        }
    }

    for index in 0..staged.len() {
        if let Err(error) = replace_one(&mut staged[index], index + 1, &mut before_replace) {
            let rollback_targets = staged
                .iter()
                .filter(|item| item.published)
                .map(|item| item.target.clone())
                .collect(); // 回滚前冻结确实可能已被AssetServer读到的新正式路径。
            let rollback_errors = rollback_all(&mut staged);
            let rollback_complete = rollback_errors.is_empty(); // 混合磁盘版本绝不能进入“旧资源已恢复”交接门。
            return Err(TransactionFailure {
                message: append_rollback_errors(error, rollback_errors),
                rollback_targets,
                rollback_complete,
            });
        }
    }

    // 全部正式目标都已原子发布后才删除旧版本；完整新批次不能因清理诊断被误报成保存失败。
    cleanup_after_success(&staged);
    Ok(())
}

fn replace_one<F>(
    staged: &mut StagedWrite,
    replace_index: usize,
    before_replace: &mut F,
) -> Result<(), String>
where
    F: FnMut(usize, &Path) -> Result<(), String>,
{
    staged.had_original = match std::fs::symlink_metadata(&staged.target) {
        Ok(metadata) if metadata.file_type().is_file() => true, // 普通旧文件可以安全形成备份。
        Ok(_) => {
            return Err(format!(
                "地图保存目标不是普通文件：{}",
                staged.target.display()
            )); // 目录或符号链接不能进入可能越过资产所有者边界的替换事务。
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(format!(
                "检查地图保存目标{}失败：{error}",
                staged.target.display()
            ));
        }
    };
    if staged.had_original {
        backup_file(&staged.target, &staged.backup)?; // 正式目标仍留在原路径，AssetServer不会看到Removed。
        staged.backup_ready = true;
    }
    atomic_replace_checked(&staged.stage, &staged.target, || {
        before_replace(replace_index, &staged.target) // 备份后且每次重试前都重新核对版本；冲突立即停止。
    })?; // 单文件发布继续保持原 atomic_write 的路径原子性。
    staged.stage_ready = false; // stage已经移动到正式路径，不再拥有内部文件名。
    staged.published = true;
    Ok(())
}

fn backup_file(source: &Path, backup: &Path) -> Result<(), String> {
    if std::fs::hard_link(source, backup).is_ok() {
        return Ok(()); // 同目录硬链接不复制大文件且保留完全相同的旧inode内容。
    }
    let mut backup_created = false; // create_new失败时绝不能删除同名的外部遗留文件。
    let result = (|| {
        let mut source_file = File::open(source)
            .map_err(|error| format!("读取旧地图目标{}失败：{error}", source.display()))?;
        let mut backup_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(backup)
            .map_err(|error| format!("创建地图事务备份{}失败：{error}", backup.display()))?;
        backup_created = true; // 从此处起失败清理只删除本调用创建的部分副本。
        std::io::copy(&mut source_file, &mut backup_file)
            .map_err(|error| format!("复制地图事务备份{}失败：{error}", backup.display()))?;
        backup_file
            .sync_all()
            .map_err(|error| format!("刷新地图事务备份{}失败：{error}", backup.display()))?;
        Ok(())
    })();
    if result.is_err() && backup_created {
        let _ = std::fs::remove_file(backup); // 失败的部分backup绝不能冒充可恢复快照。
    }
    result
}

fn rollback_all(staged: &mut [StagedWrite]) -> Vec<String> {
    let mut errors = Vec::new();
    for item in staged.iter_mut().rev() {
        if item.published && item.had_original {
            if let Err(error) = atomic_replace(&item.backup, &item.target) {
                errors.push(format!(
                    "原子恢复旧目标{}失败：{error}",
                    item.target.display()
                ));
                continue; // 唯一旧快照必须保留，不能在错误清理中删除backup。
            }
            item.backup_ready = false; // replace已把backup移动回正式目标。
            item.published = false;
        } else if item.published {
            if let Err(error) = std::fs::remove_file(&item.target)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                errors.push(format!(
                    "删除事务新增目标{}失败：{error}",
                    item.target.display()
                ));
                continue;
            }
            item.published = false; // 原本不存在的正式路径已经恢复为缺席。
        } else if item.backup_ready {
            if let Err(error) = std::fs::remove_file(&item.backup) {
                errors.push(format!(
                    "清理未发布备份{}失败：{error}",
                    item.backup.display()
                ));
                continue;
            }
            item.backup_ready = false; // 原子覆盖未发生，正式目标始终还是旧文件。
        }
    }
    cleanup_stages(staged); // 正常回滚后清除全部未发布stage。
    errors
}

fn cleanup_after_success(staged: &[StagedWrite]) {
    cleanup_stages(staged); // 全部stage在成功时均已移动，但统一清扫防御异常文件系统行为。
    for item in staged {
        if item.backup_ready {
            let _ = std::fs::remove_file(&item.backup); // 全批成功后且由本事务创建的旧快照才可删除。
        }
    }
}

fn append_rollback_errors(error: String, rollback_errors: Vec<String>) -> String {
    if rollback_errors.is_empty() {
        format!("{error}；已恢复保存前全部地图文件")
    } else {
        format!(
            "{error}；地图保存回滚不完整：{}",
            rollback_errors.join("；")
        )
    }
}

pub fn commit_with_replace_hook<F>(
    writes: Vec<PreparedWrite<'_>>,
    before_replace: F,
) -> Result<(), TransactionFailure>
where
    F: FnMut(usize, &Path) -> Result<(), String>,
{
    commit_prepared_writes_with(writes, before_replace)
}
