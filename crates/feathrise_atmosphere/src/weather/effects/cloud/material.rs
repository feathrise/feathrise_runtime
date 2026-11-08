//! 动态云自定义材质：把世界视线投影参数、云色与周期噪声绑定到透明前向管线。

use bevy::mesh::MeshVertexBufferLayoutRef; // Material 专用化保持 Bevy 网格布局错误通道。
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin}; // 注册独立透明云材质管线。
use bevy::prelude::*; // Asset、Handle、Image、Vec4 与 AlphaMode 等基础类型。
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
}; // 自动建立绑定组并关闭球体背面裁剪。
use bevy::shader::ShaderRef; // 材质返回 crate 内置 WGSL 固定句柄。

use self::shaders::{DYNAMIC_CLOUD_SHADER_HANDLE, register_dynamic_cloud_shader}; // 管线与材质共享唯一 Shader 身份。
use super::profile::CloudCurtainProfile; // CPU 纯参数转换为云色、投影和最终太阳边光的固定 GPU 布局。

mod shaders {
    //! 动态云内置 Shader 的固定资产句柄与注册入口。

    use bevy::asset::{load_internal_asset, uuid_handle}; // Shader 随 crate 编译，不依赖部署目录松散文件。
    use bevy::prelude::{App, Handle, Shader}; // 插件构建阶段向 Shader 资产表登记源码。

    /// 动态云透明片元 Shader 的稳定内部句柄。
    pub(super) const DYNAMIC_CLOUD_SHADER_HANDLE: Handle<Shader> =
        uuid_handle!("a76e931c-a71d-47e5-a718-7bbb98c96ef9"); // 独立 UUID 避免与宿主或其他材质冲突。

    /// 注册由动态云材质引用的唯一片元 Shader。
    pub(super) fn register_dynamic_cloud_shader(app: &mut App) {
        load_internal_asset!(
            app,                         // 向当前主世界 Shader 资产表登记源码。
            DYNAMIC_CLOUD_SHADER_HANDLE, // Material 返回同一固定句柄。
            "shaders/cloud.wesl",        // 源码相对当前模块编译期定位。
            |source, _| Shader::from_wesl(source, "embedded://feathrise_atmosphere/weather/effects/cloud/shaders/cloud.wesl")
        );
    }
}

/// CPU 与 WGSL 共享的固定动态云参数布局。
#[derive(Clone, Copy, Debug, PartialEq, ShaderType)]
pub(super) struct DynamicCloudUniform {
    pub(super) top_color: Vec4, // 受昼夜和天气影响的云顶线性色；Alpha 保留为一。
    pub(super) bottom_color: Vec4, // 云底较暗色提供厚度层次，避免整屏同色贴纸感。
    pub(super) horizon_color: Vec4, // 低仰角平滑靠近大气/雾色，消除载体边缘。
    pub(super) motion_cover: Vec4, // xy 为周期滚动偏移，z 为云量，w 为最大不透明度。
    pub(super) projection: Vec4, // x 为投影缩放，y 为最小高度分母，zw 为地平线淡入区间。
    pub(super) sun_direction: Vec4, // xyz 为向太阳的单位世界方向，w 不参与采样。
    pub(super) sun_color_rim: Vec4, // rgb 为最终太阳色相，w 为天气衰减后的暖边强度。
}

impl DynamicCloudUniform {
    /// 将已经规范化的 CPU 云层参数编码为单个固定 Uniform。
    pub(super) fn from_profile(profile: CloudCurtainProfile, scroll: Vec2) -> Self {
        Self {
            top_color: profile.top_color.extend(1.0), // 云顶只携带线性 RGB，透明度由独立字段控制。
            bottom_color: profile.bottom_color.extend(1.0), // 暗部不通过 Alpha 制造灰膜。
            horizon_color: profile.horizon_color.extend(1.0), // 地平线颜色与 ClearColor 回退保持同源。
            motion_cover: Vec4::new(
                scroll.x,         // 世界风向 X 对应噪声 U 偏移。
                scroll.y,         // 世界风向 Z 对应噪声 V 偏移。
                profile.coverage, // 零至一连续云量用于 Shader 阈值。
                profile.opacity,  // 云体最大不透明度不包含任何固定最低 Alpha。
            ),
            projection: Vec4::new(0.17, 0.085, 0.012, 0.20), // 有界水平射线投影及柔和地平线过渡。
            sun_direction: profile.sunlight.direction.extend(0.0), // 夜间无灯时为零向量，不需分支材质变体。
            sun_color_rim: profile.sunlight.color.extend(profile.sunlight.rim), // 日间只给薄边少量同帧日光。
        }
    }
}

/// 世界视线动态云材质；全部天气只共享一个实例。
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub(super) struct DynamicCloudMaterial {
    #[uniform(0)]
    pub(super) params: DynamicCloudUniform, // 七个 vec4 构成稳定且很小的逐帧 GPU 数据。
    #[texture(1)]
    #[sampler(2)]
    pub(super) noise_texture: Handle<Image>, // 四通道周期噪声由全部采样层复用。
}

impl DynamicCloudMaterial {
    /// 使用当前天气参数与共享噪声构造首帧材质。
    pub(super) fn new(
        noise_texture: Handle<Image>, // 一次性程序纹理强句柄。
        profile: CloudCurtainProfile, // 当前连续天气外观。
        scroll: Vec2,                 // 首帧或重建时的周期风偏移。
    ) -> Self {
        Self {
            params: DynamicCloudUniform::from_profile(profile, scroll), // 创建时避免闪现默认灰色。
            noise_texture, // 保持程序纹理存活并供渲染世界提取。
        }
    }

