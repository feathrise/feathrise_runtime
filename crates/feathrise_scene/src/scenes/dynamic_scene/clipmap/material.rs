//! Bevy 0.20 Geometry Clipmap 的 GPU 顶点高度页与现有地表材质桥。
//!
//! 主视图、Deferred、深度/法线/运动预通道和阴影均读取同一稀疏页表与场景
//! 边界。正式新图以外层 ExtendedMaterial 包装现有 TerrainSurfaceMaterial，
//! 120..123 只承载高度位移，地表权重、天气和 PBR 片元继续由内层负责；独立
//! 地表PBR尚未就绪时先以 StandardMaterial 外包装同一高度扩展，保证纹理或
//! 组合管线失败不会把地形降回平面。页表零入口按 Y=0 显示，作者与运行时必须
//! 保证相邻驻留页共享边一致。

use bevy::asset::{AssetId, RenderAssetUsages, load_internal_asset, uuid_handle}; // 内置 WGSL、一乘一回退图与材质租约身份。
use bevy::image::ImageSampler; // 数值高度页需要线性过滤。
use bevy::mesh::MeshVertexBufferLayoutRef; // 外层扩展为地表片元注入 Clipmap 专用 shader define。
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
    MaterialPlugin,
}; // 保留标准 PBR 管线与多 pass 覆写。
use bevy::prelude::{
    AlphaMode, App, Asset, Assets, FromWorld, Handle, Image, Plugin, Resource, Shader,
    StandardMaterial, TypePath, UVec2, Vec4, World, Mat4,
}; // 材质资产、插件和世界矩形参数。
#[cfg(any(feature = "test_panel", feature = "author_view"))] // 发布构建不需要多边形线模式材质。
use bevy::render::render_resource::PolygonMode; // 直接覆盖真实三角索引，不生成边线Mesh。
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
    TextureDimension, TextureFormat,
}; // 固定 GPU 绑定布局。
use bevy::shader::ShaderRef; // Bevy 多 pass 使用独立内置 Shader 句柄。
use feathrise_environment::terrain_surface::TerrainSurfaceMaterial; // 新图仍复用现有地表预设、雨湿与 PBR 片元。

use super::ClipmapBounds; // 与静态网格使用同一世界 X/Z 范围类型。

pub(crate) const CLIPMAP_TILE_CELLS: u32 = 256; // 一页覆盖 256×256 作者单元、257×257 个高度点。
const MAX_LOGICAL_CELLS: u32 = 65_536; // 8192m/0.125m 的单轴最大细格数。

const HEIGHT_FUNCTIONS_SHADER: Handle<Shader> =
    uuid_handle!("7931b3c4-7851-4c15-9f6f-f20102d0ed00"); // 顶点与片元共用边界/高度采样函数。
const MAIN_VERTEX_SHADER: Handle<Shader> = uuid_handle!("7931b3c4-7851-4c15-9f6f-f20102d0ed01"); // Forward 主顶点。
const PREPASS_VERTEX_SHADER: Handle<Shader> = uuid_handle!("7931b3c4-7851-4c15-9f6f-f20102d0ed02"); // 阴影及深度/法线/运动/Deferred 顶点。
const MAIN_FRAGMENT_SHADER: Handle<Shader> = uuid_handle!("7931b3c4-7851-4c15-9f6f-f20102d0ed03"); // Forward 与 Deferred PBR 片元。
const PREPASS_FRAGMENT_SHADER: Handle<Shader> =
    uuid_handle!("7931b3c4-7851-4c15-9f6f-f20102d0ed04"); // 阴影、深度和运动片元裁切。

/// 已完整上传的 GPU 高度层数组和整数页表；页内解码范围存在页表 texel。
#[derive(Clone, Debug)]
pub(crate) struct ClipmapHeightAtlas {
    pub heights: Handle<Image>, // R16_UNORM 2D array；每层固定 257×257 高度点。
    pub page_table: Handle<Image>, // RGBA32_UINT；每个层级页占一个紧凑 texel。
    pub max_mip: u32,           // 页表包含的最高持久父页层级，当前上限为八。
}

