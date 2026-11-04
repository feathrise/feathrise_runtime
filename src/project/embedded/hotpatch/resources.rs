//! 原地热补丁的有限运行事实；作者和工程代码只能读取真实采用代次。
use bevy::prelude::*;

#[derive(Resource, Default)]
pub struct FunctionHotpatchState {
    pub(super) epoch: u64,
}

impl FunctionHotpatchState {
    /// 只有实际跳表采用成功并通知主 App 后递增，不由源码文本或宿主请求号推算。
    pub fn epoch(&self) -> u64 { self.epoch }
}

#[cfg(all(debug_assertions, windows))]
pub(super) mod active {
    use super::super::types::{Outcome, Request};
    use std::{sync::{Arc, atomic::{AtomicBool, AtomicU64}, mpsc}, thread::JoinHandle};

    pub(in super::super) struct Runtime {
        pub sender: mpsc::SyncSender<Request>,
        pub receiver: mpsc::Receiver<Outcome>,
        pub stop: Arc<AtomicBool>,
        pub map_generation: Arc<AtomicU64>,
        pub worker: Option<JoinHandle<()>>,
        pub in_flight: Option<Request>,
        pub reply: Option<(Outcome, u64)>,
        pub ready_sent: bool,
        pub last_patch_id: u64,
        pub last_source_generation: u64,
        pub epoch: u64,
        pub base_build: String,
        pub aslr_reference: u64,
    }

    impl Drop for Runtime {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Release);
            // 不在 App/帧析构等待文件IO或装载；宿主仍需由精确Job确认整个进程退出。
            if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
                let _ = self.worker.take().unwrap().join();
            }
        }
    }
}
