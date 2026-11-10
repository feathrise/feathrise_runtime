//! 三维场景驻留的通用所有权契约；移动资源，保留实体，不克隆场景或GPU资产。

mod asset_events;
mod components;
mod instance_context; // 同时运行的完整子场景拥有独立状态与源坐标桥。
mod render;
mod resources;
mod snapshot;
mod terrain_support; // 已采用碰撞覆盖与活动半径，不依赖角色分类或游戏状态。
#[cfg(test)]
mod tests;

pub use asset_events::{ResidentAssetEvent, ResidentAssetEventReader};
pub use instance_context::*;
pub use components::{
    ResidentSceneEntity, ResidentSceneToken, ResidentSceneActivated, ResidentSceneParking,
};
pub use resources::{SceneResidencyActivity, SceneResidencyRuntimeSet};
pub use snapshot::{
    ResidentResourceSnapshot, SceneResidencyAppExt, restore_resident_resources,
    take_resident_resources,
};
pub use terrain_support::{TerrainActivityPaused, TerrainCollisionRegion, TerrainSupportRows};

use bevy::{ecs::schedule::ScheduleLabel, prelude::*};

/// 场景切换期间只冻结场景运行时；窗口、作者外壳、资产加载与渲染事件循环继续推进。
pub fn resident_runtime_active(activity: Option<Res<SceneResidencyActivity>>) -> bool {
    activity.is_none_or(|state| !state.switching)
}

/// 资源登记是可独立安装的叶子能力；场景宿主负责启动/结束驻留事务。
pub struct SceneResidencyResourcePlugin;

impl Plugin for SceneResidencyResourcePlugin {
    fn build(&self, app: &mut App) {
        render::install(app);
        app.init_resource::<SceneResidencyActivity>();
        for schedule in [
            PreUpdate.intern(),
            Update.intern(),
            PostUpdate.intern(),
            FixedUpdate.intern(),
        ] {
            app.configure_sets(
                schedule,
                SceneResidencyRuntimeSet.run_if(resident_runtime_active),
            );
        }
    }
}
