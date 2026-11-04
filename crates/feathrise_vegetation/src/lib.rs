//! 与内容类别无关的 Bevy 种植、空间竞争和实现后端公共库。
//!
//! 核心层只处理候选、密度、竞争、足迹与布局版本；“草、麦、树、岩石”等含义由工程
//! 自行组合。叶片绘制、通用多部件 Mesh 和近场实体代理通过正交后端接入。

mod competition; // 纯 CPU 竞争解析、优先关系与排斥图验证。
mod core_types; // 内容无关的层、组、密度、竞争和实现值类型。
mod distribution; // 世界稳定候选和正负坐标一致的确定性哈希。
mod footprint; // 圆、椭圆、矩形和 Mesh AABB 的距离及软恢复计算。
mod layout; // 事件触发脏块重算、暂存布局和原子版本切换。
mod messages; // 布局变更请求和近场代理进出消息。
mod near_field; // 与 GPU 候选同种子的近场代理差分发布。

#[cfg(feature = "blade")]
pub mod blade; // GPU叶片实现已迁入公共库，专业API保留独立命名空间。
#[cfg(feature = "blade")]
mod blade_backend; // 现有 GPU 叶片后端的兼容插件和公共 API 命名空间。
#[cfg(feature = "mesh")]
mod mesh_backend; // 通用多部件 PBR、公告板和距离 LOD 实现后端。
#[cfg(feature = "pbr-monitor")]
#[doc(hidden)]
pub mod pbr_mdi_monitor; // Bevy 0.19私有MDI成员由项目自有只读排队镜像恢复精确诊断。

use bevy::prelude::{App, Plugin}; // 公共插件只注册核心资源、消息和事件驱动系统。

#[cfg(all(feature = "blade", feature = "diagnostics"))]
pub(crate) use blade::diagnostics; // 诊断内部模块只在对应feature存在。
#[cfg(feature = "blade")]
pub(crate) use blade::{components, constants, gpu_compute, gpu_timing, resources, shaders, types}; // 迁入的叶片实现仍以原crate根路径共享内部模块，避免改写已验收GPU算法。

#[cfg(feature = "blade")]
pub use blade_backend::{
    VegetationBladeAdoptionPhase, VegetationBladeAdoptionStatus, VegetationBladeLayer,
    VegetationBladePlugin, VegetationBladeValidationLayers,
}; // 叶片后端保持原有GPU ABI，并支持可见采用观察与正式输入parity门。
pub use competition::{
    PriorityConstraintBuilder, VegetationConfigError, VegetationLayerDemand, VegetationLayerShare,
    resolve_competition,
}; // 消费者和离线生成器共享同一竞争语义。
pub use core_types::{
    ExplicitVegetationPlacement, VegetationCandidateSource, VegetationClaimSampling,
    VegetationCompetition, VegetationCompetitionGroup, VegetationCompetitionGroupId,
    VegetationCompetitionMode, VegetationDensityField, VegetationFootprint,
    VegetationFootprintScale, VegetationFootprintShape, VegetationLayerConfig, VegetationLayerId,
    VegetationPrototypeId, VegetationRealization, VegetationWeight,
};
pub use distribution::{
    VegetationCandidate, deterministic_candidate, resolved_candidate_enabled,
    stable_instance_selector, stable_world_selector,
}; // GPU、CPU和近场代理使用同一世界稳定候选及选择入口。
pub use footprint::{footprint_influence, footprint_signed_distance}; // 稀疏足迹栅格化可复用同一距离函数。
pub use layout::{
    ActiveVegetationLayout, VegetationDirtyTile, VegetationLayoutCommit, VegetationLayoutState,
    VegetationResolvedLayer, VegetationResolvedTile, VegetationSourceField,
}; // 后端只读取当前已原子切换的解析结果。
pub use messages::{
    MarkVegetationDirty, VegetationLayoutRejected, VegetationNearFieldEntered,
    VegetationNearFieldExited,
}; // 项目适配器用消息触发布局变更并消费近场代理差分。
pub use near_field::{
    VegetationNearFieldObserver, VegetationNearFieldPlugin, VegetationNearFieldRuntime,
}; // 核心只发布实例ID和Transform，不绑定物理或玩法。

