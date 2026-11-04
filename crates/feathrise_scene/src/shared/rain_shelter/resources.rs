//! 所有显式代理源仅保留小记录，超32源拒绝整批而不静默遗漏。

use super::types::ShelterSource;
use bevy::prelude::*; // 实体ID、资源与地图状态。
use feathrise_map_content::MapId; // 动态地图不能只比较GameScene枚举。
use feathrise_scene_core::state::ScenePhase; // 菜单/加载与玩法地图区分。
use std::collections::{HashMap, HashSet}; // 变更/移除按实体O(1)访问。 // 每源缓存固定小代理和根身份。

#[derive(Resource, Default)] // 稳定帧不会遍历场景实体。
pub(super) struct RainShelterRegistry {
    // 地图根卸载时按RemovedComponents清理。
    pub sources: HashMap<Entity, ShelterSource>, // 每个源仅保存地图身份、祖先和一个受检OBB。
    pub owners: HashMap<Entity, HashSet<Entity>>, // 根到代理索引，隐藏候选根无需逐源检查。
    pub descendants: HashMap<Entity, HashSet<Entity>>, // 祖先重挂只使相关代理重新解析根。
    pub context: Option<(ScenePhase, Option<MapId>)>, // 当前正式地图完整身份。
    pub error: Option<&'static str>,             // 相同错误只诊断一次。
} // 完成来源资源。
