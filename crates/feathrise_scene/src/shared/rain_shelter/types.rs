//! 代理来源缓存保持根身份与最多64级祖先，局部重挂只使关联源失效。

use bevy::prelude::*; // 实体、变换与场景身份。
use feathrise_environment::rain_shelter::RainShelterVolume; // 小型已验证OBB。
use feathrise_scene_content::SceneId; // 动态场景完整键。
use feathrise_scene_core::state::ScenePhase; // 静态场景身份。

pub(super) struct ShelterSource {
    // 不保存任何Mesh或大作者快照。
    pub context: Option<(ScenePhase, Option<SceneId>)>, // 无显式根时仍保留创建时场景身份。
    pub volume: Option<RainShelterVolume>,           // 无效体使活动批次整批拒绝。
    pub owner: Entity,                               // 缓存正式placement或场景根，稳定帧仅单次get。
    pub ancestors: Vec<Entity>,                      // 最多64级，供重挂事件定向失效。
} // 完成单源缓存。