#[cfg(feature = "mesh")]
pub use mesh_backend::{
    VolumeMeshCells, spawn_volume_mesh_batch,
    VegetationHeightWeightedWind, VegetationMeshAdoptionBlocker, VegetationMeshAdoptionPhase,
    VegetationMeshAdoptionStatus, VegetationMeshBatch, VegetationMeshBatchNode,
    VegetationMeshBatchPlacement, VegetationMeshBatchQuality, VegetationMeshBatchVertexMode,
    VegetationMeshBatchView, VegetationMeshBillboard, VegetationMeshBillboardCamera,
    VegetationMeshBillboardExtension, VegetationMeshBillboardFrontAxis,
    VegetationMeshBillboardMaterial, VegetationMeshBillboardNode, VegetationMeshBillboardPhase,
    VegetationMeshBillboardPlugin, VegetationMeshBillboardStatus, VegetationMeshInstance,
    VegetationMeshLayerAdoptionStatus, VegetationMeshLod, VegetationMeshLodLevel,
    VegetationMeshLodNode, VegetationMeshManagedRoot, VegetationMeshMaterialBinding,
    VegetationMeshMaterialNode, VegetationMeshPart, VegetationMeshPlugin, VegetationMeshPrototype,
    VegetationMeshPrototypeRegistry, VegetationMeshRenderIdentity,
    VegetationMeshRenderMonitorConfig, VegetationMeshRenderMonitorPlugin, VegetationMeshRenderPart,
    VegetationMeshRenderPartStats, VegetationMeshRenderPhaseStats, VegetationMeshRenderStats,
    VegetationMeshRenderTarget, VegetationMeshRenderTargetStats, VegetationMeshSystems,
    VegetationMeshWind, create_vegetation_mesh_billboard_material,
    spawn_resolved_vegetation_mesh_instance, spawn_vegetation_mesh_batch,
    spawn_vegetation_mesh_instance,
}; // 树干树冠、灌木、岩石通过同一原型、异构材质绑定与只读采用状态表达，不含类别枚举。

/// 注册内容无关的种植布局核心。
#[derive(Debug, Default, Clone, Copy)] // 插件无内容配置，可由库消费者统一安装一次。
pub struct VegetationPlugin; // 具体层、密度资产和竞争规则由项目侧注册。

impl Plugin for VegetationPlugin {
    fn build(&self, app: &mut App) {
        layout::install_layout_core(app); // 注册事件触发的验证、脏块合成和原子版本切换。
    } // 完成种植核心注册。
} // 完成公共插件实现。

/// 常用公共入口，不拍平可选叶片后端的全部专业类型。
pub mod prelude {
    pub use crate::{
        ActiveVegetationLayout, ExplicitVegetationPlacement, MarkVegetationDirty,
        VegetationCandidateSource, VegetationClaimSampling, VegetationCompetition,
        VegetationCompetitionGroup, VegetationCompetitionGroupId, VegetationCompetitionMode,
        VegetationDensityField, VegetationFootprint, VegetationFootprintScale,
        VegetationFootprintShape, VegetationLayerConfig, VegetationLayerId, VegetationLayoutCommit,
        VegetationLayoutRejected, VegetationLayoutState, VegetationNearFieldEntered,
        VegetationNearFieldExited, VegetationNearFieldObserver, VegetationNearFieldPlugin,
        VegetationPlugin, VegetationPrototypeId, VegetationRealization, VegetationSourceField,
        VegetationWeight,
    }; // 应用通常只需这些层配置、布局与插件类型。
    #[cfg(feature = "blade")]
    pub use crate::{
        VegetationBladeAdoptionPhase, VegetationBladeAdoptionStatus, VegetationBladeLayer,
        VegetationBladePlugin, VegetationBladeValidationLayers, blade,
    }; // 叶片功能开启时保留可见绑定与不可见迁移校验入口。
    #[cfg(feature = "mesh")]
    pub use crate::{
        VegetationHeightWeightedWind, VegetationMeshAdoptionBlocker, VegetationMeshAdoptionPhase,
        VegetationMeshAdoptionStatus, VegetationMeshBatch, VegetationMeshBatchNode,
        VegetationMeshBatchPlacement, VegetationMeshBatchQuality, VegetationMeshBatchVertexMode,
        VegetationMeshBatchView, VegetationMeshBillboard, VegetationMeshBillboardCamera,
        VegetationMeshBillboardExtension, VegetationMeshBillboardFrontAxis,
        VegetationMeshBillboardMaterial, VegetationMeshBillboardNode, VegetationMeshBillboardPhase,
        VegetationMeshBillboardPlugin, VegetationMeshBillboardStatus, VegetationMeshInstance,
        VegetationMeshLayerAdoptionStatus, VegetationMeshLod, VegetationMeshLodLevel,
        VegetationMeshLodNode, VegetationMeshManagedRoot, VegetationMeshMaterialBinding,
        VegetationMeshMaterialNode, VegetationMeshPart, VegetationMeshPlugin,
        VegetationMeshPrototype, VegetationMeshPrototypeRegistry, VegetationMeshRenderIdentity,
        VegetationMeshRenderMonitorConfig, VegetationMeshRenderMonitorPlugin,
        VegetationMeshRenderPart, VegetationMeshRenderPartStats, VegetationMeshRenderPhaseStats,
        VegetationMeshRenderStats, VegetationMeshRenderTarget, VegetationMeshRenderTargetStats,
        VegetationMeshWind, create_vegetation_mesh_billboard_material,
        spawn_resolved_vegetation_mesh_instance, spawn_vegetation_mesh_batch,
        spawn_vegetation_mesh_instance,
    }; // Mesh功能开启时提供异构材质、显式公告板、原型登记、采用状态与多部件实例入口。
} // 完成公共 prelude。
