//! 数据驱动传送图运行时。
//!
//! 单张地图只在私有JSON声明传送点，本模块统一完成跨地图目标解析、完整校验和热更新替换。

use feathrise_scene_core::residency::MapInstanceContextAppExt; // 父资源与子实例资源表共同参加地图驻留。
use feathrise_scene_core::residency::SceneResidencyAppExt;

mod resources; // 热更新事务状态与公开诊断。
mod systems; // 资产事件观察、图解析、实体替换和失败处理。
mod bindings; // 跨地图地形关联索引共用一个后台计算槽。

pub use resources::{MapPortalRuntimePhase, MapPortalRuntimeStatus}; // F3与编辑器只读取结构化状态。
pub(crate) use systems::{
    invalidate_portal_runtime_on_scene_exit, observe_portal_data_events,
    rebuild_data_driven_portals,
    follow_local_portal_arrivals,
}; // 场景根插件注册唯一写侧与离场会话失效门。

use bevy::prelude::*; // App、Plugin和Update调度。
use feathrise_map_content::MapContentPlugin; // 传送运行时先注册公共地图资产目录。

/// 为所有玩法地图安装数据驱动传送图。
#[derive(Default)]
pub(crate) struct MapPortalDataPlugin;

impl Plugin for MapPortalDataPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MapContentPlugin)
            .init_resident_resource::<bindings::PortalBindingResolver>()
            .init_instance_resource::<resources::MapPortalRuntimeState>()
            .init_instance_resource::<MapPortalRuntimeStatus>();
        for &scene in feathrise_scene_core::state::ScenePhase::active_phases() {
            app.add_resident_systems(
                OnExit(scene),
                invalidate_portal_runtime_on_scene_exit, // 离场后同枚举或同MapId返回也必须重建已销毁实体。
            );
        }
        app.add_resident_systems(
            Update,
            (
                observe_portal_data_events,
                rebuild_data_driven_portals
                    .after(observe_portal_data_events)
                    .before(feathrise_billboard_text::spawn_billboard_texts), // 先落地地图重建/旧锚点清理，再消费最新请求，避免语言重放读取已淘汰的来源。
            )
                .run_if(feathrise_scene_core::state::in_active_scene()),
        );
    }
}
