//! F3的Clipmap统计：按固定实例预算完成一轮后发布，复用场景正式计数而不扫描几何。

mod resources;
mod systems;

pub(super) use resources::ClipmapMonitorState;
pub(super) use systems::sample_clipmaps;

#[cfg(test)]
mod tests;
