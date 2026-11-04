//! 树木与灌木共享的项目自有 GPU 风扩展材质。
//! 【功能设计】扩展只替换顶点阶段，PBR、AlphaMask、双面、阴影片元与延迟写入继续由 `StandardMaterial` 负责。

use bevy::asset::Asset; // 自有扩展需要作为ExtendedMaterial的一部分进入Assets和RenderAssets。
use bevy::camera::primitives::Aabb; // 完整树高参考和保守剔除边界沿用Bevy原生局部包围盒。
use bevy::mesh::MeshVertexBufferLayoutRef; // 材质专用化保持Bevy网格布局错误通道不变。
use bevy::pbr::{
    ExtendedMaterial,          // 基础PBR与自有顶点风扩展组合成具体资产类型。
    MaterialExtension,         // 提供主渲染、预通道、延迟顶点Shader和专用化接口。
    MaterialExtensionKey,      // 读取材质创建时固定的详细叶簇分支键。
    MaterialExtensionPipeline, // 专用化签名保留后续Bevy迁移的高层边界。
    MaterialPlugin,            // 为具体扩展材质注册提取、准备和各渲染管线。
    StandardMaterial,          // 原GLB的PBR纹理、AlphaMask和双面配置继续完整复用。
}; // 完成Bevy PBR扩展材质类型导入。
use bevy::prelude::*; // 引入App、Handle、Image与Shader等公共类型。
use bevy::reflect::TypePath; // 自有Asset使用稳定Rust类型路径标识。
use bevy::render::render_resource::{
    AsBindGroup,                  // 把固定128字节Uniform及共享噪声纹理映射到100/101/102。
    RenderPipelineDescriptor,     // 专用化阶段只增加详细叶簇顶点Shader定义。
    SpecializedMeshPipelineError, // 保持MaterialExtension标准错误返回。
}; // 完成渲染资源类型导入。
use bevy::shader::ShaderRef; // 固定内部Shader句柄通过MaterialExtension返回。

use super::helpers::{
    combine_tree_part_aabbs as combine_part_aabbs_internal, // 合并同档树干与树冠完整树高。
    expanded_tree_wind_culling_aabb as expanded_culling_aabb_internal, // 覆盖最大GPU风位移。
    tree_wind_gpu_frame,      // 每帧只更新当前三个vec4并推进上一帧历史。
    tree_wind_parameters,     // 把植被同源风场安全映射到部件参数。
    tree_wind_profile,        // 材质创建时固定详细或宏观专用化分支。
    tree_wind_reference_aabb, // 地表以下根须使用零高度权重。
    tree_wind_uniform,        // 创建固定八vec4的当前、历史与边界Uniform。
}; // 完成Mesh风纯函数导入。
use super::resources::TreeWindNoiseTexture; // Startup唯一噪声资源替代第三方类型身份。
use super::shaders::{
    TREE_WIND_PREPASS_SHADER_HANDLE, // 深度、阴影相关预通道和延迟共用的顶点入口。
    TREE_WIND_VERTEX_SHADER_HANDLE,  // 主渲染与方向光阴影使用的顶点入口。
}; // 完成crate内置Shader固定句柄导入。
use super::types::{TreeWindMaterialKey, TreeWindUniform}; // 固定GPU ABI与仅含一个布尔值的专用化键。
use super::{MeshWindLodLevel, MeshWindPart}; // 内容域只传入共享质量档与部件语义。

