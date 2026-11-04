//! 通用Mesh动态公告板的GPU PBR扩展材质。
//! 【功能设计】扩展只替换顶点阶段；基础颜色、贴图、AlphaMask、双面、雾和光照继续由StandardMaterial负责。

use bevy::asset::Asset; // 扩展与StandardMaterial组合后进入Bevy Assets和RenderAssets。
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin, StandardMaterial}; // 使用官方PBR扩展契约复用基础材质全部片元语义。
use bevy::prelude::{App, Plugin, UVec4}; // 插件注册管线，UVec4固定承载16字节GPU配置。
use bevy::reflect::TypePath; // 自有扩展资产需要稳定Rust类型路径。
use bevy::render::render_resource::AsBindGroup; // 把模式与正面轴映射到材质binding 100。
use bevy::shader::ShaderRef; // 主渲染与预通道返回crate内置Shader固定句柄。

use super::super::types::{VegetationMeshBillboard, VegetationMeshBillboardFrontAxis}; // 公共值类型是CPU原型与GPU材质之间的唯一配置来源。
use super::shaders::{
    VEGETATION_BILLBOARD_PREPASS_SHADER_HANDLE, VEGETATION_BILLBOARD_VERTEX_SHADER_HANDLE,
}; // 两个入口分别匹配Bevy主PBR与预通道顶点接口。

const BILLBOARD_MODE_FIXED: u32 = 0; // 固定模式保留作者世界矩阵，主要用于安全默认值。
const BILLBOARD_MODE_CYLINDRICAL: u32 = 1; // 圆柱模式只使用相机方向的世界X/Z分量。
const BILLBOARD_MODE_SPHERICAL: u32 = 2; // 全向模式使用相机到节点的完整三维方向。
const BILLBOARD_AXIS_POSITIVE_X: u32 = 0; // 资产局部正X是正面。
const BILLBOARD_AXIS_NEGATIVE_X: u32 = 1; // 资产局部负X是正面。
const BILLBOARD_AXIS_POSITIVE_Z: u32 = 2; // 资产局部正Z是正面。
const BILLBOARD_AXIS_NEGATIVE_Z: u32 = 3; // 资产局部负Z是正面。

/// 通用动态公告板的MaterialExtension。
///
/// 字段保持私有，调用方只能通过`create_vegetation_mesh_billboard_material`让Rust枚举与
/// WGSL编码同时生成，避免模式和正面轴在CPU/GPU两端手工重复填写。
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct VegetationMeshBillboardExtension {
    #[uniform(100)] // 一个UVec4固定占16字节并满足Uniform对齐，不创建逐实例上传。
    gpu_configuration: UVec4, // x为模式、y为正面轴，z/w保留给兼容扩展。
    billboard: VegetationMeshBillboard, // CPU只读副本供诊断、测试和资产检查，不进入绑定组。
} // 完成GPU公告板扩展资产定义。

impl Default for VegetationMeshBillboardExtension {
    fn default() -> Self {
        Self {
            gpu_configuration: UVec4::ZERO, // 零编码严格对应Fixed与PositiveX安全默认值。
            billboard: VegetationMeshBillboard::Fixed, // 默认扩展不改变作者姿态。
        } // 返回CPU与GPU编码一致的固定模式。
    } // 完成扩展默认值。
} // 完成GPU公告板扩展默认实现。

impl VegetationMeshBillboardExtension {
    /// 返回创建材质时固定的公告板模式。
    pub const fn billboard(&self) -> VegetationMeshBillboard {
        self.billboard // 外部可以核对资产配置，但不能绕过构造入口修改GPU编码。
    } // 完成公告板模式读取。
} // 完成GPU公告板扩展公共行为。

/// 保留StandardMaterial完整PBR语义的通用GPU公告板材质。
pub type VegetationMeshBillboardMaterial =
    ExtendedMaterial<StandardMaterial, VegetationMeshBillboardExtension>; // 同模式同基础材质可由全部实例共享和自动批处理。