/// 坏页和非法格数必须在改写正式材质资产之前拒绝。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClipmapHeightPageError {
    InvalidBounds,      // 有效页必须有有限非零世界面积。
    InvalidGrid,        // 逻辑格数必须位于受测上限内。
    InvalidHeightRange, // 高度解码不得包含 NaN、负跨度或无穷。
    InvalidMip,         // 持久父页层级只能位于格式支持的零至八级。
    InvalidLayerIndex,  // layer+1 必须保留零作缺页哨兵。
}

/// 所有渲染通道共用源图坐标与实例变换；移动实例不改写高度页。
#[derive(Clone, Copy, Debug, PartialEq, ShaderType)]
pub(crate) struct ClipmapHeightUniform {
    pub(crate) stitches: feathrise_environment::vegetation::blade::GpuBladeClipmapStitches,
    pub(crate) scene_bounds: Vec4, // xy 为场景最小 X/Z，zw 为最大 X/Z。
    pub(crate) grid: Vec4,       // xy 为两轴实际作者格距，zw 为逻辑单元数。
    pub(crate) options: Vec4,    // x 最高 mip，y 为整套 GPU 页资产是否已完整采用。
    pub(crate) reserved: Vec4,   // 光追代理：stride、两轴固定单元数、启用门；不混写相机/页身份。
    pub(crate) source_to_world: Mat4,
    pub(crate) world_to_source: Mat4,
}

/// 三十二个矩形与计数共528字节，所有光栅 pass 使用同一固定预算。
#[derive(Clone, Copy, Debug, PartialEq, ShaderType)]
pub(crate) struct ClipmapHoleUniform {
    pub(crate) rects: [Vec4; 32],
    pub(crate) info: Vec4,
}

impl ClipmapHoleUniform {
    pub(crate) fn from_holes(holes: &[feathrise_scene_content::SceneTerrainCutout]) -> Self {
        let mut result = Self {
            rects: [Vec4::ZERO; 32],
            info: Vec4::ZERO,
        };
        for (to, hole) in result.rects.iter_mut().zip(holes) {
            *to = Vec4::new(hole.min[0], hole.min[1], hole.max[0], hole.max[1]);
        }
        result.info.x = holes.len().min(32) as f32;
        result
    }
}

/// 固定世界代理的两条坐标轴；洞口边界插入粗网格后仍能恢复精确三角编号。
/// 每轴最多513点，合计4144字节，远低于16 KiB统一缓冲下限。
#[derive(Clone, Copy, Debug, PartialEq, ShaderType)]
pub(crate) struct ClipmapProxyAxes {
    pub(crate) x: [Vec4; 129],
    pub(crate) z: [Vec4; 129],
    pub(crate) info: Vec4,
}

impl Default for ClipmapProxyAxes {
    fn default() -> Self {
        Self {
            x: [Vec4::ZERO; 129],
            z: [Vec4::ZERO; 129],
            info: Vec4::ZERO,
        }
    }
}

impl ClipmapProxyAxes {
    pub(crate) fn new(x: &[f32], z: &[f32]) -> Self {
        let mut result = Self::default();
        for (i, value) in x.iter().enumerate() {
            result.x[i / 4][i % 4] = *value;
        }
        for (i, value) in z.iter().enumerate() {
            result.z[i / 4][i % 4] = *value;
        }
        result.info = Vec4::new(1.0, x.len() as f32, z.len() as f32, 0.0);
        result
    }
}

/// 最小原型的高度页 PBR 扩展，绑定号避开现有地表扩展的 100..119。
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct ClipmapHeightExtension {
    #[uniform(120)]
    pub(crate) params: ClipmapHeightUniform, // 所有 pass 共用场景原点、格距和页驻留门。
    #[texture(121, dimension = "2d_array")]
    #[sampler(122)]
    pub(crate) heights: Handle<Image>, // R16_UNORM 高度数组；格点高度随驻留层更新。
    #[texture(123, sample_type = "u_int")]
    pub(crate) page_table: Handle<Image>, // RGBA32_UINT 页表用 textureLoad，零层编码表示缺页。
    #[uniform(124)]
    pub(crate) holes: ClipmapHoleUniform,
    #[uniform(125)]
    pub(crate) proxy_axes: ClipmapProxyAxes,
}

