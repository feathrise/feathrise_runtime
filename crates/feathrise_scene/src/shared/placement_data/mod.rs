//! 数据驱动显式场景实例运行时。
//!
//! 编辑器写入每图`placements.scene_placements.json`，本模块在完整校验后原子替换
//! GLB/GLTF实例根；草、麦等批量植被仍走各自密度系统，不滥用显式实例文件。

use feathrise_scene_core::residency::SceneInstanceContextAppExt; // 父资源与子实例资源表共同参加场景驻留。
use feathrise_scene_core::residency::SceneResidencyAppExt;

mod components; // 隐藏候选根与World实例完成身份。
mod builtin; // 内置参数化立方体与编辑器预览共用固定拓扑及弱资产缓存。
pub use builtin::{ATTRIBUTE_AUTHORING_WIRE, editable_cuboid_mesh, editable_mesh_mesh, spawn_builtin_placement_visual, set_builtin_placement_collision_enabled, set_builtin_placement_collision_enabled_world, set_builtin_placement_batch_enabled, set_builtin_placement_batch_enabled_world};
mod resources; // 热更新脏状态、候选事务与公开诊断。
#[cfg(feature = "runtime_test_support")]
pub(crate) mod runtime_test_support; // 下游回归只构造真实共享门，不放宽生产候选生命周期入口。
mod settlement; // World实例与整网格赋水必须联合就绪后才结算候选。
mod systems; // 事件观察、完整校验和实体替换。
mod terrain_collision; // 三维地形按固定字节捕获，在单飞后台烘焙完整真实网格。
mod terrain_mesh_upload;
pub use terrain_mesh_upload::TerrainMeshUpload;
#[cfg(test)]
pub(crate) mod terrain_topology_test_support;
pub(crate) use terrain_collision::InstallTerrainPatchCollider;
pub(crate) use terrain_collision::TerrainCollisionGarbage;
pub(crate) use terrain_collision::TerrainPatchColliderReady;
pub(crate) use terrain_collision::StaticTerrainShape;
pub(crate) use terrain_collision::TerrainPhysicsProxy;
pub use terrain_collision::{
    DeferredTerrainPreviewCollider, TerrainPatchColliderFailure, TerrainPatchCollisionRequest,
    terrain_placement_collision,
};

/// 局部体积预览接管同一正式根时暂时撤销旧支撑，恢复仍走Ready安装门。
pub(crate) fn suspend_terrain_collider(commands: &mut Commands, root: Entity) {
    commands
        .entity(root)
        .try_remove::<terrain_collision::TerrainColliderProxy>();
}

use crate::shared::large_scene::DynamicWaterMeshOverrideSystems; // 候选结算必须显式等待本帧单根赋水处理完成。
use crate::shared::world_asset_lifecycle::{
    WorldAssetLifecycleSystems, install_world_asset_lifecycle,
}; // Scene事件先于SpawnScene推进epoch并按固定根预算撤销旧信用。
use avian3d::prelude::{
    ColliderConstructor, ColliderConstructorHierarchy, RigidBody, TrimeshFlags,
}; // 建筑Scene0与直接primitive共用真实网格静态碰撞配置。
use bevy::prelude::*;
pub(crate) use components::ScenePlacementProgressPulse; // 水体增量调度器只通过根定向脉冲汇报真实推进。
pub use components::{
    ScenePlacementInstance, ScenePlacementPrimitiveVisual, ScenePlacementWaterEvidence,
    PlacementPendingInstance,
}; // F12与保存门读取不可变根证据，直接primitive公开窄可视标记，非空水标记限制等待帧查询。
pub(crate) use resources::ScenePlacementRuntimeState; // 共享资产生命周期域只更新当前隐藏候选的Scene epoch门。
pub use resources::{
    ScenePlacementRebuildRequest, ScenePlacementRuntimePhase, ScenePlacementRuntimeStatus,
}; // 编辑器保存重试与F3/F12共用公开运行时证据。
use feathrise_scene_content::ScenePlacementRecord; // 可选建筑语义决定是否附加物理层级。

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct InstancePlacementAdvance;

