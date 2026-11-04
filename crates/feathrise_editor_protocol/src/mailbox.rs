//! 相机、视口和待发草稿的最新值邮箱；可靠事务与按键边沿不得使用可覆盖邮箱。

use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishResult {
    Published,
    Busy,
    Stale,
}

struct Slot<T> {
    serial: u64,
    value: Option<T>,
}

pub struct LatestMailbox<T> {
    slot: Arc<Mutex<Slot<T>>>,
}

impl<T> Clone for LatestMailbox<T> {
    fn clone(&self) -> Self {
        Self {
            slot: Arc::clone(&self.slot),
        }
    }
}

impl<T> Default for LatestMailbox<T> {
    fn default() -> Self {
        Self {
            slot: Arc::new(Mutex::new(Slot {
                serial: 0,
                value: None,
            })),
        }
    }
}

impl<T> LatestMailbox<T> {
    /// 调用方只在 Published 时放弃重试；Busy 保留自己的 Arc，下一帧重新投递。
    pub fn publish(&self, serial: u64, value: T) -> PublishResult {
        let Ok(mut slot) = self.slot.try_lock() else {
            return PublishResult::Busy;
        };
        if serial == 0 || serial <= slot.serial {
            return PublishResult::Stale;
        }
        slot.serial = serial;
        slot.value = Some(value);
        PublishResult::Published
    }

    pub fn take(&self) -> Option<(u64, T)> {
        let mut slot = self.slot.try_lock().ok()?;
        let value = slot.value.take()?;
        Some((slot.serial, value))
    }
}
