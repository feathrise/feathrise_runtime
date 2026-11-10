//! 自动遮雨的固定局部预算；分辨率与跨度不随场景大小增长。
pub(super) const RESOLUTION: u32 = 2048; // 256米窗口中每像素12.5厘米，屋檐精度由实机继续验收。
pub(super) const SPAN: f32 = 256.0; // 覆盖半径100米的雨盘及存活期间风漂移。
pub(crate) const DEPTH: f32 = 256.0; // 与片元归一化偏移同源的雨向深度。
pub(super) const UPSTREAM: f32 = 160.0; // 捕获中心上游160米，留下游96米容纳存活雨丝。