impl ClipmapHeightExtension {
    pub(in crate::scenes::dynamic_scene) fn set_source_frame(&mut self, frame: Mat4) {
        self.params.source_to_world = frame;
        self.params.world_to_source = frame.inverse();
    }
    pub(super) fn same_height_binding(&self, other: &Self) -> bool {
        // 覆盖层只在正式高度资产或参数变化时更新绑定。
        self.params == other.params
            && self.holes == other.holes
            && self.proxy_axes == other.proxy_axes
            && self.heights == other.heights
            && self.page_table == other.page_table // 不读取或比较页像素。
    } // 稳定帧无需触发材质热更。
    pub(crate) fn blade_textures(&self) -> (Handle<Image>, Handle<Image>) {
        // 空白图也复用地表回退图片。
        (self.heights.clone(), self.page_table.clone()) // 不创建第二份图集。
    } // 同代强句柄。

    /// 空白图仍只分配小页表；最细格距由实际场景边长和逻辑单元数得出。
    pub(crate) fn new_flat(
        scene_bounds: ClipmapBounds,        // 清单权威世界 X/Z 边界。
        grid_cells: UVec2,                // 各轴 ceil(场景边长/作者请求格距) 的实际单元数。
        fallback: &ClipmapFlatHeightPage, // 插件注册的两个合法回退 GPU 图片。
    ) -> Result<Self, ClipmapHeightPageError> {
        ClipmapBounds::new(scene_bounds.min, scene_bounds.max)
            .map_err(|_| ClipmapHeightPageError::InvalidBounds)?; // 防止原始字段绕过验证。
        if grid_cells.x == 0
            || grid_cells.y == 0
            || grid_cells.x > MAX_LOGICAL_CELLS
            || grid_cells.y > MAX_LOGICAL_CELLS
        {
            return Err(ClipmapHeightPageError::InvalidGrid); // 防止零格除法与超规格页表。
        }
        let world_size = scene_bounds.max - scene_bounds.min; // 两轴均使用同一世界米制权威尺寸。
        let cell_size = world_size / grid_cells.as_vec2(); // 实际格距不大于作者请求格距。
        Ok(Self {
            params: ClipmapHeightUniform {
                stitches: Default::default(),
                scene_bounds: bounds_vec4(scene_bounds), // 场景外片元在所有 pass 一致裁切。
                grid: Vec4::new(
                    cell_size.x,
                    cell_size.y,
                    grid_cells.x as f32,
                    grid_cells.y as f32,
                ), // 世界 X/Z 到全局作者格坐标。
                options: Vec4::ZERO,                 // y=0 使所有高度查询直接返回 Y=0。
                reserved: Vec4::ZERO,                // 避免未初始化 GPU padding。
                source_to_world: Mat4::IDENTITY,
                world_to_source: Mat4::IDENTITY,
            },
            heights: fallback.heights.clone(), // 2D array 绑定不可留空。
            page_table: fallback.page_table.clone(), // 整数页表绑定不可留空。
            holes: ClipmapHoleUniform::from_holes(&[]),
            proxy_axes: ClipmapProxyAxes::default(),
        })
    }

    /// 高度数组、整数页表与修订必须完整匹配后才一起采用。
    pub(crate) fn set_atlas(
        &mut self,
        atlas: ClipmapHeightAtlas,
    ) -> Result<(), ClipmapHeightPageError> {
        if atlas.max_mip > 8 {
            return Err(ClipmapHeightPageError::InvalidMip); // Shader 紧凑页表循环只接受格式定义的零至八级。
        }
        self.params.options = Vec4::new(atlas.max_mip as f32, 1.0, 0.0, 0.0); // y=1 打开整套页表读取。
        self.heights = atlas.heights; // 与页表同一次材质资产更新。
        self.page_table = atlas.page_table; // 尚未驻留的 tile 仍由零入口返回 Y=0。
        Ok(()) // 上层仍需以场景代次/作者修订/票据检查原子采用。
    }

