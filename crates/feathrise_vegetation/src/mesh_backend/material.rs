//! 通用Mesh部件的类型擦除材质绑定适配。
//! 【功能设计】公共层只恢复调用方提供的`MeshMaterial3d<M>`，不注册具体扩展Shader、风资源或项目内容插件。

use core::{
    any::{TypeId, type_name}, // 运行时诊断核对具体Bevy材质类型，不依赖内容枚举。
    fmt,                      // Debug只暴露材质类型与非类型化资产身份。
};

use bevy::asset::{UntypedAssetId, UntypedHandle}; // 类型擦除身份用于异构材质诊断并保留原句柄强弱语义。
use bevy::ecs::system::EntityCommands; // 共享生成入口把正确的具体材质组件插入子节点。
use bevy::pbr::Material; // 任意标准或扩展PBR材质都通过Bevy Material契约接入。
use bevy::prelude::{Handle, MeshMaterial3d, StandardMaterial}; // 标准材质提供便利构造，具体句柄恢复为MeshMaterial3d。

use super::components::VegetationMeshMaterialNode; // 每个异构材质节点额外保留统一诊断身份。
use super::types::VegetationMeshMaterialBinding; // 值类型保存原句柄、类型名和单态安装函数。

impl VegetationMeshMaterialBinding {
    /// 从任意已注册Bevy材质类型创建一个类型安全绑定。
    pub fn new<M: Material>(
        handle: Handle<M>, // 调用方仍负责安装该材质类型需要的MaterialPlugin与专用资源。
    ) -> Self {
        Self {
            handle: handle.untyped(), // 擦除泛型但保留原强弱语义和内部TypeId。
            material_type_name: type_name::<M>(), // 保存稳定进程内类型名供诊断显示。
            insert: insert_typed_material::<M>, // 单态函数恢复与句柄完全一致的具体组件类型。
        } // 返回可与其他材质类型共同存入一个原型的绑定。
    } // 完成任意Bevy材质绑定构造。

    /// 为最常见的标准PBR材质创建绑定。
    pub fn standard(
        handle: Handle<StandardMaterial>, // 原PBR、AlphaMode、双面和贴图设置均留在同一资产中。
    ) -> Self {
        Self::new(handle) // 标准材质不走特殊分支，避免维护第二套生成路径。
    } // 完成标准PBR便利构造。

    /// 返回包含具体材质TypeId的非类型化资产身份。
    pub fn asset_id(&self) -> UntypedAssetId {
        self.handle.id() // 相同内部索引但不同材质类型仍是不同身份。
    } // 完成非类型化材质身份查询。

    /// 返回具体Bevy材质类型的完整Rust名称。
    pub fn material_type_name(&self) -> &'static str {
        self.material_type_name // F3无需尝试查询全部可能的MeshMaterial3d<M>。
    } // 完成材质类型名查询。

    /// 判断绑定是否属于指定Bevy材质类型。
    pub fn is_material<M: Material>(&self) -> bool {
        self.handle.type_id() == TypeId::of::<M>() // 私有构造保证TypeId与安装函数始终成对。
    } // 完成材质类型核对。

    pub(super) fn insert_into(
        &self,
        entity: &mut EntityCommands<'_>, // 当前LOD部件子节点只能安装一种真实材质组件。
    ) {
        (self.insert)(&self.handle, entity); // 调用构造时锁定的单态函数恢复MeshMaterial3d<M>。
    } // 完成具体材质组件安装。

    pub(super) fn node_identity(&self) -> VegetationMeshMaterialNode {
        VegetationMeshMaterialNode {
            asset_id: self.asset_id(), // 保存包含真实材质类型的资产身份。
            material_type_name: self.material_type_name(), // 保存无需反射表即可显示的类型名。
        } // 返回所有材质类型共享的轻量诊断组件。
    } // 完成统一材质节点身份构造。
}

impl<M: Material> From<Handle<M>> for VegetationMeshMaterialBinding {
    fn from(handle: Handle<M>) -> Self {
        Self::new(handle) // `.into()`保留调用点简洁且不丢失具体材质类型。
    } // 完成任意Bevy材质句柄转换。
}

impl fmt::Debug for VegetationMeshMaterialBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VegetationMeshMaterialBinding") // 不打印私有安装函数地址。
            .field("material_type_name", &self.material_type_name()) // 显示真实材质类型。
            .field("asset_id", &self.asset_id()) // 显示包含TypeId的非类型化资产身份。
            .finish() // 完成稳定诊断格式。
    } // 完成材质绑定Debug输出。
}

impl PartialEq for VegetationMeshMaterialBinding {
    fn eq(&self, other: &Self) -> bool {
        self.asset_id() == other.asset_id() // 资产身份已经包含具体材质TypeId。
    } // 完成材质绑定相等判断。
}

impl Eq for VegetationMeshMaterialBinding {} // 非类型化资产身份具备完整等价关系。

fn insert_typed_material<M: Material>(
    handle: &UntypedHandle, // 构造时由同一泛型M擦除，且保留调用方句柄强弱语义。
    entity: &mut EntityCommands<'_>, // 当前部件子节点。
) {
    let typed = handle.clone().typed::<M>(); // 私有不变量保证恢复类型匹配，错误构造无法通过公共API产生。
    entity.insert(MeshMaterial3d(typed)); // Bevy按真实M选择正确RenderMaterialInstances与渲染管线。
} // 完成具体材质组件恢复。
