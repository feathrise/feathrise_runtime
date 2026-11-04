//! 地图保存事务的目标校验、逐项stage生成与内部暂存文件清理。
//!
//! 【功能设计】普通JSON、纹理和GLB仍通过借用、共享或独占字节逐项落盘；Blend等大型
//! 外部派生只能读取同批较早目标已经sync完成的stage，并直接生成当前stage。此模块不接触
//! 任何正式目标的backup或replace，发布与回滚继续由父事务统一负责。

use std::collections::BTreeSet; // Windows大小写不敏感路径需要统一去重。
use std::fs::{File, OpenOptions}; // 字节stage使用create_new，外部stage完成后用可写句柄刷新。
use std::io::Write; // 完整写入与sync确保字节stage可作为提交完成值。
use std::path::{Path, PathBuf}; // 目标、依赖和事务内部路径保持原生平台路径。
use std::sync::Arc; // 大型R8作者源沿用冻结草稿的共享所有权，避免保存时复制整图。

use crate::authoring_io::asset_path_collision_key; // 保存批次沿用正式跨平台词法路径身份。

type BytesProducer<'a> = Box<dyn FnOnce() -> Result<PreparedBytes<'a>, String> + 'a>; // 编码只在对应stage即将写入时执行一次。
type StagedFileProducer<'a> = Box<dyn FnOnce(&Path, &Path) -> Result<(), String> + 'a>; // 外部工具只读取已完成依赖stage并直接生成当前stage。
type StreamProducer<'a> = Box<dyn FnOnce(&mut dyn Write) -> Result<(), String> + 'a>; // 地图角色JSON直接编码到stage，不生成第二份完整字节数组。

/// 保存项的两种有界生成方式：内存字节或依赖较早stage的完整外部文件。
enum PreparedProducer<'a> {
    Bytes(BytesProducer<'a>), // JSON、作者源与普通派生仍逐项生成并立即写盘释放。
    Stream(StreamProducer<'a>), // 任意规模JSON沿流式写入路径处理。
    FromStagedFile {
        source_target: PathBuf, // 依赖目标必须在同批当前项之前，正式路径尚未发布。
        produce: StagedFileProducer<'a>, // 闭包接收依赖stage与当前输出stage的精确路径。
    },
}

/// 一个已经冻结目标与输入、但尚未生成大型派生字节的保存项。
pub struct PreparedWrite<'a> {
    pub path: PathBuf,             // 已通过对应资产所有者规则校验的最终路径。
    produce: PreparedProducer<'a>, // 字节编码或受检stage依赖都只消费一次。
}

/// stage时短暂持有的三种字节所有权，写完当前项立即释放。
enum PreparedBytes<'a> {
    Borrowed(&'a [u8]),   // 保存任务独占的小型作者Vec可以直接借用。
    Owned(Vec<u8>),       // 当前JSON/KTX2/PNG/R16/GLB编码结果由本项短暂持有。
    Shared(Arc<Vec<u8>>), // 大型raw R8画布只复制Arc句柄。
}

impl PreparedBytes<'_> {
    /// 统一取得只读字节切片而不改变当前所有权。
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) => bytes, // 生命周期绑定冻结草稿直到保存任务结束。
            Self::Owned(bytes) => bytes.as_slice(), // 写完对应stage即释放，不与下一派生物叠加。
            Self::Shared(bytes) => bytes.as_slice(), // Arc保证其他后台任务释放也不影响保存。
        }
    }
}

impl<'a> PreparedWrite<'a> {
    /// 登记已经生成的小型或兼容派生字节。
    pub fn new(path: PathBuf, bytes: Vec<u8>) -> Self {
        Self {
            path, // 已经生成的小型或兼容派生目标。
            produce: PreparedProducer::Bytes(Box::new(move || {
                Ok(PreparedBytes::Owned(bytes)) // 所有权只转移一次。
            })),
        }
    }

    /// 登记由冻结草稿直接借用的作者字节。
    pub fn borrowed(path: PathBuf, bytes: &'a [u8]) -> Self {
        Self {
            path, // 已冻结作者源对应的正式目标。
            produce: PreparedProducer::Bytes(Box::new(move || {
                Ok(PreparedBytes::Borrowed(bytes)) // 不复制草稿自有Vec。
            })),
        }
    }

