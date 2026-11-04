//! 作者快照的进程内不透明版本票据；浅克隆保留身份，新作者值显式换票据。

use std::sync::{Arc, atomic::AtomicU8};

/// 保存交接只比较这份快照的拥有权，不遍历作者数组，也不依赖会回绕的ECS tick。
#[derive(Clone, Debug)]
pub struct AuthoringVersion(Arc<AtomicU8>);

impl AuthoringVersion {
    pub(crate) fn from_shared_state(state: Arc<AtomicU8>) -> Self {
        Self(state) // 摆放初始版本复用既有批次门，不为每个正式根额外分配票据。
    }
}

impl Default for AuthoringVersion {
    fn default() -> Self {
        Self(Arc::new(AtomicU8::new(0))) // 值本身无公开语义，唯一信用来自仍被持有的Arc身份。
    }
}

impl PartialEq for AuthoringVersion {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for AuthoringVersion {}
