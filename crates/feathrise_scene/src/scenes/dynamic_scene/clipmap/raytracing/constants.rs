//! 地形代理的输入、CPU 单飞和几何硬预算；不扩大现有动态 Solari 上传上限。

pub(super) const MAX_PROXY_CELLS: u64 = 512;
pub(super) const MAX_LIVE_PAGES: usize = 512;
pub(super) const CAPTURE_PAGES_PER_FRAME: usize = 64;
pub(super) const MAX_FORMAL_PAGES: usize = 4;
pub(super) const PREPARATION_TIMEOUT_SECONDS: f32 = 30.0;