    /// 切换场景或整套页失效时回到平面，不复用上一个场景的驻留层。
    pub(crate) fn clear_atlas(&mut self, fallback: &ClipmapFlatHeightPage) {
        self.params.options = Vec4::ZERO; // Shader 在读取页表前直接返回 Y=0。
        self.heights = fallback.heights.clone(); // 数组回退句柄保持强所有权。
        self.page_table = fallback.page_table.clone(); // 页表回退清零。
    }
}

/// 构造 RGBA32_UINT 页表的一个驻留项，调用方写入 tile 对应的 texel。
pub(crate) fn encode_page_table_entry(
    layer: u32,
    min_height: f32,
    max_height: f32,
) -> Result<[u32; 4], ClipmapHeightPageError> {
    if layer == u32::MAX {
        return Err(ClipmapHeightPageError::InvalidLayerIndex); // 零哨兵要求 layer+1 不溢出。
    }
    let height_span = max_height - min_height; // 两个有限端点相减仍可能因极端输入溢出。
    if !min_height.is_finite()
        || !max_height.is_finite()
        || max_height < min_height
        || !height_span.is_finite()
    {
        return Err(ClipmapHeightPageError::InvalidHeightRange); // 不允许把非有限浮点位写进页表。
    }
    Ok([layer + 1, min_height.to_bits(), height_span.to_bits(), 0]) // x 为驻留层加一；y/z 按位保存解码范围。
}

/// Bevy 仍使用 StandardMaterial 的完整光照，仅覆写必要入口。
impl MaterialExtension for ClipmapHeightExtension {
    fn vertex_shader() -> ShaderRef {
        MAIN_VERTEX_SHADER.clone().into() // 主视图从页取顶点 Y 与坡面法线。
    }

    fn prepass_vertex_shader() -> ShaderRef {
        PREPASS_VERTEX_SHADER.clone().into() // 阴影、深度、法线和运动全部使用同源高度。
    }

    fn deferred_vertex_shader() -> ShaderRef {
        PREPASS_VERTEX_SHADER.clone().into() // Deferred GBuffer 避免回到平面顶点。
    }

    fn fragment_shader() -> ShaderRef {
        ShaderRef::Default // 正式新图继续使用 TerrainSurfaceMaterial 的地表预设片元。
    }

    fn deferred_fragment_shader() -> ShaderRef {
        ShaderRef::Default // Deferred 同样沿用地表预设、天气和光追元数据路径。
    }

    fn prepass_fragment_shader() -> ShaderRef {
        PREPASS_FRAGMENT_SHADER.clone().into() // 深度-only 阴影也必须执行场景外 discard。
    }

    fn alpha_mode() -> Option<AlphaMode> {
        Some(AlphaMode::Mask(0.5)) // 让 Bevy 为深度-only 和阴影绑定材质并运行裁切片元。
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.vertex.shader_defs.push("SPARSE_CLIPMAP".into()); // 顶点入口与调试信息保持新图身份。
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader_defs.push("SPARSE_CLIPMAP".into()); // 现有 TerrainSurface 片元据此导入边界裁切函数。
            // 白模基材原本沿用未裁切的标准PBR片元；源外中心会将覆盖环扩到其他对象区域。
            // 仅替换Bevy明确声明的标准入口（Forward/Deferred同源），正式地表和深度片元保持各自入口。
            if let ShaderRef::Path(standard) = <StandardMaterial as bevy::pbr::Material>::fragment_shader()
                && fragment.shader.path() == Some(&standard)
            {
                fragment.shader = MAIN_FRAGMENT_SHADER.clone();
            }
        }
        Ok(())
    }
}

/// Clipmap显示网格复用同一高度页绑定，但拥有独立的线模式管线与生命周期。
#[cfg(any(feature = "test_panel", feature = "author_view"))] // 正式包不编译作者辅助材质。
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)] // 与正式高度扩展保持相同GPU绑定布局。
pub(super) struct ClipmapGridExtension {
    #[uniform(125)]
    proxy_axes: ClipmapProxyAxes,
    #[uniform(124)]
    holes: ClipmapHoleUniform,
    #[uniform(120)] // 与正式Clipmap顶点Shader的参数绑定完全一致。
    params: ClipmapHeightUniform, // 场景边界、格距与页采用门来自当前正式修订。
    #[texture(121, dimension = "2d_array")] // 复用当前R16_UNORM高度数组句柄。
    #[sampler(122)] // 复用正式页内线性采样器。
    heights: Handle<Image>, // 只克隆强句柄，不复制任何高度像素。
    #[texture(123, sample_type = "u_int")] // 复用当前RGBA32_UINT整数页表。
    page_table: Handle<Image>, // 页表与高度数组始终来自同一extension_revision。
} // 完成专用线框高度绑定定义。

