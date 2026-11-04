//! 水面反射租约只保存当前相机和地图代次，不拥有DepthPrepass组件。

use bevy::prelude::*; // 小型资源与实体身份。
use feathrise_map_content::MapId; // 动态地图完整身份。
use feathrise_settings::GraphicsRuntimeLeaseId; // 共享附件可撤销票据。
use feathrise_scene_core::state::ScenePhase; // 菜单和玩法生命周期。

#[derive(Resource, Default)] // 初始无深度所有权。
pub(super) struct WaterReflectionCameraState {
    // 稳定帧不重新创建租约。
    pub lease: Option<(Entity, GraphicsRuntimeLeaseId)>, // 本功能唯一租约，不影响其他持有者。
    pub context: Option<(ScenePhase, Option<MapId>)>,     // 同枚举动态切图也必须丢弃旧资格。
} // 完成最小状态。
