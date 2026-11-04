//! 通用Mesh后端的ECS组件身份。
//! 【功能设计】旧根可只携带通用实例身份；只有自动生命周期创建的根才额外携带托管标记。

use bevy::asset::UntypedAssetId; // 异构材质节点用包含TypeId的统一资产身份供诊断查询。
use bevy::ecs::{lifecycle::HookContext, world::DeferredWorld}; // 身份结构变化只推进轻量拓扑代次。
use bevy::prelude::{Component, Quat}; // 组件只保存稳定身份、公告板作者姿态和只读适配参数。

use crate::{VegetationLayerId, VegetationPrototypeId}; // 实例身份不包含树、灌木或岩石枚举。

use super::resources::{VegetationMeshBillboardTopology, VegetationMeshOwnerTopology}; // 两类私有代次让稳定帧无需扫描全部根或公告板节点。
use super::types::{VegetationHeightWeightedWind, VegetationMeshBillboard, VegetationMeshLodLevel}; // LOD、公告板模式与风参数保持独立值类型。

/// 通知生命周期下一帧重新核对根身份与单一所有者。
fn mark_mesh_owner_topology_changed(mut world: DeferredWorld, _context: HookContext) {
    let Some(mut topology) = world.get_resource_mut::<VegetationMeshOwnerTopology>() else {
        return; // 未安装Mesh插件的手动生成入口保持可独立使用。
    }; // 完成可选私有资源读取。
    topology.advance(); // 只推进O(1)代次，PostUpdate变化帧再执行完整确定性差分。
} // 完成根身份拓扑变化通知。

/// 通知公告板诊断只在节点集合结构变化后重新聚合一次。
fn mark_mesh_billboard_topology_changed(mut world: DeferredWorld, _context: HookContext) {
    let Some(mut topology) = world.get_resource_mut::<VegetationMeshBillboardTopology>() else {
        return; // 未安装Mesh插件的手动节点保持可独立存在。
    }; // 完成可选私有资源读取。
    topology.advance(); // 只推进O(1)代次，稳定帧不再扫描全部公告板节点。
} // 完成公告板拓扑变化通知。

/// 通用Mesh根实例身份。
#[derive(Clone, Copy, Debug, Component, PartialEq, Eq)]
#[component(
    immutable,
    on_insert = mark_mesh_owner_topology_changed,
    on_discard = mark_mesh_owner_topology_changed
)]
pub struct VegetationMeshInstance {
    pub layer: VegetationLayerId, // 布局层用于生命周期、诊断和玩法适配。
    pub prototype: VegetationPrototypeId, // 内容原型只由消费者解释。
    pub instance_id: u64,         // 与候选和近场代理共享稳定实例ID。
}

/// 单个Mesh渲染子节点的通用诊断身份。
#[derive(Clone, Copy, Debug, Component, PartialEq, Eq)]
pub struct VegetationMeshLodNode {
    pub level: VegetationMeshLodLevel, // 当前节点所属LOD档。
    pub part_id: u32,                  // 当前节点所属原型部件。
    pub billboard: bool,               // 公告板适配器无需检查名称或内容类别。
}

/// 一个可安全提取到RenderApp的通用Mesh渲染身份。
#[derive(Clone, Copy, Debug, Component, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VegetationMeshRenderIdentity {
    pub layer: VegetationLayerId, // 分箱归属必须先匹配无限布局层，禁止只按共享Mesh句柄猜测内容。
    pub prototype: VegetationPrototypeId, // 同层原型热替换或跨地图复用Mesh时继续保持独立身份。
    pub level: VegetationMeshLodLevel, // 高、中、低和公告板由消费项目解释具体数值。
    pub part_id: u32,             // 树干、树冠或任意多部件使用原型声明的稳定编号。
    pub billboard: bool,          // F3可独立核对最远公告板而不解析节点名称或材质类型。
} // 完成只读RenderApp分箱身份；该组件本身不改变PBR排队或绘制命令。

impl bevy::render::sync_component::SyncComponent for VegetationMeshRenderIdentity {
    type Target = Self; // 手动提取仍写入同名组件；主世界移除时同步插件据此清理渲染世界残留身份。
}

/// 兼容保留的旧CPU公告板相机标记。
///
/// GPU公告板现在按每个渲染视图读取Bevy的LOD观察位置，因此本标记不再参与朝向或诊断；
/// 保留类型只避免既有消费代码在迁移期间立即失去组件身份。
#[derive(Clone, Copy, Debug, Component, Default, PartialEq, Eq)]
pub struct VegetationMeshBillboardCamera; // 新代码无需再向玩法相机插入该标记。

/// 一个公告板部件节点的GPU朝向配置与确定性作者姿态。
#[derive(Clone, Copy, Debug, Component, PartialEq)]
#[component(
    immutable,
    on_insert = mark_mesh_billboard_topology_changed,
    on_discard = mark_mesh_billboard_topology_changed
)]
pub struct VegetationMeshBillboardNode {
    pub billboard: VegetationMeshBillboard, // 固定、绕Y轴或全向模式不依赖内容类别。
    pub rest_rotation: Quat, // CPU始终保留作者局部旋转；GPU退化方向也使用原世界矩阵回退。
    pub gpu_material_ready: bool, // 动态模式只有绑定专用扩展材质时才计为GPU可用。
}

/// 一个异构Bevy材质子节点的统一只读身份。
#[derive(Clone, Copy, Debug, Component, PartialEq, Eq)]
pub struct VegetationMeshMaterialNode {
    pub asset_id: UntypedAssetId, // 身份同时包含具体材质TypeId与资产索引/UUID。
    pub material_type_name: &'static str, // F3可显示标准或扩展材质类型而无需内容分支。
}

/// 可选高度权重风参数组件。
#[derive(Clone, Copy, Debug, Component, PartialEq)]
pub struct VegetationMeshWind(pub VegetationHeightWeightedWind); // 专业材质适配插件读取一次并共享GPU材质。

/// 标记由通用Mesh生命周期独占管理的根实体。
#[derive(Clone, Copy, Debug, Component, PartialEq, Eq)]
#[component(
    immutable,
    on_add = mark_mesh_owner_topology_changed,
    on_remove = mark_mesh_owner_topology_changed
)]
pub struct VegetationMeshManagedRoot {
    pub layout_revision: u64,    // 当前根已经核对并采用的活动布局revision。
    pub prototype_revision: u64, // 当前根使用的注册原型revision，热替换时必须重建。
} // 旧树木/景观根不携带本标记，因此注册原型前后都不会被通用后端误删。