/// 项目自有树木风材质扩展。
/// 该类型公开仅因为它构成公开 `TreeWindMaterial` 的资产身份，地图层无需直接操作其字段。
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, Default)] // 非bindless派生路径适合正式场景十份共享材质。
#[bind_group_data(TreeWindMaterialKey)] // 只把详细叶簇布尔值带入管线缓存键，不上传额外Uniform。
pub struct TreeWindExtension {
    #[uniform(100)] // 当前帧、上一帧和树高边界严格占用固定128字节。
    uniform: TreeWindUniform, // WGSL binding 100按八个vec4直接读取。
    #[texture(101)] // 四通道周期RGBA8噪声只绑定一次。
    #[sampler(102)] // 重复线性采样器与纹理句柄成对准备。
    noise_texture: Handle<Image>, // 全部树木与灌木材质共享Startup创建的同一Image。
    detailed: bool, // 高中档叶簇为真；树干和低档叶簇只编译宏观弯曲。
} // 完成自有材质扩展定义。

/// 树木与灌木主世界共用的具体 GPU 风材质。
pub type TreeWindMaterial = ExtendedMaterial<StandardMaterial, TreeWindExtension>; // 保留原公共名称和地图层签名。

impl From<&TreeWindExtension> for TreeWindMaterialKey {
    // AsBindGroup在材质准备时提取稳定且廉价的管线缓存键。
    fn from(extension: &TreeWindExtension) -> Self {
        Self {
            detailed: extension.detailed, // 只有该值会改变顶点Shader编译分支。
        } // 返回不包含逐帧风参数的专用化键。
    } // 完成扩展到材质键转换。
} // 完成自有AsBindGroup数据键接线。

impl MaterialExtension for TreeWindExtension {
    // 主渲染和方向光阴影使用同一当前帧位移入口。
    fn vertex_shader() -> ShaderRef {
        TREE_WIND_VERTEX_SHADER_HANDLE.clone().into() // 返回编译进crate的固定主顶点Shader。
    } // 完成主顶点Shader选择。

    // 深度、法线与运动向量预通道使用能够重建上一帧顶点的位置入口。
    fn prepass_vertex_shader() -> ShaderRef {
        TREE_WIND_PREPASS_SHADER_HANDLE.clone().into() // 返回编译进crate的固定预通道Shader。
    } // 完成预通道顶点Shader选择。

    // 延迟PBR预通道必须采用与主渲染完全相同的当前顶点姿态。
    fn deferred_vertex_shader() -> ShaderRef {
        TREE_WIND_PREPASS_SHADER_HANDLE.clone().into() // 复用同一预通道入口并由Bevy设置延迟定义。
    } // 完成延迟顶点Shader选择。

    // 只在创建时固定的高中档叶簇材质上启用三类局部细摆。
    fn specialize(
        _pipeline: &MaterialExtensionPipeline, // 当前无需改写Bevy网格顶点缓冲布局。
        descriptor: &mut RenderPipelineDescriptor, // 当前具体主、预通道或延迟管线描述。
        _layout: &MeshVertexBufferLayoutRef,   // StandardMaterial已完成UV、法线和切线布局专用化。
        key: MaterialExtensionKey<Self>,       // 读取扩展自身的详细叶簇布尔键。
    ) -> Result<(), SpecializedMeshPipelineError> {
        if key.bind_group_data.detailed {
            // 统一顶点库据此编译微观噪声、S曲线与上下起伏。
            descriptor
                .vertex
                .shader_defs
                .push("TREE_WIND_DETAILED".into()); // 树干和远景不承担该分支成本。
        } // 完成详细叶簇Shader定义选择。
        Ok(()) // 不修改管线其余PBR、AlphaMask、剔除和阴影配置。
    } // 完成自有材质管线专用化。
} // 完成MaterialExtension实现。

/// 注册程序噪声、内置Shader与具体扩展材质管线。
pub(super) fn install_mesh_wind_material_pipeline(app: &mut App) {
    super::shaders::register_mesh_wind_shaders(app); // 先登记Shader，MaterialPlugin专用化时句柄一定可解析。
    app.add_plugins(MaterialPlugin::<TreeWindMaterial>::default()); // 安装主渲染、阴影、预通道和延迟资产接线。
    app.add_systems(Startup, super::noise::setup_tree_wind_noise_texture); // 只在应用Startup生成一次固定一MiB噪声。
} // 完成共享Mesh GPU风公共管线注册。