impl MaterialExtension for VegetationMeshBillboardExtension {
    /// 主画面与阴影视图都按各自LOD观察位置构造公告板世界矩阵。
    fn vertex_shader() -> ShaderRef {
        VEGETATION_BILLBOARD_VERTEX_SHADER_HANDLE.clone().into() // 返回编译进crate的固定主顶点Shader。
    } // 完成主顶点Shader选择。

    /// 深度、法线、运动向量与延迟预通道使用相同GPU朝向核心。
    fn prepass_vertex_shader() -> ShaderRef {
        VEGETATION_BILLBOARD_PREPASS_SHADER_HANDLE.clone().into() // 返回预通道专用顶点接口。
    } // 完成预通道顶点Shader选择。

    /// 延迟PBR必须与主画面使用相同顶点位置和法线。
    fn deferred_vertex_shader() -> ShaderRef {
        VEGETATION_BILLBOARD_PREPASS_SHADER_HANDLE.clone().into() // Bevy通过延迟Shader定义复用该入口。
    } // 完成延迟顶点Shader选择。
} // 完成GPU公告板PBR扩展契约。

/// 注册crate内置Shader和具体GPU公告板材质管线。
///
/// 本插件需要在Bevy的`DefaultPlugins`之后安装；无窗口ECS fixture只安装
/// `VegetationMeshPlugin`，因此不会被迫创建AssetServer或RenderApp。
#[derive(Clone, Copy, Debug, Default)]
pub struct VegetationMeshBillboardPlugin; // 生产环境组合显式安装一次，基础生命周期仍可独立测试。

impl Plugin for VegetationMeshBillboardPlugin {
    fn build(&self, app: &mut App) {
        super::shaders::register_vegetation_billboard_shaders(app); // 先登记源码，材质专用化时句柄必定可解析。
        app.add_plugins(MaterialPlugin::<VegetationMeshBillboardMaterial>::default()); // 安装主渲染、阴影和预通道资产接线。
    } // 完成GPU公告板材质插件构建。
} // 完成GPU公告板材质插件实现。

/// 从现有基础PBR材质创建一份共享GPU公告板材质。
pub fn create_vegetation_mesh_billboard_material(
    base: StandardMaterial, // 调用方已经完成贴图、双面、AlphaMask与PBR参数校准的副本。
    billboard: VegetationMeshBillboard, // 固定、圆柱或全向模式及显式资产正面轴。
) -> VegetationMeshBillboardMaterial {
    VegetationMeshBillboardMaterial {
        base, // PBR片元阶段继续完整复用调用方现有材质。
        extension: VegetationMeshBillboardExtension {
            gpu_configuration: billboard_gpu_configuration(billboard), // 一次编码供全部共享实例读取。
            billboard, // 保存同一Rust值供诊断和回归测试核对。
        }, // 完成GPU扩展配置。
    } // 返回可加入Assets并绑定到动态公告板部件的具体材质。
} // 完成GPU公告板材质创建。

/// 把公共模式稳定编码为WGSL读取的单个UVec4。
fn billboard_gpu_configuration(billboard: VegetationMeshBillboard) -> UVec4 {
    let (mode, front_axis) = match billboard {
        VegetationMeshBillboard::Fixed => (
            BILLBOARD_MODE_FIXED,      // 固定模式不改变世界矩阵。
            BILLBOARD_AXIS_POSITIVE_X, // 正面轴不会被固定分支读取，使用零编码保持默认稳定。
        ),
        VegetationMeshBillboard::Cylindrical { front_axis } => (
            BILLBOARD_MODE_CYLINDRICAL,            // 只绕世界Y轴朝向当前渲染视图。
            billboard_front_axis_code(front_axis), // 资产正面轴保持显式内容契约。
        ),
        VegetationMeshBillboard::Spherical { front_axis } => (
            BILLBOARD_MODE_SPHERICAL,              // 使用当前渲染视图的完整三维方向。
            billboard_front_axis_code(front_axis), // 资产正面轴保持显式内容契约。
        ),
    }; // 完成公共枚举到GPU数值的穷尽匹配。
    UVec4::new(mode, front_axis, 0, 0) // z/w保留为零，未来扩展不改变当前16字节ABI。
} // 完成公告板GPU配置编码。

