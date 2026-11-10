//! 内容无关的多部件 PBR、公告板和距离 LOD Mesh 后端。
//! 【功能设计】原型登记与活动布局生命周期只管理通用技术身份；材质、公告板朝向和项目物理适配保持正交。

use feathrise_scene_core::residency::SceneResidencyAppExt;

mod batch; // 海量落点按空间块共享实例缓冲，ECS与分箱不再随逐株数量增长。
mod billboard; // 固定交叉面与GPU逐视图动态公告板的内容无关材质适配。
mod components; // Mesh根、LOD节点、风参数和自动生命周期托管身份。
mod lifecycle; // 活动revision、配置代次与原型登记变化驱动的确定性差分。
mod material; // 标准与任意扩展Material共用的类型擦除句柄安装适配。
mod render_monitor; // 可选RenderApp监控按层、原型、LOD与部件读取真实PBR公开分箱。
mod resources; // 原型注册表、私有活动实体表和公开只读采用状态。
mod spawn; // 保持既有手动生成入口并复用共享层级装配。
mod types; // LOD、部件、风参数和通用Mesh原型值类型。

use bevy::prelude::{App, IntoScheduleConfigs, Plugin, PostUpdate, SystemSet, TransformSystems}; // Mesh差分与公告板都在最终变换传播前收口，公开SystemSet供项目原子接管适配器可靠排序。

pub use batch::{
    VolumeMeshCells, spawn_volume_mesh_batch,
    VegetationMeshBatch, VegetationMeshBatchNode, VegetationMeshBatchPlacement,
    VegetationMeshBatchQuality, VegetationMeshBatchView, spawn_vegetation_mesh_batch,
}; // 动态场景与编辑器使用无作者总量上限的分块批次入口。
pub use billboard::{
    VegetationMeshBillboardExtension, VegetationMeshBillboardMaterial,
    VegetationMeshBillboardPlugin, create_vegetation_mesh_billboard_material,
}; // 动态公告板显式安装PBR扩展材质；固定交叉面继续使用任意原材质。
pub use components::{
    VegetationMeshBillboardCamera, VegetationMeshBillboardNode, VegetationMeshInstance,
    VegetationMeshLodNode, VegetationMeshManagedRoot, VegetationMeshMaterialNode,
    VegetationMeshRenderIdentity, VegetationMeshWind,
}; // 消费者与诊断按稳定组件身份识别根、LOD和自动托管所有者。
pub use render_monitor::VegetationMeshRenderMonitorPlugin; // 生产组合必须显式选择监控，不由基础Mesh插件自动改变RenderApp调度。
pub use resources::{
    VegetationMeshAdoptionBlocker, VegetationMeshAdoptionPhase, VegetationMeshAdoptionStatus,
    VegetationMeshBillboardPhase, VegetationMeshBillboardStatus, VegetationMeshLayerAdoptionStatus,
    VegetationMeshPrototypeRegistry, VegetationMeshRenderMonitorConfig, VegetationMeshRenderStats,
}; // 项目只登记内容原型并读取状态，不直接修改私有活动实体表。
pub use spawn::{spawn_resolved_vegetation_mesh_instance, spawn_vegetation_mesh_instance}; // 手动fixture和旧示例继续使用原路径。
pub use types::{
    VegetationHeightWeightedWind, VegetationMeshBatchVertexMode, VegetationMeshBillboard,
    VegetationMeshBillboardFrontAxis, VegetationMeshLod, VegetationMeshLodLevel,
    VegetationMeshMaterialBinding, VegetationMeshPart, VegetationMeshPrototype,
    VegetationMeshRenderPart, VegetationMeshRenderPartStats, VegetationMeshRenderPhaseStats,
    VegetationMeshRenderTarget, VegetationMeshRenderTargetStats,
}; // 原型保持内容无关，不新增树木、灌木或岩石枚举。

/// 通用Mesh后端的稳定调度边界。
///
/// 项目内容接管器可以在生命周期前暂存旧所有者，并在生命周期后验证新根；
/// 公共核心仍不解释树木、灌木、岩石或任何物理组件。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SystemSet)]
pub enum VegetationMeshSystems {
    Lifecycle, // 活动布局、原型注册表与现有所有者共同驱动根差分。
    Billboard, // 新根落地后只聚合变化驱动公告板状态，不再逐帧写Transform。
} // 完成内容无关的Mesh调度边界定义。

/// 通用Mesh后端插件；基础PBR实例无需逐帧系统。
/// 固定交叉impostor无需变换更新，动态公告板由独立GPU材质插件在顶点阶段逐视图朝向。
/// 自动生命周期只消费显式落点与已原子提交布局，稳定帧没有公告板节点扫描或Transform写回。
#[derive(Clone, Copy, Debug, Default)]
pub struct VegetationMeshPlugin; // 项目风Shader、RenderApp专业分箱和物理仍由后续独立适配插件组合。

impl Plugin for VegetationMeshPlugin {
    fn build(&self, app: &mut App) {
        batch::install_batch_backend(app); // 分块批次与旧逐实例入口并存，只有调用新入口的层改变渲染所有权。
        app.init_resident_resource::<VegetationMeshPrototypeRegistry>() // 空注册表不会接管任何现有可见对象。
            .init_resident_resource::<resources::VegetationMeshOwnerTopology>() // 根结构变化以O(1)代次触发单一所有权复核。
            .init_resident_resource::<resources::VegetationMeshRuntime>() // 私有表只记录本插件实际生成的根。
            .init_resident_resource::<VegetationMeshAdoptionStatus>() // F3与迁移门只读取完整状态快照。
            .init_resident_resource::<resources::VegetationMeshBillboardTopology>() // 组件钩子只用O(1)代次标记节点结构变化。
            .init_resident_resource::<VegetationMeshBillboardStatus>() // 公告板状态默认NoBillboards且不再绑定单一相机。
            .configure_sets(
                PostUpdate,
                (
                    VegetationMeshSystems::Lifecycle, // 项目适配器可在该边界前暂存旧所有者。
                    VegetationMeshSystems::Billboard, // 项目适配器可在该边界前完成新根姿态确认。
                )
                    .chain() // 生命周期Commands先落地，公告板系统才能读取完整新层级。
                    .before(TransformSystems::Propagate), // 两个公共边界都在最终GlobalTransform传播前收口。
            )
            .add_resident_systems(
                PostUpdate,
                (
                    lifecycle::synchronize_mesh_lifecycle.in_set(VegetationMeshSystems::Lifecycle), // 布局Update完成后先做同代差分。
                    billboard::synchronize_vegetation_mesh_billboard_status
                        .in_set(VegetationMeshSystems::Billboard), // 同帧新节点只发布GPU材质就绪诊断，不改Transform。
                )
                    .before(TransformSystems::Propagate), // Bevy渲染提取读取本帧已经传播的最终GlobalTransform。
            );
    } // 完成通用Mesh生命周期基础注册。
} // 完成Mesh后端插件实现。

#[cfg(test)] // 程序化fixture不启动窗口、AssetServer或RenderApp。
mod tests; // 独立锁定手动生成、登记、换版、拒绝、回收和单一所有者。
