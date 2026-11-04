//! 动态地图区域边界静态碰撞、热更新与F12未保存覆盖。

mod components; // 原子边界根与逐段诊断组件。
mod derivation; // 分帧输入捕获与后台空间块碰撞缓存。
mod helpers; // 作者X/Z线段到Avian Cuboid的纯几何派生。
mod resources; // 边界状态、公开诊断和F12覆盖资源。
mod visualization; // 游戏内同源碰撞范围半透明显示。
pub(super) use visualization::{show_air_wall_outlines, show_air_walls}; // 显示系统不写碰撞。
mod systems; // 首载、热更、原子替换与场景清理系统。

pub(super) use resources::DynamicMapBoundaryState; // 动态地图插件初始化唯一写侧状态。
pub use resources::{
    DynamicMapBoundaryOverride, DynamicMapBoundaryPhase, DynamicMapBoundaryStatus,
}; // F12和诊断只使用最小公开接口。
pub(super) use systems::*; // 插件层注册边界生命周期系统。