/// 合并同一 LOD 树干与树冠边界，供两个材质共享完整树高。
pub fn combine_tree_part_aabbs(trunk: Aabb, canopy: Aabb) -> Aabb {
    combine_part_aabbs_internal(trunk, canopy) // 转交纯函数执行逐轴最小和最大边界合并。
} // 完成公共风动Mesh部件边界合并。

/// 扩展局部剔除边界以覆盖最大 GPU 位移。
pub fn expanded_tree_wind_culling_aabb(source: Aabb) -> Aabb {
    expanded_culling_aabb_internal(source) // 转交纯函数应用固定最坏位移预算。
} // 完成公共GPU风剔除边界构造。

/// 从现有 GLB 基础材质创建单个 LOD 部件的 GPU 风材质。
pub fn create_tree_wind_material(
    base: StandardMaterial, // 已完成树干单面或树冠双面AlphaMask校准的现有PBR材质副本。
    combined_tree_aabb: Aabb, // 当前LOD完整树干加树冠边界。
    noise_texture: &TreeWindNoiseTexture, // Startup生成的一份公共项目自有风噪声纹理。
    level: MeshWindLodLevel, // 高、中、低决定叶簇细摆等级。
    part: MeshWindPart,     // 茎干只弯曲，叶簇可叠加局部高频位移。
) -> TreeWindMaterial {
    let profile = tree_wind_profile(level, part); // 固定着色分支在材质创建时选择，逐帧更新不会重建管线。
    let initial = tree_wind_parameters(level, part, Vec2::X, 0.0, 1.0); // 首帧当前和历史都从严格静止姿态开始。
    let reference_aabb = tree_wind_reference_aabb(combined_tree_aabb); // 树干与树冠共享完整树高并固定地表根部。
    TreeWindMaterial {
        base, // 完整复用现有颜色、纹理、粗糙度、AlphaMask、双面与PBR模式。
        extension: TreeWindExtension {
            uniform: tree_wind_uniform(initial, initial, reference_aabb), // 首次运动向量不会产生历史脉冲。
            noise_texture: noise_texture.0.clone(), // 十份正式材质共享同一个程序Image句柄。
            detailed: !profile.low_quality,         // 只有高中档叶簇启用三类详细位移。
        }, // 完成固定绑定与专用化数据构造。
    } // 返回可加入Assets并供同档全部固定实例共享的材质。
} // 完成树木GPU风材质创建。

/// 用统一世界风场推进材质的当前帧与上一帧参数。
pub fn update_tree_wind_material(
    material: &mut TreeWindMaterial, // 当前LOD部件全部固定实例共享的一份扩展材质资产。
    level: MeshWindLodLevel,         // 维持创建时相同的LOD细节等级。
    part: MeshWindPart,              // 维持茎干或叶簇的固定细节策略。
    direction_xz: Vec2,              // 与GPU草地同源的世界X/Z风向。
    normalized_strength: f32,        // F1草尖强度归一化值。
    animation_speed: f32,            // F1草地风动画速度。
) {
    material.extension.uniform.previous = material.extension.uniform.current; // 先保存上一帧完整GPU值供运动向量重建。
    let current = tree_wind_parameters(
        level,               // 继续使用创建时相同LOD等级。
        part,                // 继续使用创建时相同部件身份。
        direction_xz,        // 写入安全归一化后的世界风向。
        normalized_strength, // 写入限制到零至一的统一强度。
        animation_speed,     // 写入限制到公共范围的统一速度。
    ); // 完成当前帧可读CPU参数构造。
    material.extension.uniform.current = tree_wind_gpu_frame(current); // 只改写当前三个vec4，树高边界保持不变。
} // 完成单份共享Mesh风材质更新。