    /// 登记通过Arc共享的大型作者源字节。
    pub fn shared(path: PathBuf, bytes: Arc<Vec<u8>>) -> Self {
        Self {
            path, // 大型共享R8对应的正式目标。
            produce: PreparedProducer::Bytes(Box::new(move || {
                Ok(PreparedBytes::Shared(bytes)) // 不复制整张作者图。
            })),
        }
    }

    /// 登记在当前stage开始前才执行的单项字节编码器。
    pub fn deferred<F>(path: PathBuf, produce: F) -> Self
    where
        F: FnOnce() -> Result<Vec<u8>, String> + 'a,
    {
        Self {
            path, // 全部目标先收集，编码函数稍后只生成这一项。
            produce: PreparedProducer::Bytes(Box::new(move || {
                produce().map(PreparedBytes::Owned) // 编码失败只会清理stage。
            })),
        }
    }

    /// 当前项从同批较早目标的完整stage生成，正式文件仍要等全部stage成功后共同发布。
    pub fn from_staged_file<F>(
        path: PathBuf,          // 当前派生文件的最终正式路径。
        source_target: PathBuf, // 依赖项在同批中的正式目标身份。
        produce: F,             // 只执行一次的外部或流式文件生成器。
    ) -> Self
    where
        F: FnOnce(&Path, &Path) -> Result<(), String> + 'a,
    {
        Self {
            path, // 当前项仍参与目标去重、backup、原子替换与回滚。
            produce: PreparedProducer::FromStagedFile {
                source_target,              // 事务会解析为尚未发布的完整stage。
                produce: Box::new(produce), // 生成器不能访问正式依赖路径。
            },
        }
    }

    /// 返回可选stage依赖的正式目标身份，供事务在首个文件创建前验证拓扑顺序。
    pub(super) fn source_target(&self) -> Option<&Path> {
        match &self.produce {
            PreparedProducer::Bytes(_) | PreparedProducer::Stream(_) => None, // 普通项没有跨stage依赖。
            PreparedProducer::FromStagedFile { source_target, .. } => Some(source_target), // 外部文件项必须引用较早目标。
        }
    }

    /// 登记项目统一漂亮UTF-8 JSON编码项。
    pub fn json<T>(path: PathBuf, value: &'a T) -> Self
    where
        T: serde::Serialize + ?Sized,
    {
        let error_path = path.clone(); // 延迟序列化仍需报告正式目标而不是内部stage。
        Self::streaming(path, move |writer| {
            serde_json::to_writer_pretty(&mut *writer, value)
                .map_err(|error| format!("序列化{}失败：{error}", error_path.display()))?;
            writer.write_all(b"\n").map_err(|error| error.to_string())
        }) // 输出与原漂亮JSON字节一致，但不保留完整编码副本。
    }

    /// 直接向本事务独占stage流式编码；闭包只能写候选，不能发布正式文件。
    pub fn streaming<F>(path: PathBuf, produce: F) -> Self
    where
        F: FnOnce(&mut dyn Write) -> Result<(), String> + 'a,
    {
        Self {
            path,
            produce: PreparedProducer::Stream(Box::new(produce)),
        }
    }
}

/// 以项目既有格式生成漂亮UTF-8 JSON并保留终止换行。
pub fn encode_json_bytes<T: serde::Serialize + ?Sized>(
    path: &Path, // 只用于报告正式目标身份。
    value: &T,   // 冻结草稿中的可序列化值。
) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("序列化{}失败：{error}", path.display()))?; // 非有限值等错误发生在正式发布前。
    bytes.push(b'\n'); // 与既有atomic_write_json输出保持字节兼容。
    Ok(bytes) // 当前项stage写完后立即释放。
}

/// 一个正式目标对应的stage、backup及回滚状态。
pub(super) struct StagedWrite {
    pub(super) target: PathBuf,    // 对外可见的正式资产路径。
    pub(super) stage: PathBuf,     // 同目录、同事务的完整新文件。
    pub(super) backup: PathBuf,    // 同目录、同事务的旧文件备份。
    pub(super) stage_ready: bool,  // 只允许清理本进程确实生成成功的stage。
    pub(super) had_original: bool, // 回滚时区分原子恢复旧文件与删除本次新增文件。
    pub(super) backup_ready: bool, // 旧目标已经复制或硬链接到backup。
    pub(super) published: bool,    // stage已经原子替换正式目标。
}

