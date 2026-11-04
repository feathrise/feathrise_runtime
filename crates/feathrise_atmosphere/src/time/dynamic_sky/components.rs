//! 动态天空私有组件：记录相机接管前状态，并标识由本领域采用或生成的大气实体。

use bevy::core_pipeline::Skybox; // 玩法动态天空启用期间移除可见 Cube，离场后精确归还。
use bevy::pbr::AtmosphereSettings; // 保存相机原生大气配置完整副本。
use bevy::prelude::*; // Component 派生与基础类型。

/// 动态天空接管一台相机前冻结的完整作者状态。
#[derive(Component, Clone)]
pub(in crate::time) struct DynamicSkyCameraRuntime {
    pub(super) original_settings: Option<AtmosphereSettings>, // 外部已有质量参数按值恢复，缺失则离场移除。
    pub(super) original_skybox: Option<Skybox>, // 菜单或其他非玩法用途的可见 Cube 不被永久删除。
    pub(super) original_hdr: bool,              // 仅由大气 required 关系引入的 HDR 在离场时清理。
}

/// 本领域采用或生成的唯一地球大气实体标记。
#[derive(Component, Clone, Copy, Debug)]
pub(in crate::time) struct DynamicSkyAtmosphere; // 标记不保存参数，实际物理值由 Bevy Atmosphere 组件持有。