/// 把四种有限正面轴编码为WGSL常量。
const fn billboard_front_axis_code(front_axis: VegetationMeshBillboardFrontAxis) -> u32 {
    match front_axis {
        VegetationMeshBillboardFrontAxis::PositiveX => BILLBOARD_AXIS_POSITIVE_X, // 局部正X。
        VegetationMeshBillboardFrontAxis::NegativeX => BILLBOARD_AXIS_NEGATIVE_X, // 局部负X。
        VegetationMeshBillboardFrontAxis::PositiveZ => BILLBOARD_AXIS_POSITIVE_Z, // 局部正Z。
        VegetationMeshBillboardFrontAxis::NegativeZ => BILLBOARD_AXIS_NEGATIVE_Z, // 局部负Z。
    } // 完成正面轴稳定编码。
} // 完成正面轴编码函数。

#[cfg(test)] // 材质编码回归不启动窗口、AssetServer或RenderApp。
mod tests {
    use bevy::prelude::{AlphaMode, StandardMaterial}; // 构造默认和已有AlphaMask基础材质。

    use super::{
        BILLBOARD_AXIS_NEGATIVE_Z, BILLBOARD_MODE_CYLINDRICAL,
        create_vegetation_mesh_billboard_material,
    }; // 直接核对正式构造入口与私有GPU编码。
    use crate::{VegetationMeshBillboard, VegetationMeshBillboardFrontAxis}; // 构造公共动态模式。

    #[test] // Rust模式与WGSL数值必须由同一构造入口保持一致。
    fn cylindrical_material_encodes_mode_and_front_axis_once() {
        let billboard = VegetationMeshBillboard::Cylindrical {
            front_axis: VegetationMeshBillboardFrontAxis::NegativeZ, // 使用Bevy常见局部负Z正面。
        }; // 完成测试动态模式。
        let material = create_vegetation_mesh_billboard_material(
            StandardMaterial::default(), // 默认不透明PBR材质足够验证配置与官方预通道绑定。
            billboard,                   // 同一值同时写入CPU副本与GPU编码。
        ); // 完成测试材质构造。
        assert_eq!(material.extension.billboard(), billboard); // CPU只读配置必须逐值保留。
        assert_eq!(
            material.extension.gpu_configuration.x,
            BILLBOARD_MODE_CYLINDRICAL
        ); // WGSL模式必须选择圆柱分支。
        assert_eq!(
            material.extension.gpu_configuration.y,
            BILLBOARD_AXIS_NEGATIVE_Z
        ); // WGSL正面轴必须选择局部负Z。
        assert_eq!(material.base.alpha_mode, AlphaMode::Opaque); // Bevy 0.19.1无需伪装AlphaMask即可读取扩展绑定。
    } // 完成GPU公告板配置回归。

    #[test] // 真实公告板AlphaMask裁切阈值不能被预通道兼容处理覆盖。
    fn billboard_material_preserves_existing_alpha_mask_cutoff() {
        let material = create_vegetation_mesh_billboard_material(
            StandardMaterial {
                alpha_mode: AlphaMode::Mask(0.45), // 模拟HN类植被叶缘既有裁切阈值。
                ..Default::default()               // 其余PBR参数不影响本测试。
            }, // 完成基础材质构造。
            VegetationMeshBillboard::Spherical {
                front_axis: VegetationMeshBillboardFrontAxis::PositiveZ, // 使用完整三维朝向路径。
            }, // 完成全向模式构造。
        ); // 创建正式扩展材质。
        assert_eq!(material.base.alpha_mode, AlphaMode::Mask(0.45)); // 既有叶缘阈值必须原样保留。
    } // 完成AlphaMask保留回归。
} // 完成GPU公告板材质测试模块。