#[cfg(any(feature = "test_panel", feature = "author_view"))] // 转换能力只供F12 Clipmap覆盖层使用。
impl From<&ClipmapHeightExtension> for ClipmapGridExtension {
    fn from(source: &ClipmapHeightExtension) -> Self {
        // 从当前已采用正式高度扩展建立轻量绑定快照。
        Self {
            holes: source.holes,
            proxy_axes: source.proxy_axes,
            params: source.params, // 固定4416字节Uniform按值复制，保持高度和共同边参数一致。
            heights: source.heights.clone(), // 强句柄共享同一GPU高度页资产。
            page_table: source.page_table.clone(), // 强句柄共享同一页表资产。
        } // 不读取、复制或重建Mesh与图片内容。
    } // 完成高度扩展到线框扩展转换。
} // 结束专用线框扩展转换实现。

/// 高度感知线框复用正式主顶点与片元边界裁切，只关闭阴影/预通道并改为三角线模式。
#[cfg(any(feature = "test_panel", feature = "author_view"))] // 发布构建不专用化额外线框管线。
impl MaterialExtension for ClipmapGridExtension {
    fn vertex_shader() -> ShaderRef {
        // 主通道必须执行与正式地形相同的GPU顶点位移。
        MAIN_VERTEX_SHADER.clone().into() // 复用stable_world_xz与height_at调用，不画Y=0平面。
    } // 完成线框顶点Shader选择。

    fn fragment_shader() -> ShaderRef {
        // 场景边缘仍需与正式地形一致裁切。
        MAIN_FRAGMENT_SHADER.clone().into() // 复用inside_scene，避免环带场景外边线泄漏。
    } // 完成线框片元Shader选择。

    fn enable_prepass() -> bool {
        // 作者辅助线不提供深度、法线、运动或Deferred输入。
        false // 避免网格覆盖层污染正式地形GBuffer。
    } // 完成预通道禁用。

    fn enable_shadows() -> bool {
        // 辅助线不属于世界几何。
        false // 不生成第二份地形阴影投射。
    } // 完成阴影禁用。

    fn specialize(
        _pipeline: &MaterialExtensionPipeline, // 基础StandardMaterial专用化结果保持不变。
        descriptor: &mut RenderPipelineDescriptor, // 只修改本材质自己的primitive状态与Shader定义。
        _layout: &MeshVertexBufferLayoutRef,   // 组合Mesh布局由共享主顶点Shader直接消费。
        _key: MaterialExtensionKey<Self>,      // 线框材质没有额外按实体分支键。
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.polygon_mode = PolygonMode::Line; // GPU按原三角索引直接画线，不创建边列表。
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            // 与Bevy原生Wireframe3dPipeline一致：倾斜三角面的线/面深度插值不同，只有constant=1仍会断线。
            depth.bias.slope_scale = 1.0; // 仅改变辅助线的深度测试，不抬高真实地形或改变作者高度。
        }
        descriptor.vertex.shader_defs.push("SPARSE_CLIPMAP".into()); // 保持正式Clipmap顶点编译身份。
        if let Some(fragment) = descriptor.fragment.as_mut() {
            // 前向线框存在标准片元阶段。
            fragment.shader_defs.push("SPARSE_CLIPMAP".into()); // 启用同源场景边界裁切路径。
        } // 完成可选片元定义注入。
        Ok(()) // 其余深度、混合和颜色目标沿用StandardMaterial前向管线。
    } // 完成Clipmap线框管线专用化。
} // 结束高度感知线框扩展实现。

/// Clipmap专用线框资产只共享源Mesh和当前高度页，不替代正式地表材质。
#[cfg(any(feature = "test_panel", feature = "author_view"))] // 发布构建不登记该资产类型。
pub(super) type ClipmapGridMaterial = ExtendedMaterial<StandardMaterial, ClipmapGridExtension>; // 独立draw不获得正式地形身份。