/// 项目天气桥使用连续相位；独立Mesh风示例继续使用原有无偏移接口。
pub fn update_tree_wind_material_with_phase(
    material: &mut TreeWindMaterial, // 同档所有植被共享一份材质而不遍历逐株实体。
    level: MeshWindLodLevel,         // 固定LOD质量档和风弯曲细节。
    part: MeshWindPart,              // 树干、叶簇或静止公告板部件。
    direction_xz: Vec2,              // 来自WeatherOutput的世界风向。
    normalized_strength: f32,        // 由草尖实际摆幅换算的通用Mesh强度。
    animation_speed: f32,            // 天气转换后的无量纲速度，可含内容频率倍率。
    phase_offset: f32,               // 同一速度倍率下，草和Mesh共享的连续时间相位补偿。
) {
    update_tree_wind_material(
        material,
        level,
        part,
        direction_xz,
        normalized_strength,
        animation_speed,
    ); // 保留现有上一帧/当前帧历史推进。
    material.extension.uniform.current.secondary.w = if phase_offset.is_finite() {
        phase_offset
    } else {
        0.0
    }; // 占用原保留槽，不扩大Mesh材质128字节ABI。
} // 完成含连续天气相位的共享材质更新。

#[cfg(test)] // 材质布局和历史回归不启动窗口或RenderApp。
mod tests {
    use std::mem::size_of; // 固定GPU ABI通过Rust物理字节数直接锁定。

    use bevy::camera::primitives::Aabb; // 构造十米完整树高参考边界。
    use bevy::prelude::*; // 构造默认PBR材质、弱纹理句柄和测试风向。

    use super::{
        create_tree_wind_material, update_tree_wind_material, update_tree_wind_material_with_phase,
    }; // 同时验证旧接口与天气连续相位入口。
    use crate::mesh_wind::types::{TreeWindGpuFrame, TreeWindUniform}; // 验证三个及八个vec4固定布局。
    use crate::mesh_wind::{MeshWindLodLevel, MeshWindPart, TreeWindNoiseTexture}; // 选择详细叶簇并构造自有资源。

    #[test] // Rust侧布局必须与WGSL两个三vec4帧加两个边界vec4严格一致。
    fn tree_wind_uniform_layout_stays_at_eight_vec4s() {
        assert_eq!(size_of::<TreeWindGpuFrame>(), 48); // 单帧固定三个vec4。
        assert_eq!(size_of::<TreeWindUniform>(), 128); // 完整Uniform固定八个vec4。
    } // 完成GPU ABI字节布局回归。

    #[test] // 天气桥的相位补偿必须与上一帧速度、幅度一并进入运动向量历史。
    fn weather_phase_uses_reserved_slot_without_losing_history() {
        let mut material = create_tree_wind_material(
            StandardMaterial::default(),                    // 不依赖GLB源材质。
            Aabb::from_min_max(Vec3::ZERO, Vec3::Y * 10.0), // 十米树高固定参考边界。
            &TreeWindNoiseTexture(Handle::default()),       // 测试只需有效句柄类型。
            MeshWindLodLevel::High,                         // 同时覆盖叶簇细摆分支。
            MeshWindPart::Foliage,                          // 使用可读微观风幅的树冠部件。
        ); // 构造完整当前与历史帧GPU uniform。
        update_tree_wind_material_with_phase(
            &mut material,
            MeshWindLodLevel::High,
            MeshWindPart::Foliage,
            Vec2::X,
            0.5,
            1.1,
            -2.0,
        ); // 第一帧已有天气相位偏移。
        update_tree_wind_material_with_phase(
            &mut material,
            MeshWindLodLevel::High,
            MeshWindPart::Foliage,
            Vec2::Y,
            0.8,
            1.5,
            -4.0,
        ); // 第二帧切风向与速度。
        assert_eq!(material.extension.uniform.previous.secondary.w, -2.0); // 旧帧运动向量不能复用新相位。
        assert_eq!(material.extension.uniform.current.secondary.w, -4.0); // 新帧主渲染与阴影读取最新偏移。
    } // 完成固定128字节Mesh材质相位历史验证。