/// 每个上下文拥有自己的系统游标，复用正式摆放事务与有界清理。
pub(crate) fn add_instance_systems(schedule: &mut Schedule) {
    schedule.add_systems((
        systems::observe_placement_scene_scope,
        systems::observe_placement_data_events,
        settlement::process_pending_placement_settlements,
        systems::rebuild_data_driven_placements,
        systems::cleanup_active_placement_markers,
        systems::cleanup_retired_placement_plan,
        systems::cleanup_retired_placement_trees,
    ).chain().in_set(InstancePlacementAdvance)
        );
}
pub(crate) use systems::{
    complete_world_asset_invalidation, observe_world_asset_event, world_asset_event_is_relevant,
}; // 唯一WorldAsset消息读者通过窄接口驱动Scene依赖与采用阻断。

/// 为带建筑预设语义的显式摆放根返回统一静态三角网格碰撞组件。
pub fn building_placement_collision(
    record: &ScenePlacementRecord,
) -> Option<(RigidBody, ColliderConstructorHierarchy)> {
    if record.building_preset_id.is_none() {
        return None; // 普通资产保持既有无隐式碰撞语义；三维地形必须保留真实空腔。
    }
    Some((
        RigidBody::Static, // 建筑作者变换后通过重建或预览根重新应用，不参与动态求解。
        ColliderConstructorHierarchy::new(ColliderConstructor::TrimeshFromMeshWithConfig(
            TrimeshFlags::FIX_INTERNAL_EDGES,
        )), // 每个真实可见网格独立生成Trimesh，模型门洞和空腔按拓扑保留。
    ))
}

/// 判断一个直接primitive网格是否声明了Bevy蒙皮管线所需的联合索引和权重属性。
pub fn direct_primitive_requires_skinning(mesh: &Mesh) -> bool {
    // 预览与正式摆放必须复用同一判定，避免渲染布局分歧。
    mesh.contains_attribute(Mesh::ATTRIBUTE_JOINT_INDEX) // 关节索引会使PBR管线选择蒙皮顶点输入。
        && mesh.contains_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT) // 两项齐全时还必须有SkinnedMesh骨骼绑定，不能裸生成Mesh3d。
} // 常数次属性表查询，不读取或复制任意规模的顶点数组。

/// 为所有玩法场景安装显式实例JSON热更新。
#[derive(Default)]
pub(crate) struct ScenePlacementDataPlugin;