    /// 仅在实际参数变化时替换 Uniform，暂停帧不会制造无意义资产变更。
    pub(super) fn write_profile(&mut self, profile: CloudCurtainProfile, scroll: Vec2) {
        let desired = DynamicCloudUniform::from_profile(profile, scroll); // 在栈上构造完整新快照。
        if self.params != desired {
            self.params = desired; // 原子替换全部颜色、云量与偏移，避免上一天气字段残留。
        }
    }
}

impl Material for DynamicCloudMaterial {
    fn fragment_shader() -> ShaderRef {
        DYNAMIC_CLOUD_SHADER_HANDLE.clone().into() // 使用编译进 atmosphere crate 的固定 Shader。
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend // 云边与天空按连续 Alpha 混合，不写不透明灰色底层。
    }

    fn enable_prepass() -> bool {
        false // 远景透明云不写深度、法线或运动向量预通道。
    }

    fn enable_shadows() -> bool {
        false // 天气对直射光的压暗由统一环境修正完成，云球不进入阴影图。
    }

    fn specialize(
        _pipeline: &MaterialPipeline, // 不替换 Bevy 默认网格顶点阶段。
        descriptor: &mut RenderPipelineDescriptor, // 只修改当前动态云管线的图元状态。
        _layout: &MeshVertexBufferLayoutRef, // 二十面球使用标准位置和法线布局。
        _key: MaterialPipelineKey<Self>, // 所有天气共享同一管线，不增加专用化变体。
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None; // 相机位于球体内部，必须绘制内侧且兼容不同绕向。
        Ok(()) // 其余透明排序、深度测试和 HDR 目标沿用 Bevy Material 管线。
    }
}

/// 在具有真实资产系统的宿主中安装内置 Shader 与动态云材质管线。
pub(crate) fn install_dynamic_cloud_material(app: &mut App) {
    if !app.world().contains_resource::<AssetServer>()
        || !app.world().contains_resource::<Assets<Shader>>()
    {
        return; // 纯 ECS 单元测试没有 AssetPlugin 或 RenderApp，直接使用 Assets<DynamicCloudMaterial> 即可。
    }
    register_dynamic_cloud_shader(app); // MaterialPlugin 专用化前保证固定句柄已经可解析。
    app.add_plugins(MaterialPlugin::<DynamicCloudMaterial>::default()); // 注册提取、绑定组与透明主通道。
}

#[cfg(test)]
mod tests {
    use super::*; // 直接锁定材质透明、预通道与 Uniform 布局契约。

    #[test]
    fn dynamic_cloud_material_is_transparent_and_skips_prepasses() {
        let profile = CloudCurtainProfile::test_visible(); // 使用内部稳定测试外观构造材质。
        let material = DynamicCloudMaterial::new(Handle::default(), profile, Vec2::ZERO);
        assert_eq!(material.alpha_mode(), AlphaMode::Blend); // 云边必须连续混合。
        assert!(!DynamicCloudMaterial::enable_prepass()); // 云不得污染深度与运动向量。
        assert!(!DynamicCloudMaterial::enable_shadows()); // 云不得生成第二套阴影。
        assert_eq!(std::mem::size_of::<DynamicCloudUniform>(), 112); // Rust 与 WGSL 固定为七个 vec4。
    }

    #[test]
    fn cloud_shader_reconstructs_world_ray_and_bounds_horizon_projection() {
        let source = include_str!("shaders/cloud.wesl"); // 检查与正式内置句柄相同的源码。
        assert!(source.contains("in.world_position.xyz - view.world_from_view[3].xyz")); // 视线必须来自世界位置与相机位置。
        assert!(source.contains("direction.xz / max(direction.y, params.projection.y)")); // 地平线投影分母必须有界。
        assert!(!source.contains("in.uv")); // 禁止回退到会产生极点和经线的球面 UV。
        assert_eq!(source.matches("textureSample(").count(), 2); // 固定两次噪声采样控制片元成本。
        assert!(
            source.find("let noise_b = textureSample(").unwrap()
                < source.find("let warp = ").unwrap()
        ); // 第二次采样的低频结果先扭曲主云轮廓。
        assert!(source.contains("let shaped_alpha = cloud_body")); // 真实云洞保持完全透明，不恢复最低灰膜。
        assert!(!source.contains("max(cloud_body, horizon_haze)")); // 暴雨地平线不能额外铺灰色透明顶棚。
        assert!(source.contains("dot(direction, params.sun_direction.xyz)")); // 暖边只在向阳薄云轮廓出现。
    }

    #[test] // 内置Shader保留Bevy导入后，其完整片元正文仍须通过锁定WGPU版本的WGSL校验。
    fn cloud_fragment_passes_naga_validation() {
        let source = include_str!("shaders/cloud.wesl") // 校验实际注册的同一份WGSL源码。
            .replace("import bevy_pbr::render::forward_io::VertexOutput;", "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) world_position: vec4<f32>, };") // 仅为离屏测试补足Bevy提供的片元输入。
            .replace("import bevy_pbr::render::mesh_view_bindings::view;", "struct View { world_from_view: mat4x4<f32>, }; @group(0) @binding(0) var<uniform> view: View;") // 保留正式相机世界矩阵访问。
            .replace("constants::MATERIAL_BIND_GROUP", "2"); // 使用Bevy扩展材质的合法绑定组号。
        let module =
            wgpu::naga::front::wgsl::parse_str(&source).expect("动态云WGSL必须通过语法解析"); // 不允许运行时才发现材质坏Shader。
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("动态云WGSL必须通过语义验证"); // 覆盖两次采样、导数和日光参数布局。
    }
}
