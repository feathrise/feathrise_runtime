//! 稀疏脏块布局资源、合成器和事件驱动系统。

mod async_compute; // 整图级脏块在有界AsyncCompute worker中跨帧并行解析。
mod compose; // 单个64×64块的竞争与足迹解析。
mod resources; // 活动布局、暂存版本和密度源资源。
mod systems; // 消息收集、预算化重算和原子切换。

use bevy::prelude::App; // 核心插件通过内部安装函数保持lib.rs简短。

#[cfg(feature = "blade")]
pub(crate) use compose::{VegetationCompetitionInputLayer, encode_competition_tile_inputs}; // 叶片GPU冷路径取得未决正式输入，不读取CPU最终密度作为着色器输入。
pub use resources::{
    ActiveVegetationLayout, VegetationDirtyTile, VegetationLayoutCommit, VegetationLayoutState,
    VegetationResolvedLayer, VegetationResolvedTile, VegetationSourceField,
}; // 后端和项目适配器读取公共资源与源字段。
#[cfg(feature = "blade")]
pub(crate) use resources::{VEGETATION_TILE_SIDE, VEGETATION_TILE_TEXEL_COUNT}; // 叶片后端共享固定块上传契约。
#[cfg(feature = "blade")]
pub(crate) use systems::recompute_dirty_tiles; // 叶片适配系统显式排在原子换版系统之后。

pub(super) fn install_layout_core(app: &mut App) {
    systems::install(app); // 统一注册布局消息、资源和串行系统。
}