    #[test] // 更新当前帧前必须保存旧GPU参数供运动向量重建。
    fn wind_material_keeps_previous_frame_parameters() {
        let mut material = create_tree_wind_material(
            StandardMaterial::default(),                    // 测试不依赖真实GLB纹理。
            Aabb::from_min_max(Vec3::ZERO, Vec3::Y * 10.0), // 使用十米完整树高。
            &TreeWindNoiseTexture(Handle::default()),       // 弱Image句柄足以构造纯CPU材质资产。
            MeshWindLodLevel::High,                         // 高档启用完整叶簇细摆分支。
            MeshWindPart::Foliage,                          // 当前测试覆盖树冠材质。
        ); // 完成测试材质初始化。
        update_tree_wind_material(
            &mut material,          // 更新同一共享材质。
            MeshWindLodLevel::High, // 保持高档身份。
            MeshWindPart::Foliage,  // 保持树冠身份。
            Vec2::X,                // 第一帧使用世界X正向。
            0.4,                    // 第一帧使用四成风力。
            1.0,                    // 第一帧使用单位动画速度。
        ); // 完成第一帧参数推进。
        let first_current = material.extension.uniform.current; // 保存第一帧GPU值供第二帧历史比较。
        update_tree_wind_material(
            &mut material,          // 继续更新同一共享材质。
            MeshWindLodLevel::High, // 保持高档身份。
            MeshWindPart::Foliage,  // 保持树冠身份。
            Vec2::Y,                // 第二帧切换世界Z正向。
            0.8,                    // 第二帧提高到八成风力。
            1.5,                    // 第二帧提高动画速度。
        ); // 完成第二帧参数推进。
        assert_eq!(material.extension.uniform.previous, first_current); // 历史必须精确保留第一帧全部三个vec4。
        assert_eq!(
            material
                .extension
                .uniform
                .current
                .direction_strength_noise
                .xy(),
            Vec2::Y
        ); // 当前方向必须接收第二帧安全风向。
        assert!(
            material
                .extension
                .uniform
                .current
                .direction_strength_noise
                .z
                > first_current.direction_strength_noise.z
        ); // 更高F1强度必须产生更大宏观摆幅。
        assert!(material.extension.detailed); // 高档叶簇必须选择详细顶点专用化分支。
    } // 完成材质帧历史与详细分支回归。

    #[test] // Bevy 0.19.1原生为自定义预通道顶点Shader保留材质绑定，无需伪装AlphaMask。
    fn opaque_wind_material_preserves_opaque_phase() {
        let material = create_tree_wind_material(
            StandardMaterial::default(), // 默认完全不透明树干应继续进入Opaque3d阶段。
            Aabb::from_min_max(Vec3::ZERO, Vec3::Y * 10.0),
            &TreeWindNoiseTexture(Handle::default()),
            MeshWindLodLevel::High,
            MeshWindPart::Stem,
        );
        assert_eq!(material.base.alpha_mode, AlphaMode::Opaque); // 0.19.1原生材质绑定路径不再需要零阈值伪装。
    }

    #[test] // 真实叶簇裁切阈值不能被兼容处理覆盖。
    fn wind_material_preserves_existing_alpha_mask_cutoff() {
        let material = create_tree_wind_material(
            StandardMaterial {
                alpha_mode: AlphaMode::Mask(0.45),
                ..default()
            },
            Aabb::from_min_max(Vec3::ZERO, Vec3::Y * 10.0),
            &TreeWindNoiseTexture(Handle::default()),
            MeshWindLodLevel::High,
            MeshWindPart::Foliage,
        );
        assert_eq!(material.base.alpha_mode, AlphaMode::Mask(0.45)); // 叶片边缘仍使用既有0.45阈值。
    }
} // 结束树木GPU风材质测试模块。
