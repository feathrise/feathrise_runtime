//! 动态地图白模到通用RGBA地表材质的原子接管。

mod author; // 作者完整PBR覆盖与保存事件交接。
pub use author::DynamicSurfaceAuthorPreview; // 编辑器只写输入，不抢正式组件。
mod components;
mod helpers;
mod resources;
mod systems;
mod transactions; // 统一失败白模事务，首载与热更新不得分叉。

pub(super) use resources::DynamicSurfaceState;
pub(super) use systems::{
    install_dynamic_surface, observe_dynamic_surface_assets, prepare_dynamic_surface_definition,
};
pub(super) use transactions::{begin_dynamic_surface, clear_dynamic_surface}; // 进入与退出都由完整事务模块维护代次和资源生命周期。
