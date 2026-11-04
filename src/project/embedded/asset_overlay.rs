//! 完整快照的只读内存资产源；读者冻结自己的 Arc，主帧只尝试短锁并交换整份表。

use bevy::{
    asset::io::{Reader, STACK_FUTURE_SIZE, SeekableReader, StackFuture},
    prelude::Resource,
    tasks::futures_lite::io::{AsyncRead, AsyncSeek},
};
use std::{
    collections::BTreeMap,
    io::{Error, ErrorKind, SeekFrom},
    path::{Component, Path},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};

pub(super) type Files = BTreeMap<String, Arc<Vec<u8>>>;
pub(super) type ReadEvidence = BTreeMap<String, SourceEvidence>;

/// 打开新正文、生产 Assets 真正采用、生产 loader 失败是三个不同事实。
/// 热重载期间 AssetServer 可能仍报告旧 Loaded，不能仅凭 reader 被调用就允许试玩。
#[derive(Default)]
pub(super) struct SourceEvidence {
    pub read: AtomicBool,
    pub applied: AtomicBool,
    pub failed: AtomicBool,
}

#[derive(Default)]
pub(super) struct Publication {
    pub files: Arc<Files>,
    pub reads: Arc<ReadEvidence>,
}

#[derive(Resource, Clone, Default)]
pub(crate) struct AssetOverlay(pub(super) Arc<Mutex<Publication>>);

impl AssetOverlay {
    /// Assets 的异步 reader 调用；锁内只共享一个字节 Arc，不做读取、解码或扫描。
    pub(crate) fn read(&self, path: &Path) -> Option<ArcReader> {
        let path = path.to_str()?.replace('\\', "/");
        let current = self.0.lock().ok()?;
        if let Some(evidence) = current.reads.get(&path) {
            evidence.read.store(true, Ordering::Release);
        }
        let bytes = current.files.get(&path)?.clone();
        Some(ArcReader { bytes, position: 0 })
    }

    pub(super) fn publish(&self, files: Publication) -> Result<Publication, Publication> {
        match self.0.try_lock() {
            Ok(mut current) => Ok(std::mem::replace(&mut *current, files)),
            Err(_) => Err(files), // 主线程从不等 AssetReader；下一帧再试。
        }
    }
}

pub(super) fn valid_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains([':', '#', '\\'])
        && Path::new(value)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && !value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

pub(crate) struct ArcReader {
    bytes: Arc<Vec<u8>>,
    position: usize,
}

impl AsyncRead for ArcReader {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        output: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let count = output
            .len()
            .min(this.bytes.len().saturating_sub(this.position));
        if count > 0 {
            output[..count].copy_from_slice(&this.bytes[this.position..this.position + count]);
        }
        this.position += count;
        Poll::Ready(Ok(count))
    }
}

impl AsyncSeek for ArcReader {
    fn poll_seek(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        from: SeekFrom,
    ) -> Poll<std::io::Result<u64>> {
        let this = self.get_mut();
        let next = match from {
            SeekFrom::Start(value) => i128::from(value),
            SeekFrom::Current(value) => this.position as i128 + i128::from(value),
            SeekFrom::End(value) => this.bytes.len() as i128 + i128::from(value),
        };
        match usize::try_from(next) {
            Ok(value) => {
                this.position = value;
                Poll::Ready(Ok(value as u64))
            }
            Err(_) => Poll::Ready(Err(Error::new(
                ErrorKind::InvalidInput,
                "内存资产 seek 越界",
            ))),
        }
    }
}

impl Reader for ArcReader {
    fn read_to_end<'a>(
        &'a mut self,
        output: &'a mut Vec<u8>,
    ) -> StackFuture<'a, std::io::Result<usize>, STACK_FUTURE_SIZE> {
        StackFuture::from(async move {
            let count = self.bytes.len().saturating_sub(self.position);
            if count > 0 {
                output.extend_from_slice(&self.bytes[self.position..]);
            }
            self.position += count;
            Ok(count)
        })
    }
    fn seekable(
        &mut self,
    ) -> Result<&mut dyn SeekableReader, bevy::asset::io::ReaderNotSeekableError> {
        Ok(self)
    }
}
