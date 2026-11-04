//! 作者视图的共享GPU实现；编辑器UI和工程视口只发布相机参数，不复制渲染算法。
//! 普通游戏不安装这两个Plugin；参数属于已有相机，不新建地图实体或改写场景材质。
mod camera;
mod components;
mod grid_render;
mod render;
mod wireframe;
pub mod tools; // 语义工具预览与协议解耦，编辑器与工程共享同一份几何规则。
#[cfg(test)]
mod gpu_tests;
pub use camera::apply_shading;
pub use components::PrepassLease;
pub use components::{GridView, ShadingView};
pub use grid_render::GridRenderPlugin;
pub use render::ShadingRenderPlugin;