/// 地表PBR加载或预热期间的高度兜底材质；正式地表权重完成后由组合材质接管。
pub(crate) type ClipmapHeightMaterial = ExtendedMaterial<StandardMaterial, ClipmapHeightExtension>; // 不改旧 TerrainSurfaceMaterial。

/// 新稀疏场景的正式材质：内层完整保留已有地表预设，外层只负责高度位移与边界。
pub type SparseClipmapSurfaceMaterial =
    ExtendedMaterial<TerrainSurfaceMaterial, ClipmapHeightExtension>;

/// 地表系统继续拥有内层作者材质；本租约证明当前外层只包装该精确资产版本。
#[derive(bevy::prelude::Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SparseClipmapSurfaceLease {
    source: AssetId<TerrainSurfaceMaterial>,
    installed: AssetId<SparseClipmapSurfaceMaterial>,
}

impl SparseClipmapSurfaceLease {
    /// 冻结本次内外两层材质身份，热更新时不允许把旧包装误留给新作者源。
    pub(crate) fn new(
        source: &Handle<TerrainSurfaceMaterial>,
        installed: &Handle<SparseClipmapSurfaceMaterial>,
    ) -> Self {
        Self {
            source: source.id(),
            installed: installed.id(),
        }
    }

    /// 地表仲裁只在两端资产 ID 都仍精确匹配时保留外层包装。
    pub(crate) fn matches(
        &self,
        source: AssetId<TerrainSurfaceMaterial>,
        installed: AssetId<SparseClipmapSurfaceMaterial>,
    ) -> bool {
        self.source == source && self.installed == installed
    }
}

/// 插件共享的合法空数组和零页表，避免 AsBindGroup 在空图时失败。
#[derive(Resource, Clone)]
pub(crate) struct ClipmapFlatHeightPage {
    pub heights: Handle<Image>,    // 两层一乘一 R16_UNORM 高度数组。
    pub page_table: Handle<Image>, // 一乘一 RGBA32_UINT 零入口页表。
}

impl FromWorld for ClipmapFlatHeightPage {
    fn from_world(world: &mut World) -> Self {
        let mut heights = Image::new_fill(
            Extent3d {
                width: 1,
                height: 2,
                depth_or_array_layers: 1,
            }, // 两张一乘一切片按竖向堆叠。
            TextureDimension::D2,            // 稍后转成 texture_2d_array<f32>。
            &[0, 0],                         // R16_UNORM 的零高度编码。
            TextureFormat::R16Unorm,         // 与正式 257×257 数值页一致。
            RenderAssetUsages::RENDER_WORLD, // CPU 不读取固定回退图。
        );
        heights
            .reinterpret_stacked_2d_as_array(2)
            .expect("两层空高度图必须可解释为二维数组"); // WebGPU array 绑定至少两层。
        heights.sampler = ImageSampler::linear(); // 与未来高度页保持相同采样器类型。
        let page_table = Image::new_fill(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            }, // 最小合法整数页表。
            TextureDimension::D2,      // 通过 textureLoad 按 tile 整数坐标查询。
            &[0; 16],                  // RGBA32_UINT 零入口表示没有驻留页。
            TextureFormat::Rgba32Uint, // layer+1/minY/spanY/保留四个 u32 字段。
            RenderAssetUsages::RENDER_WORLD, // CPU 不读取固定页表。
        );
        let mut assets = world.resource_mut::<Assets<Image>>(); // 同一图片资产表保持两个强句柄。
        Self {
            heights: assets.add(heights),
            page_table: assets.add(page_table),
        } // 无页材质始终具有合法两类 GPU 绑定。
    }
}

/// 注册五份内置 WGSL、基础高度兜底材质和正式组合材质，不在帧关键路径读取磁盘源码。
pub(crate) struct ClipmapHeightMaterialPlugin; // 由动态场景插件显式安装一次。

