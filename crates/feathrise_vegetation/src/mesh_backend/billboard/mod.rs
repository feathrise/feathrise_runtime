//! 通用Mesh公告板的内容无关GPU朝向功能域。
//! 【功能设计】固定交叉面保留原PBR路径；动态平面在每个渲染视图的顶点Shader中独立朝向。

mod material; // StandardMaterial扩展只替换主渲染、阴影和预通道顶点阶段。
mod shaders; // crate内置WGSL使用固定句柄，宿主无需复制Shader资产。
mod systems; // PostUpdate只在节点拓扑变化后发布一次完整诊断快照。

pub use material::{
    VegetationMeshBillboardExtension, VegetationMeshBillboardMaterial,
    VegetationMeshBillboardPlugin, create_vegetation_mesh_billboard_material,
}; // 消费层显式创建扩展材质并安装独立GPU管线插件。
pub(super) use systems::synchronize_vegetation_mesh_billboard_status; // Mesh基础插件只注册变化驱动状态入口。
