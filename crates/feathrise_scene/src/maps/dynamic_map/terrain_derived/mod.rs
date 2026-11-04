//! 动态地图 GLB 地形的有界捕获、后台派生与原子采用域。

mod resources; // 公开完成值、阶段以及内部单飞捕获/任务状态。
mod systems; // 当前地形发现、固定预算捕获、非阻塞轮询和原子采用。

pub(super) use resources::{DynamicTerrainCommitState, DynamicTerrainDerivationState}; // 插件初始化唯一派生写侧和同帧提交令牌。
pub use resources::{
    // 场景外只读取不可变完成值和结构化状态。
    DynamicTerrainHeightCache,
    DynamicTerrainHeightData,
    DynamicTerrainHeightPhase,
    DynamicTerrainHeightStatus,
}; // 完成公开接口导出。
pub(super) use systems::{
    // 动态地图插件按严格链顺序注册四个系统。
    capture_dynamic_terrain_source,
    clear_dynamic_terrain_derivation,
    commit_dynamic_terrain_derivation,
    finish_dynamic_terrain_commit,
    observe_dynamic_terrain_source,
    poll_dynamic_terrain_derivation,
    prepare_dynamic_terrain_commit,
}; // 完成内部系统导出。