fn load_height_shader(source: &str, _: std::borrow::Cow<'_, str>) -> Shader {
    let stitches = feathrise_environment::vegetation::blade::clipmap_stitch_shader_source(false)
        .replace("#{MATERIAL_BIND_GROUP}", "constants::MATERIAL_BIND_GROUP");
    Shader::from_wesl(
        format!("{stitches}\n{source}"),
        "embedded://feathrise_scene/scenes/dynamic_scene/clipmap/shaders/height.wesl",
    )
}

impl Plugin for ClipmapHeightMaterialPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(
            app,
            HEIGHT_FUNCTIONS_SHADER,
            "shaders/height.wesl",
            load_height_shader
        ); // 共用纹理页函数。
        load_internal_asset!(
            app,
            MAIN_VERTEX_SHADER,
            "shaders/main_vertex.wesl",
            |source, _| Shader::from_wesl(source, "embedded://feathrise_scene/scenes/dynamic_scene/clipmap/shaders/main_vertex.wesl")
        ); // 主视图位移入口。
        load_internal_asset!(
            app,
            PREPASS_VERTEX_SHADER,
            "shaders/prepass_vertex.wesl",
            |source, _| Shader::from_wesl(source, "embedded://feathrise_scene/scenes/dynamic_scene/clipmap/shaders/prepass_vertex.wesl")
        ); // 阴影与预通道位移入口。
        load_internal_asset!(
            app,
            MAIN_FRAGMENT_SHADER,
            "shaders/main_fragment.wesl",
            |source, _| Shader::from_wesl(source, "embedded://feathrise_scene/scenes/dynamic_scene/clipmap/shaders/main_fragment.wesl")
        ); // Forward 与 Deferred 片元。
        load_internal_asset!(
            app,
            PREPASS_FRAGMENT_SHADER,
            "shaders/prepass_fragment.wesl",
            |source, _| Shader::from_wesl(source, "embedded://feathrise_scene/scenes/dynamic_scene/clipmap/shaders/prepass_fragment.wesl")
        ); // 深度/阴影片元裁切。
        app.init_resource::<ClipmapFlatHeightPage>() // 无页时 Y=0 且绑定始终合法。
            .add_plugins((
                super::overlay::ClipmapVegetationOverlayPlugin, // F12 独立共享网格覆盖，不修改正式地表片元。
                MaterialPlugin::<ClipmapHeightMaterial>::default(), // 地表PBR未就绪或失败时仍保持真实高度位移。
                MaterialPlugin::<SparseClipmapSurfaceMaterial>::default(), // 正式新图把高度外层和现有地表片元组合为一个 draw。
            ));
    }
}

/// Vec2.y 统一对应世界 Z，不从 UV 或相机局部坐标推断场景边界。
fn bounds_vec4(bounds: ClipmapBounds) -> Vec4 {
    Vec4::new(bounds.min.x, bounds.min.y, bounds.max.x, bounds.max.y) // 与 WGSL xy/zw 契约一致。
}

#[cfg(test)]
mod tests {
    use super::*; // 测试只构造材质数据与正式 WGSL 源，不启动 GPU。
    use bevy::prelude::Vec2; // 测试场景边界使用二维 X/Z。

