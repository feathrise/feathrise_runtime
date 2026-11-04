//! P3近景离水水花功能域装配。
mod components; // 固定槽组件。
mod constants; // 容量与寿命。
mod helpers; // 固定候选和遮罩采样。
mod resources; // 当前水体身份。
mod systems;
mod types; // 局部时钟。 // 运行与初始化。
pub(super) use resources::SplashWaterTarget; // 既有水体遍历写入唯一身份。
pub(super) use systems::{initialize_rain_splashes, update_rain_splashes}; // 水面插件装配入口。
#[cfg(test)]
mod tests; // 无窗口实际系统初始化与暂停/切图回归。
