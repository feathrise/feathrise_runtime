// 传送门 — 组件定义

use bevy::prelude::*; // Component 等 ECS 常用类型
use feathrise_scene_core::state::ScenePhase; // 游戏场景枚举

/// 传送门标记组件
#[derive(Component, Debug, Clone)] // 组件 + 调试 + 克隆支持
pub struct Portal {
    pub target_scene: ScenePhase,       // 目标场景：碰触后切换到此场景
    pub target_scene_id: Option<String>, // DynamicSceneRoot使用的稳定SceneId；静态场景保持None。
    pub target_display_name: String,   // 目标清单中的实际场景名，供LoadingScene显示。
    pub spawn_position: Vec3,          // 到达目标场景后玩家的出生坐标
    pub label: String,                 // 显示标签：门上方文字提示（如 "前往郊区"）
}

/// 传送门上方显示文字的锚点，保留作者原文，不包含语言或翻译策略。
#[derive(Component, Debug, Clone)]
pub struct PortalLabel {
    pub(super) source: String, // 传送模块维护作者原文，应用仅通过只读接口消费。
}

impl PortalLabel {
    /// 作者原文可供应用的显示层读取，传送模块不解释语言或翻译键。
    pub fn source(&self) -> &str {
        &self.source
    }
}