/// 在任何编码或文件创建前验证目标唯一性及stage依赖顺序。
pub(super) fn validate_targets(writes: &[PreparedWrite<'_>]) -> Result<(), String> {
    let mut targets = BTreeSet::new(); // 只保存已经完整验证的较早目标键。
    for write in writes {
        let target_key = asset_path_collision_key(&write.path); // 同批目标按跨平台词法身份去重。
        if targets.contains(&target_key) {
            return Err(format!(
                "地图保存批次包含重复目标：{}",
                write.path.display()
            )); // 重复正式路径不能靠写入顺序决定胜者。
        }
        if write.path.file_name().is_none() || write.path.parent().is_none() {
            return Err(format!(
                "地图保存目标没有有效文件名或父目录：{}",
                write.path.display()
            )); // 内部stage必须能与正式目标同目录构造。
        }
        if let Some(source_target) = write.source_target()
            && !targets.contains(&asset_path_collision_key(source_target))
        {
            return Err(format!(
                "地图保存派生目标{}依赖的较早stage不存在：{}",
                write.path.display(),
                source_target.display()
            )); // 依赖必须已经登记在当前项之前，禁止环、前向引用或读取正式文件。
        }
        targets.insert(target_key); // 当前项完成全部校验后才允许后续派生依赖它。
    }
    Ok(()) // 全部路径与有向stage依赖都可安全执行。
}

/// 为每个目标计算同目录唯一stage与backup路径，但尚不创建任何文件。
pub(super) fn prepare_staged_targets(
    writes: &[PreparedWrite<'_>], // 已通过路径和依赖验证的完整批次。
    tag: &str,                    // 进程号与事务计数组成的唯一标签。
) -> Result<Vec<StagedWrite>, String> {
    writes
        .iter() // 输出顺序与PreparedWrite顺序完全一致。
        .enumerate() // 索引进一步隔离同一事务内不同正式文件。
        .map(|(index, write)| {
            let parent = write
                .path
                .parent()
                .expect("目标父目录已由validate_targets验证"); // stage与backup保持同卷原子替换能力。
            let file_name = write
                .path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| {
                    format!("地图保存文件名不是有效Unicode：{}", write.path.display())
                })?; // 内部诊断名不能丢失目标身份。
            Ok(StagedWrite {
                target: write.path.clone(), // 提交时才会变更正式路径。
                stage: parent.join(format!(".{file_name}.map-save-{tag}-{index}.stage")), // 新完成值。
                backup: parent.join(format!(".{file_name}.map-save-{tag}-{index}.backup")), // 旧快照。
                stage_ready: false,  // stage_one成功后才取得清理所有权。
                had_original: false, // 父事务replace_one会在备份前冻结真实存在性。
                backup_ready: false, // 尚未接触旧目标。
                published: false,    // 尚未发布新目标。
            })
        })
        .collect() // 任一Unicode路径错误时整个批次仍然零文件写入。
}

/// 按当前项类型写入字节或调用依赖stage的外部文件生成器。
pub(super) fn stage_one(
    write: PreparedWrite<'_>,    // 当前唯一待生成项。
    staged: &mut StagedWrite,    // 当前项的事务内部路径和所有权状态。
    source_stage: Option<&Path>, // 外部文件生成项取得的已完成依赖stage。
) -> Result<(), String> {
    let parent = staged
        .stage
        .parent()
        .expect("stage与已验证正式目标位于同一父目录"); // prepare_staged_targets已建立同目录身份。
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("创建地图保存目录{}失败：{error}", parent.display()))?; // IO仍只发生在后台任务。
    match write.produce {
        PreparedProducer::Bytes(produce) => stage_bytes(produce, staged), // 普通项保持原逐项字节写入路径。
        PreparedProducer::Stream(produce) => stage_stream(produce, staged), // 流式JSON不使用容量门槛。
        PreparedProducer::FromStagedFile {
            source_target,
            produce,
        } => {
            let source_stage = source_stage.ok_or_else(|| {
                format!(
                    "地图保存派生目标{}缺少已完成stage：{}",
                    staged.target.display(),
                    source_target.display()
                )
            })?; // 防御未来调用绕过validate_targets。
            stage_generated_file(produce, source_stage, staged) // 外部工具只接触两个内部stage路径。
        }
    }
}

fn stage_stream(produce: StreamProducer<'_>, staged: &mut StagedWrite) -> Result<(), String> {
    let file = create_transaction_file(&staged.stage)?;
    let mut writer = std::io::BufWriter::new(file);
    let result = produce(&mut writer)
        .and_then(|()| writer.flush().map_err(|error| error.to_string()))
        .and_then(|()| {
            writer
                .get_ref()
                .sync_all()
                .map_err(|error| error.to_string())
        });
    drop(writer); // Windows原子替换前关闭stage句柄。
    if let Err(error) = result {
        let _ = std::fs::remove_file(&staged.stage);
        return Err(format!("流式写入{}失败：{error}", staged.target.display()));
    }
    staged.stage_ready = true;
    Ok(())
}