impl Plugin for ScenePlacementDataPlugin {
    fn build(&self, app: &mut App) {
        // 候选容器也属于完整场景；父世界空摆放批次没有ChildOf/DespawnOnExit，必须显式纳入驻留索引。
        // 否则关闭标签只退役资源槽，容器会在每次重新打开后遗留一份。
        crate::register_resident_scene_root::<components::PlacementStagingRoot>(app);
        terrain_mesh_upload::install(app);
        app.init_instance_resource::<resources::ScenePlacementRuntimeState>()
            .init_instance_resource::<terrain_collision::TerrainCollisionRuntime>()
            .init_resource::<TerrainCollisionGarbage>()
            .add_observer(terrain_collision::retire_ready_shape)
            .add_observer(terrain_collision::retire_installed_shape)
            .add_observer(terrain_collision::retire_physics_proxy)
            .add_observer(terrain_collision::register_request)
            .add_observer(terrain_collision::unregister_request)
            .add_observer(terrain_collision::invalidate_world_ready)
            .add_observer(terrain_collision::invalidate_world_root)
            .add_observer(terrain_collision::install_ready_collider)
            .add_resident_systems(
                Update,
                (
                    terrain_collision::drive_terrain_collision,
                    terrain_collision::sync_preview_physics_pose,
                )
                    .chain()
                    .after(WorldAssetLifecycleSystems::InvalidateRoots)
                    .before(crate::scenes::dynamic_scene::TerrainTopologySystems::Collect)
                    .before(systems::rebuild_data_driven_placements),
            )
            .init_instance_resource::<crate::BuildingVegetationClearanceState>() // 正式placement与F12草稿共享完整净空快照。
            .init_instance_resource::<ScenePlacementRuntimeStatus>()
            .init_instance_resource::<resources::PlacementSettlementQueue>() // 生命周期事件只把真实变化根写入8192项去重FIFO。
            .init_instance_resource::<resources::PlacementWorldAssetInvalidations>() // 资产根换代间隙拒绝旧InstanceId迟到Ready。
            .add_message::<ScenePlacementRebuildRequest>() // 原样保存重试不修改资产内容，只请求同一正式链路再建候选。
            .add_observer(settlement::invalidate_discarded_candidate_world_asset) // WorldAssetRoot替换当下撤销旧联合Ready信用。
            .add_observer(settlement::cancel_removed_candidate_world_asset) // 根真实移除时O(1)取消阻断与待结算席位。
            .add_observer(settlement::queue_inserted_placement_world_ready) // World完成门只定向入队，不扫描全部候选。
            .add_observer(settlement::queue_inserted_placement_water_status) // 水状态整组件写入与World门在队列中合并。
            .add_observer(settlement::queue_inserted_placement_collision_ready) // 建筑碰撞Ready与World/水体使用同一候选身份。
            .add_observer(settlement::invalidate_candidate_world_asset_epoch) // 资产级全局门之后按精确根撤销旧WorldReady。
            .add_resident_systems(Update, systems::observe_placement_scene_scope) // 无条件撤销离图批次身份，不能依赖玩法run condition或DespawnOnExit递归回收。
            .configure_sets(
                Update,
                crate::scenes::dynamic_scene::TerrainTopologySystems::Collect
                    .after(settlement::process_pending_placement_settlements),
            ) // World/水/碰撞授信与撤销先结算，随后才有权启动开洞。
            .add_resident_systems(
                Update,
                (
                    systems::observe_placement_data_events
                        .before(crate::scenes::dynamic_scene::TerrainTopologySystems::Collect)
                        .after(systems::observe_placement_scene_scope)
                        .after(WorldAssetLifecycleSystems::CollectEvents),
                    settlement::process_pending_placement_settlements
                        .after(systems::observe_placement_data_events)
                        .after(WorldAssetLifecycleSystems::InvalidateRoots)
                        .after(DynamicWaterMeshOverrideSystems::ProcessPending), // 单根赋水处理命令落地后再判断候选能否原子采用。
                    systems::rebuild_data_driven_placements
                        .after(crate::scenes::dynamic_scene::TerrainTopologySystems::Publish)
                        .after(settlement::process_pending_placement_settlements),
                    systems::cleanup_active_placement_markers
                        .after(systems::rebuild_data_driven_placements), // 采用后的物理标记与正式身份解耦，固定预算逐帧清理。
                )
                    .run_if(feathrise_scene_core::state::in_active_scene()),
            )
            .add_resident_systems(
                Update,
                systems::cleanup_retired_placement_plan
                    .after(systems::rebuild_data_driven_placements), // 取消/采用帧只移交所有权，随后每帧固定预算释放计划和ID集合。
            )
            .add_resident_systems(
                Update,
                systems::cleanup_retired_placement_trees
                    .after(systems::observe_placement_scene_scope)
                    .after(systems::rebuild_data_driven_placements)
                    .after(systems::cleanup_active_placement_markers)
                    .after(systems::cleanup_retired_placement_plan), // 离图后仍持续运行exclusive非递归树清理。
            );
        install_world_asset_lifecycle(app); // 首个上层插件完成唯一消息读者、索引与生命周期observer安装。
    }
}
