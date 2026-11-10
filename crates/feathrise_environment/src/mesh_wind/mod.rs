//! 通用 PBR Mesh GPU 顶点风材质入口。
//! 【功能设计】本模块只拥有具体材质、噪声资源、风质量参数和保守边界，不解释树木、灌木落点或生命周期。

mod helpers; // 风力安全化、LOD细节分级、GPU统一布局和树高参考边界纯函数。
mod material; // 项目自有扩展材质创建、逐帧历史风参数更新和插件注册。
mod noise; // 固定尺寸周期值噪声生成及Startup唯一Image登记系统。
mod resources; // 场景装配层等待的共享风噪声资源类型。
mod shaders; // crate内置风位移库、主渲染和预通道Shader注册入口。
mod types; // CPU帧参数、GPU固定Uniform和材质专用化键等非ECS纯数据。

use bevy::prelude::{App, Plugin}; // 独立feature通过自身Plugin安装噪声资源和具体扩展材质管线。

/// 共享 Mesh 风材质的几何质量档。
/// 内容域在调用边界显式转换自身LOD，避免本模块反向依赖树木或景观类型。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MeshWindLodLevel {
    #[default]
    High, // 近景叶簇保留完整局部细摆。
    Medium,    // 中景叶簇降低局部细摆幅度。
    Low,       // 远景叶簇只保留宏观风。
    Billboard, // 固定公告板或不支持组合使用静止回退。
}

/// 共享 Mesh 风材质的几何部件语义。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MeshWindPart {
    Stem,      // 木质或刚性茎干只做根部固定的宏观弯曲。
    Foliage,   // 树冠、灌木等叶簇可按质量档叠加局部细摆。
    Billboard, // 最远固定公告板不安装本材质，保留安全回退语义。
}

/// 共享 Mesh 风材质插件。
/// TreePlugin会先安装本插件；仅启用`mesh-wind`时则由EnvironmentPlugin直接安装。
#[derive(Debug, Default, Clone, Copy)]
pub struct MeshWindPlugin;

impl Plugin for MeshWindPlugin {
    fn build(&self, app: &mut App) {
        material::install_mesh_wind_material_pipeline(app); // 注册唯一噪声资源以及主渲染、阴影和运动向量一致的材质管线。
    }
}

pub use material::{
    // 场景层从共享Mesh风子域取得完整GPU风材质生命周期接口。
    TreeWindMaterial,
    combine_tree_part_aabbs,
    create_tree_wind_material, // 创建阶段复用现有PBR并合并树干树冠完整高度。
    expanded_tree_wind_culling_aabb,
    update_tree_wind_material, // 运行期扩展剔除范围并推进当前/上一帧风参数。
    update_tree_wind_material_with_phase, // 项目天气桥在保留旧API的同时补偿变速相位。
}; // 对场景层只公开树木与灌木需要的材质生命周期和AABB接口，不暴露散布系统。
pub use resources::TreeWindNoiseTexture; // 保持场景装配层原公共名称，由本crate自有资源替代第三方类型身份。