    #[test]
    fn height_page_validation_keeps_flat_fallback_until_complete() {
        let bounds = ClipmapBounds::new(Vec2::ZERO, Vec2::splat(64.0)).unwrap(); // 测试场景范围。
        let fallback = ClipmapFlatHeightPage {
            heights: Handle::default(),
            page_table: Handle::default(),
        }; // 无窗口的两个合法绑定占位。
        let mut extension =
            ClipmapHeightExtension::new_flat(bounds, UVec2::splat(512), &fallback).unwrap(); // 缺页状态必须是平面。
        assert_eq!(extension.params.grid, Vec4::new(0.125, 0.125, 512.0, 512.0)); // 世界格距从实际单元数派生。
        assert_eq!(extension.params.options.y, 0.0); // GPU 不会读取占位页表作为真实地形。
        assert_eq!(
            encode_page_table_entry(0, 2.0, 1.0),
            Err(ClipmapHeightPageError::InvalidHeightRange)
        ); // 错误高度范围不能入表。
        let bad = ClipmapHeightAtlas {
            heights: Handle::default(),
            page_table: Handle::default(),
            max_mip: 9,
        }; // 超过稀疏格式支持的最高父页层级。
        assert_eq!(
            extension.set_atlas(bad),
            Err(ClipmapHeightPageError::InvalidMip)
        ); // 错误资产不得切换材质。
        assert_eq!(extension.params.options.y, 0.0); // 失败后仍是有效平面托底。
        let entry = encode_page_table_entry(3, -100.0, 200.0).unwrap(); // 第四驻留层的局部高度范围。
        assert_eq!(entry, [4, (-100.0f32).to_bits(), 300.0f32.to_bits(), 0]); // 0 仍保留给缺页哨兵。
        let atlas = ClipmapHeightAtlas {
            heights: Handle::default(),
            page_table: Handle::default(),
            max_mip: 0,
        }; // 最小场景只需要基础作者层的页表/层数组。
        extension.set_atlas(atlas).unwrap(); // 通过验证后才发布整套资产。
        assert_eq!(extension.params.options.y, 1.0); // GPU 开始读取页表。
        extension.clear_atlas(&fallback); // 模拟切换场景或页失效。
        assert_eq!(extension.params.options, Vec4::ZERO); // 清页不遗留旧场景高度。
    }

    #[test]
    fn all_shader_passes_share_height_and_bounds_contract() {
        let common = format!("{}\n{}",feathrise_environment::vegetation::blade::clipmap_stitch_shader_source(false),include_str!("shaders/height.wesl")); // 与正式加载器装配相同的共享源码。
        let main_vertex = include_str!("shaders/main_vertex.wesl"); // Forward 顶点。
        let prepass_vertex = include_str!("shaders/prepass_vertex.wesl"); // 阴影、深度、运动顶点。
        let main_fragment = include_str!("shaders/main_fragment.wesl"); // Forward/Deferred 地表片元。
        let prepass_fragment = include_str!("shaders/prepass_fragment.wesl"); // 阴影、深度裁切片元。
        assert!(common.contains("@binding(120)")); // 参数与 Rust AsBindGroup 同槽。
        assert!(common.contains("@binding(121)")); // 高度页数组与 Rust AsBindGroup 同槽。
        assert!(common.contains("@binding(122)")); // 采样器与 Rust AsBindGroup 同槽。
        assert!(common.contains("@binding(123)")); // 整数页表与 Rust AsBindGroup 同槽。
        assert!(common.contains("fn stable_world_xz")); // 粗层不能随组合实体的最细格平移而游动。
        assert!(main_vertex.contains("clipmap_height::height_at")); // 主顶点位移。
        assert!(prepass_vertex.contains("clipmap_height::height_at")); // 阴影/预通道位移。
        assert!(main_vertex.contains("clipmap_height::stable_world_xz")); // Forward 使用世界稳定层中心。
        assert!(prepass_vertex.contains("clipmap_height::stable_world_xz")); // 深度/阴影位置必须一致。
        assert!(prepass_vertex.contains("previous_world_position = out.world_position")); // 静止世界地形的运动向量。
        assert!(main_fragment.contains("clipmap_height::inside_surface")); // 主画面场景边界与洞口一致裁切。
        assert!(prepass_fragment.contains("clipmap_height::inside_surface")); // 阴影/深度不能封住洞口。
    }

    #[test]
    fn shared_height_shader_is_valid_wgsl() {
        let height = include_str!("shaders/height.wesl").replace("constants::MATERIAL_BIND_GROUP", "2");
        let stitches = feathrise_environment::vegetation::blade::clipmap_stitch_shader_source(false)
            .replace("#{MATERIAL_BIND_GROUP}", "2");
        let validation_source = format!("{stitches}\n{height}");
        let module = wgpu::naga::front::wgsl::parse_str(&validation_source)
            .unwrap_or_else(|error| panic!("Clipmap 高度页 WGSL 语法错误：{error}")); // 输出 Naga 的准确行列。
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(), // 检查数组纹理、整数页表和显式 mip 调用。
            wgpu::naga::valid::Capabilities::all(), // 与项目其他生产 Shader 测试采用同一完整能力集。
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("Clipmap 高度页 WGSL 语义错误：{error}"));
        // 阻止错误延迟到进图建管线。
    }
}