/// 把借用、共享或独占字节写入当前唯一stage并同步落盘。
fn stage_bytes(
    produce: BytesProducer<'_>, // 当前项只执行一次的字节生成器。
    staged: &mut StagedWrite,   // 当前事务目标状态。
) -> Result<(), String> {
    let bytes = produce()?; // 当前大型派生只在这一项的stage即将写入时存在。
    let mut file = create_transaction_file(&staged.stage)?; // create_new拒绝未知遗留文件。
    let write_result = file
        .write_all(bytes.as_slice()) // 一次写入完整当前项。
        .and_then(|()| file.sync_all()); // 完成值刷新成功后才取得stage资格。
    drop(file); // Windows必须先释放句柄，错误清理与稍后的原子替换才不会被自身阻塞。
    drop(bytes); // 本项stage完整落盘后立刻释放KTX2/GLB，不叠加下一项峰值。
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&staged.stage); // 失败部分stage不属于可提交集合。
        return Err(format!(
            "写入地图事务暂存文件{}失败：{error}",
            staged.stage.display()
        )); // 正式路径仍完全未变。
    }
    staged.stage_ready = true; // 只有完整sync成功的stage才进入提交集合。
    Ok(()) // 父事务可以继续下一项。
}

/// 让受检生成器直接写出当前stage，避免大型Blend再次读入Rust内存。
fn stage_generated_file(
    produce: StagedFileProducer<'_>, // 接收依赖stage和当前输出stage的单次生成器。
    source_stage: &Path,             // 已经sync且尚未正式发布的不可变依赖。
    staged: &mut StagedWrite,        // 当前输出stage所有权。
) -> Result<(), String> {
    if std::fs::symlink_metadata(&staged.stage).is_ok() {
        return Err(format!(
            "地图事务外部生成stage已经存在：{}",
            staged.stage.display()
        )); // 与create_new字节路径保持相同不覆盖语义。
    }
    if let Err(error) = produce(source_stage, &staged.stage) {
        let _ = std::fs::remove_file(&staged.stage); // 生成器部分输出只属于本事务精确stage。
        return Err(error); // 正式目标尚未开始发布，原诊断直接返回。
    }
    let metadata = std::fs::symlink_metadata(&staged.stage).map_err(|error| {
        format!(
            "地图事务外部生成器没有形成stage {}：{error}",
            staged.stage.display()
        )
    })?; // 外部进程零退出但没写文件仍属于失败。
    if !metadata.file_type().is_file() || metadata.len() == 0 {
        let _ = std::fs::remove_file(&staged.stage); // 空文件或非普通文件不能进入提交集合。
        return Err(format!(
            "地图事务外部生成stage不是非空普通文件：{}",
            staged.stage.display()
        )); // 正式目标保持旧版本。
    }
    OpenOptions::new()
        .read(true) // 保留完成值读取资格供文件系统刷新。
        .write(true) // Windows FlushFileBuffers需要可写句柄。
        .open(&staged.stage)
        .and_then(|file| file.sync_all())
        .map_err(|error| {
            let _ = std::fs::remove_file(&staged.stage); // 刷新失败的完成值不可重试复用。
            format!(
                "刷新地图事务外部生成stage {}失败：{error}",
                staged.stage.display()
            )
        })?;
    staged.stage_ready = true; // 只有生成、非空校验与sync全部成功后取得提交资格。
    Ok(()) // 父事务可以继续生成剩余stage。
}

/// 创建字节项独占stage文件。
fn create_transaction_file(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true) // 当前项只需要顺序写入。
        .create_new(true) // 不覆盖崩溃遗留或外部同名文件。
        .open(path)
        .map_err(|error| format!("创建地图事务暂存文件{}失败：{error}", path.display())) // 返回正式可诊断内部路径。
}

/// 删除本事务确实生成完成但尚未发布的stage。
pub(super) fn cleanup_stages(staged: &[StagedWrite]) {
    for item in staged {
        if item.stage_ready {
            let _ = std::fs::remove_file(&item.stage); // 只清除本事务确实创建并仍持有的stage。
        }
    }
}
