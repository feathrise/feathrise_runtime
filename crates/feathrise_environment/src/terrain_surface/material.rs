//! 通用八层地表权重材质的数据布局与PBR扩展实现。

use bevy::color::Color; // 公共层参数接受作者易读的sRGB颜色并在CPU端转换为线性值。
use bevy::pbr::{ExtendedMaterial, MaterialExtension}; // 复用StandardMaterial完整PBR、阴影、雾和Deferred流程。
use bevy::prelude::{Asset, Handle, Image, Vec2, Vec4}; // 材质描述符只保存固定参数和强图片句柄。
use bevy::reflect::TypePath; // 自定义材质资产需要稳定运行时类型路径。
use bevy::render::render_resource::{AsBindGroup, ShaderType}; // 自动建立固定uniform与纹理数组绑定。
use bevy::render::storage::ShaderBuffer; // 动态场景用存储缓冲表达任意数量的局部地表层。
use bevy::shader::ShaderRef; // 前向和Deferred入口共用正文，但使用设备能力边界不同的内置句柄。

use super::shaders::{TERRAIN_SURFACE_DEFERRED_SHADER_HANDLE, TERRAIN_SURFACE_SHADER_HANDLE}; // Deferred专用句柄只在源码首部增加primitive_index扩展声明。

/// 单张或双张RGBA权重纹理最多表达的正式地表层数。
pub const MAX_TERRAIN_SURFACE_LAYERS: usize = 8; // 两张RGBA各提供四个线性权重通道。

/// 一层地表的固定PBR与世界平铺参数。
#[derive(Clone, Copy, Debug, PartialEq)] // 场景规格可在栈上构造固定八项并无分配传入材质。
pub struct TerrainSurfaceLayer {
    pub base_color: Color, // 无纹理层直接使用该颜色；有纹理层用它乘基础色数组采样。
    pub perceptual_roughness: f32, // 无纹理层直接使用；有ARM层与数组绿色通道相乘。
    pub metallic: f32,     // 无纹理层直接使用；有ARM层与数组蓝色通道相乘。
    pub tile_meters: f32,  // 基础色与ARM数组沿世界X/Z每多少米重复一次。
    pub texture_layer: Option<u32>, // None表示纯参数层，Some选择基础色与ARM数组的同号切片。
} // 完成单层公开参数定义。

impl TerrainSurfaceLayer {
    // 提供不会混淆颜色空间和数组索引的具名构造器。
    /// 构造只使用固定颜色和PBR参数的地表层。
    pub fn solid(
        // 把纯参数层的必需字段集中初始化。
        base_color: Color,         // 接收作者通常使用的sRGB颜色。
        perceptual_roughness: f32, // 接收零至一感知粗糙度。
        metallic: f32,             // 接收零至一金属度。
    ) -> Self {
        // 返回无需数组纹理采样的层。
        Self {
            // 使用安全范围构造完整值。
            base_color, // Shader上传前再统一转换为线性空间。
            perceptual_roughness: perceptual_roughness.clamp(0.0, 1.0), // 防止错误规格产生非法PBR输入。
            metallic: metallic.clamp(0.0, 1.0), // 地表通常为零金属，仍保留通用接口。
            tile_meters: 1.0,                   // 纯参数层不采样纹理，使用合法占位尺度避免除零。
            texture_layer: None,                // 明确跳过基础色与ARM数组采样。
        } // 返回纯参数层。
    } // 完成纯参数层构造。

    /// 构造从基础色和ARM二维数组读取同号切片的地表层。
    pub fn textured(
        // 把数组切片与世界平铺契约集中初始化。
        base_color: Color,         // 颜色作为纹理乘色，可使用白色表示不改变源图。
        perceptual_roughness: f32, // 粗糙度作为ARM绿色通道的乘数。
        metallic: f32,             // 金属度作为ARM蓝色通道的乘数。
        tile_meters: f32,          // 纹理沿世界X/Z的米制重复周期。
        texture_layer: u32,        // 基础色与ARM数组共享的切片编号。
    ) -> Self {
        // 返回需要两次数组采样的层。
        Self {
            // 使用安全范围构造完整值。
            base_color, // Shader上传前统一转换为线性空间。
            perceptual_roughness: perceptual_roughness.clamp(0.0, 1.0), // 限制到标准PBR范围。
            metallic: metallic.clamp(0.0, 1.0), // 限制到标准PBR范围。
            tile_meters: tile_meters.max(0.01), // 防止损坏配置造成世界UV除零。
            texture_layer: Some(texture_layer), // 同号切片同时驱动基础色与ARM。
        } // 返回纹理层。
    } // 完成纹理层构造。
} // 完成单层构造接口。

impl Default for TerrainSurfaceLayer {
    // 未使用的固定数组槽必须保持安全中性值。
    fn default() -> Self {
        // 构造不贡献可见结果的默认层。
        Self::solid(Color::WHITE, 1.0, 0.0) // 白色高粗糙非金属不会污染归一化后的有效层。
    } // 完成默认层构造。
} // 完成默认实现。

/// 场景层构造一份通用地表材质所需的完整固定描述。
#[derive(Clone, Debug)] // 描述符只在创建或热更新材质时克隆，不进入逐帧热路径。
pub struct TerrainSurfaceDescriptor {
    pub world_min: Vec2,                   // 权重纹理左下角对应的世界X/Z米制坐标。
    pub world_max: Vec2,                   // 权重纹理右上角对应的世界X/Z米制坐标。
    pub weight_maps: [Handle<Image>; 2],   // 第一张表达层零至三，第二张表达层四至七或内部零图。
    pub weight_map_count: u32,             // HN-01为一；未来六至八层场景可设为二。
    pub base_color_array: Handle<Image>,   // 所有纹理层共享的一张sRGB二维数组。
    pub arm_array: Handle<Image>,          // 所有纹理层共享的一张线性AO/粗糙度/金属度二维数组。
    pub normal_array: Handle<Image>,       // 旧固定层模式绑定中性数组，动态索引模式则读取真实法线。
    pub index_map: Handle<Image>,          // 旧模式绑定零索引回退，但不进行取样。
    pub layer_table: Handle<ShaderBuffer>, // 旧模式绑定中性表，以保持唯一管线布局。
    pub layers: [TerrainSurfaceLayer; MAX_TERRAIN_SURFACE_LAYERS], // 固定八层避免运行时动态uniform和管线变体。
    pub layer_count: u32,                 // 当前场景实际使用的一至八层数量。
    pub residual_layer: u32,              // 指定用一减显式通道和得到权重的默认地表层。
    pub grass_layer: u32,                 // 指定唯一应用低成本综合色与微法线的草土层。
    pub grass_micro_normal_strength: f32, // 草土微法线强度，零可完全关闭额外扰动。
    pub grass_variation_cell_meters: f32, // 世界稳定综合色噪声的米制格尺度。
} // 完成通用场景描述符定义。

/// 动态场景四槽索引地表所需的完整绑定。
#[derive(Clone, Debug)]
pub struct TerrainSurfaceIndexedDescriptor {
    pub world_min: Vec2,
    pub world_max: Vec2,
    pub weight_map: Handle<Image>,
    pub index_map: Handle<Image>,
    pub base_color_array: Handle<Image>,
    pub normal_array: Handle<Image>,
    pub arm_height_array: Handle<Image>,
    pub layer_table: Handle<ShaderBuffer>,
    pub layer_count: u32,
    /// 索引模式不取样第二张旧权重图，仍用回退句柄保持绑定完整。
    pub zero_weight: Handle<Image>,
}

/// 单层参数的GPU对齐镜像；两个vec4保持固定三十二字节布局。
#[derive(Clone, Copy, Debug, PartialEq, ShaderType)] // ShaderType保证Rust与WGSL数组步长完全一致。
struct TerrainSurfaceLayerUniform {
    base_color: Vec4,      // 线性RGBA乘色或纯参数颜色。
    pbr_and_texture: Vec4, // x粗糙度、y金属度、z平铺米数、w为切片编号加一或零。
} // 完成单层GPU布局。

/// 动态场景存储表的单层 GPU 镜像；三个 vec4 保持固定四十八字节步长。
#[repr(C)]
#[derive(Clone, Copy, Debug, ShaderType, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TerrainSurfaceIndexedLayer {
    pub base_color: Vec4, // 线性 RGBA 乘色。
    /// x=粗糙度、y=金属度、z=平铺米数、w=全局数组切片。
    pub pbr_and_texture: Vec4,
    /// x=法线强度、y=高度混合强度、z=投影编码、w=法线 Y 翻转。
    pub normal_height_projection: Vec4,
}

/// 通用地表材质的固定GPU参数。
#[derive(Clone, Copy, Debug, PartialEq, ShaderType)] // 固定数组避免每个场景产生不同绑定组布局。
struct TerrainSurfaceUniform {
    world_bounds: Vec4, // xy/zw分别为权重纹理世界X/Z最小与最大值。
    counts: Vec4,       // 旧路径的x层数、y权重图数、z残余层、w草土层；索引路径层数读取存储表长度。
    appearance: Vec4,   // x草土微法线强度、y噪声格米数、z为动态地形光追元数据开关，w保留扩展。
    shading: Vec4,      // x为平直着色开关，y为子场景积水绑定，z为本来源遮罩启用；不增加材质变体。
    volume_shading: Vec4, // x为体积三向投影，y为体积权重查表；与子场景积水开关独立。
    source_to_world: bevy::math::Mat4,
    world_to_source: bevy::math::Mat4,
    layers: [TerrainSurfaceLayerUniform; MAX_TERRAIN_SURFACE_LAYERS], // 八层PBR与纹理切片固定表。
} // 完成通用GPU参数布局。

/// 附加到白模StandardMaterial上的通用地表PBR权重扩展。
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, PartialEq)] // 比较固定参数和句柄，避免高度页移动时反复复制体积材质。
pub struct TerrainSurfaceExtension {
    #[uniform(100)]
    params: TerrainSurfaceUniform, // 绑定从100开始，避免与StandardMaterial既有绑定冲突。
    #[texture(101)]
    #[sampler(102)]
    weight_map_0: Handle<Image>, // 第一张线性RGBA权重图覆盖层零至三。
    #[texture(103)]
    #[sampler(104)]
    weight_map_1: Handle<Image>, // 第二张权重图或插件提供的一乘一零图。
    #[texture(105, dimension = "2d_array")]
    #[sampler(106)]
    base_color_array: Handle<Image>, // sRGB二维数组只为显式纹理层提供基础色。
    #[texture(107, dimension = "2d_array")]
    #[sampler(108)]
    arm_array: Handle<Image>, // 线性二维数组提供AO、粗糙度和金属度。
    #[texture(109, dimension = "2d_array")]
    #[sampler(110)]
    normal_array: Handle<Image>, // 线性切线空间法线数组。
    #[texture(111, sample_type = "u_int")]
    index_map: Handle<Image>, // 四个 u32 场景局部层索引，只能使用 textureLoad。
    #[storage(112, read_only)]
    layer_table: Handle<ShaderBuffer>, // 场景局部层到全局预设切片和 PBR 参数的当前快照。
    #[storage(113, read_only)]
    volume_lookup: Handle<ShaderBuffer>, // 三维稳定ID解析表；旧路径不开启查询。
    #[storage(114, read_only)]
    volume_fields: Handle<ShaderBuffer>, // 原格三维笔触表，LOD和重网格不会移动作者数据。
    #[uniform(115)]
    volume_origin_step: Vec4, // xyz根平移，w为作者格距；零表示旧路径。
    #[texture(116)] // 独立积水潜力图，不与地表权重或植被密度混用。
    #[sampler(117)] // 线性过滤使作者笔划边缘连续。
    puddle_mask: Option<Handle<Image>>, // 无作者图时绑定Bevy回退，Shader禁用采样。
    #[storage(119, read_only)] // 与河湖水面共享固定64字节天气数据。
    surface_weather: Handle<ShaderBuffer>, // 天气变化不需要逐个材质更新。
    #[storage(118, read_only)] // 独立固定遮雨代理缓冲，不改变64字节天气历史ABI。
    rain_shelter: Handle<ShaderBuffer>, // 无碰撞屋檐同样遮挡地表雨湿。
} // 完成材质绑定定义。

impl TerrainSurfaceExtension {
    /// 实例变换只更新小参数，作者纹理和地表图集始终使用源场景坐标。
    pub fn set_source_frame(&mut self, source_to_world: bevy::math::Mat4) -> bool {
        if self.params.source_to_world == source_to_world { return false; }
        self.params.source_to_world = source_to_world;
        self.params.world_to_source = source_to_world.inverse();
        true
    }
    pub fn source_frame(&self) -> bevy::math::Mat4 { self.params.source_to_world }

    /// 只叠加雨天湿润，完整保留Blender原有StandardMaterial颜色、纹理、法线和PBR。
    pub fn new_passthrough(fallback: &super::resources::TerrainSurfaceFallbackImages) -> Self {
        // 旧场景无需伪造地表作者层。
        Self {
            // 全部未使用纹理仍绑定合法中性资产，不增加加载任务。
            params: TerrainSurfaceUniform {
                source_to_world: bevy::math::Mat4::IDENTITY,
                world_to_source: bevy::math::Mat4::IDENTITY,
                // 直通路径同样保留统一地形着色参数布局。
                world_bounds: Vec4::new(0.0, 0.0, 1.0, 1.0), // 未使用层采样但保留有限范围。
                counts: Vec4::ZERO,                          // 原材质直通没有地表层。
                appearance: Vec4::new(0.0, 1.0, 0.0, 2.0), // w=2跳过所有层采样；z=0不写光追三角元数据。
                shading: Vec4::ZERO,
                volume_shading: Vec4::ZERO, // 默认保留Blender平滑法线，F1运行时可只翻转x分量。
                layers: [TerrainSurfaceLayerUniform {
                    base_color: Vec4::ONE,
                    pbr_and_texture: Vec4::new(1.0, 0.0, 1.0, 0.0),
                }; MAX_TERRAIN_SURFACE_LAYERS], // 未读取的固定层保持有限中性值。
            }, // 完成原材质直通身份。
            weight_map_0: fallback.zero_weight.clone(), // 直通不采样权重，但绑定必须完整。
            weight_map_1: fallback.zero_weight.clone(), // 复用同一回退图。
            base_color_array: fallback.white_base_color_array.clone(), // 不替代Blender基础色纹理。
            arm_array: fallback.neutral_arm_array.clone(), // 不替代Blender金属度或粗糙度。
            normal_array: fallback.neutral_normal_array.clone(), // 不替代Blender法线贴图。
            index_map: fallback.zero_index.clone(),     // 直通不读取索引纹理。
            layer_table: fallback.neutral_layer_table.clone(), // 直通不读取动态层表。
            volume_lookup: fallback.neutral_layer_table.clone(),
            volume_fields: fallback.neutral_layer_table.clone(),
            volume_origin_step: Vec4::ZERO,
            puddle_mask: None, // HN02没有作者积水遮罩时只表现湿润，禁止伪造水坑。
            surface_weather: super::weather::surface_weather_buffer_handle(), // 与河湖及动态场景使用同源天气。
            rain_shelter: crate::rain_shelter::rain_shelter_buffer_handle(), // 默认材质持有完整空代理回退。
        } // 返回保留原材质的合法扩展。
    } // 结束旧场景雨天接入构造。
    // 场景层通过描述符一次性创建完整扩展。
    /// 把公开场景描述符转换为固定GPU布局和四张绑定纹理。
    pub fn new(descriptor: TerrainSurfaceDescriptor) -> Self {
        // 创建与热更新共用同一确定性转换。
        let mut layers = [TerrainSurfaceLayerUniform {
            // 先建立八个不会污染结果的中性槽。
            base_color: Vec4::ONE, // 未使用槽保持白色。
            pbr_and_texture: Vec4::new(1.0, 0.0, 1.0, 0.0), // 高粗糙非金属且不采样数组。
        }; MAX_TERRAIN_SURFACE_LAYERS]; // 固定长度与Shader声明一致。
        for (index, layer) in descriptor.layers.iter().enumerate() {
            // 将作者层逐项转换为线性GPU值。
            let linear = layer.base_color.to_linear(); // sRGB作者颜色只在CPU端转换一次。
            let texture_code = layer.texture_layer.map_or(0.0, |slice| slice as f32 + 1.0); // 零表示纯参数层，其余值减一后得到数组切片。
            layers[index] = TerrainSurfaceLayerUniform {
                // 写入当前固定层槽。
                base_color: Vec4::new(linear.red, linear.green, linear.blue, linear.alpha), // 保持纹理混合在线性空间进行。
                pbr_and_texture: Vec4::new(
                    // 把四个标量压入单一对齐向量。
                    layer.perceptual_roughness.clamp(0.0, 1.0), // x保存标准感知粗糙度。
                    layer.metallic.clamp(0.0, 1.0),             // y保存标准金属度。
                    layer.tile_meters.max(0.01),                // z保存合法世界平铺周期。
                    texture_code,                               // w保存零或切片编号加一。
                ), // 完成当前层PBR向量。
            }; // 完成当前层GPU值写入。
        } // 完成八层固定转换。
        let layer_count = descriptor
            .layer_count
            .clamp(1, MAX_TERRAIN_SURFACE_LAYERS as u32); // 损坏规格也至少保留一个可见层。
        let residual_layer = descriptor.residual_layer.min(layer_count - 1); // 残余层必须位于当前有效范围。
        let grass_layer = descriptor.grass_layer.min(layer_count - 1); // 草土效果不得索引未使用槽。
        Self {
            // 一次构造完整扩展与所有强句柄。
            params: TerrainSurfaceUniform {
                source_to_world: bevy::math::Mat4::IDENTITY,
                world_to_source: bevy::math::Mat4::IDENTITY,
                // 写入固定GPU参数。
                world_bounds: Vec4::new(
                    // 将世界范围压入一个对齐向量。
                    descriptor.world_min.x, // x保存世界最小X。
                    descriptor.world_min.y, // y保存世界最小Z。
                    descriptor.world_max.x, // z保存世界最大X。
                    descriptor.world_max.y, // w保存世界最大Z。
                ), // 完成世界范围向量。
                counts: Vec4::new(
                    // 小整数在八层范围内可由f32无损保存。
                    layer_count as f32, // x保存有效层数。
                    descriptor.weight_map_count.clamp(1, 2) as f32, // y保存一或两张权重图。
                    residual_layer as f32, // z保存残余层编号。
                    grass_layer as f32, // w保存草土层编号。
                ), // 完成计数向量。
                appearance: Vec4::new(
                    // 保存草土专属低成本表现参数。
                    descriptor.grass_micro_normal_strength.max(0.0), // x禁止负强度反转法线扰动。
                    descriptor.grass_variation_cell_meters.max(1.0), // y至少一米避免极高频噪声。
                    0.0, // z默认不占用GBuffer发光通道；仅光追动态地形的会话副本发布三角编号。
                    0.0, // w=0 明确选择旧固定层权重路径。
                ), // 完成外观向量。
                shading: Vec4::ZERO,
                volume_shading: Vec4::ZERO, // 新建材质默认使用资产平滑法线。
                layers,              // 安装已经转换完成的固定八层表。
            }, // 完成uniform构造。
            weight_map_0: descriptor.weight_maps[0].clone(), // 绑定第一张正式权重图。
            weight_map_1: descriptor.weight_maps[1].clone(), // 绑定第二张图或内部零图。
            base_color_array: descriptor.base_color_array,   // 强持有sRGB二维数组。
            arm_array: descriptor.arm_array,                 // 强持有线性ARM二维数组。
            normal_array: descriptor.normal_array,           // 旧模式不取样，但绑定组始终完整。
            index_map: descriptor.index_map,                 // 旧模式不读取整数回退纹理。
            volume_lookup: descriptor.layer_table.clone(),
            volume_fields: descriptor.layer_table.clone(),
            volume_origin_step: Vec4::ZERO,
            layer_table: descriptor.layer_table, // 旧模式不读取中性表。
            puddle_mask: None,                   // 由完整场景/作者遮罩资源统一接入，旧图没有积水。
            surface_weather: super::weather::surface_weather_buffer_handle(), // 固定缓冲兼容新建和热更新材质。
            rain_shelter: crate::rain_shelter::rain_shelter_buffer_handle(), // 热更新继续共用当前遮雨范围。
        } // 返回可直接加入Assets的扩展。
    } // 完成描述符转换。

    /// 构造每像素四槽、场景层数不固定的 PBR 地表扩展。
    pub fn new_indexed(descriptor: TerrainSurfaceIndexedDescriptor) -> Self {
        let layers = [TerrainSurfaceLayerUniform {
            base_color: Vec4::ONE,
            pbr_and_texture: Vec4::new(1.0, 0.0, 1.0, 0.0),
        }; MAX_TERRAIN_SURFACE_LAYERS];
        Self {
            params: TerrainSurfaceUniform {
                source_to_world: bevy::math::Mat4::IDENTITY,
                world_to_source: bevy::math::Mat4::IDENTITY,
                world_bounds: Vec4::new(
                    descriptor.world_min.x,
                    descriptor.world_min.y,
                    descriptor.world_max.x,
                    descriptor.world_max.y,
                ),
                counts: Vec4::new(descriptor.layer_count as f32, 1.0, 0.0, 0.0), // 零层保留为空作者地表，Shader不会读取哨兵或全局纹理。
                appearance: Vec4::new(0.0, 1.0, 0.0, 1.0), // w=1 启用索引+存储表路径。
                shading: Vec4::ZERO,
                volume_shading: Vec4::ZERO,                       // 动态场景同样从正式平滑法线开始。
                layers,
            },
            weight_map_0: descriptor.weight_map,
            weight_map_1: descriptor.zero_weight,
            base_color_array: descriptor.base_color_array,
            arm_array: descriptor.arm_height_array,
            normal_array: descriptor.normal_array,
            index_map: descriptor.index_map,
            volume_lookup: descriptor.layer_table.clone(),
            volume_fields: descriptor.layer_table.clone(),
            volume_origin_step: Vec4::ZERO,
            layer_table: descriptor.layer_table,
            puddle_mask: None, // 独立作者遮罩就绪后增量绑定，不改变四槽地表协议。
            surface_weather: super::weather::surface_weather_buffer_handle(), // 与旧图和河湖共用固定天气数据。
            rain_shelter: crate::rain_shelter::rain_shelter_buffer_handle(), // 旧图直通地表使用相同遮雨证据。
        }
    }

    /// 只为动态地形Deferred GBuffer发布光追自遮挡修正元数据；前向材质路径不读取该标记。
    pub fn set_dynamic_terrain_shadow_terminator_data(&mut self, enabled: bool) {
        // 光追生命周期只修改自己克隆的会话材质，作者源资产与HN-01继续使用普通GBuffer契约。
        self.params.appearance.z = if enabled { 1.0 } else { 0.0 }; // 精确零或一避免Shader阈值受到任意作者浮点值影响。
    } // 完成动态地形光追元数据模式切换。

    /// 只读当前光追元数据门，供嵌套地表检查原地重包后是否需要恢复租约。
    pub fn dynamic_terrain_shadow_terminator_data_enabled(&self) -> bool {
        self.params.appearance.z > 0.5 // 与生产 Shader 的启用阈值保持一致。
    }

    /// 切换当前地形片元使用作者平滑法线还是逐三角形几何法线。
    pub fn set_flat_shading(&mut self, enabled: bool) {
        self.params.shading.x = if enabled { 1.0 } else { 0.0 }; // 精确布尔编码让Shader使用稳定阈值，不产生管线变体。
    } // 完成大场景着色模式写入。

    /// 返回当前材质是否请求逐三角形平直着色。
    pub fn flat_shading_enabled(&self) -> bool {
        self.params.shading.x > 0.5 // 只读取F1专用分量，不耦合地表模式或光追元数据。
    } // 完成大场景着色模式读取。

    /// 体积接管面沿用地表涂绘；仅陡壁自动三向投影，朝上的缓坡保持原预设投影。
    pub fn set_volume_projection(&mut self, enabled: bool) {
        self.params.volume_shading.x = if enabled { 1.0 } else { 0.0 }; // 固定uniform开关，不新增材质类型或管线变体。
    }
    /// 原生盒体顶面沿用自己的XZ画布；地下各层只采用独立三维字段。
    pub fn set_native_box_top(&mut self, top: f32) {
        self.params.volume_shading.z = top;
        self.params.volume_shading.w = 1.0;
    }
    /// 解析表与地表候选共同预热采用；体积副本只共享句柄。
    pub fn set_volume_lookup(&mut self, lookup: Handle<ShaderBuffer>) {
        self.volume_lookup = lookup;
        self.params.volume_shading.y = 1.0;
    }

    pub fn set_volume_fields(
        &mut self,
        fields: Handle<ShaderBuffer>,
        origin: bevy::math::Vec3,
        step: f32,
    ) {
        self.volume_fields = fields;
        self.volume_origin_step = origin.extend(step);
    }
    pub fn volume_projection_enabled(&self) -> bool {
        self.params.volume_shading.x > 0.5
    }

    /// 为当前材质绑定已完整就绪的独立积水遮罩。
    pub fn set_puddle_mask(&mut self, image: Option<Handle<Image>>) {
        // None明确撤销旧图遮罩。
        self.puddle_mask = image; // 强句柄保持整个GPU使用期有效。
    } // 完成遮罩接入。

    /// 绑定桥用只读比较防止自己的Modified事件形成循环。
    pub fn puddle_mask(&self) -> &Option<Handle<Image>> {
        // 不复制作者像素。
        &self.puddle_mask // 只比较稳定图片身份。
    } // 完成当前遮罩读取。
    pub fn follows_global_puddle_mask(&self) -> bool { self.params.shading.y < 0.5 }
    pub fn source_puddle_bounds(&self) -> Vec4 { self.params.world_bounds }
    pub fn set_source_puddle_mask(&mut self, image: Option<Handle<Image>>, bounds: Vec4) {
        self.params.shading.y = 1.0; self.params.shading.z = if image.is_some() { 1.0 } else { 0.0 };
        self.params.world_bounds = bounds; // 直通GLB没有作者地表层，也必须登记该源图真实遮罩范围。
        self.puddle_mask = image; // 子场景自己的遮罩与源范围不会被父图天气的全局换版覆盖。
    }
} // 完成地表扩展构造接口。

impl MaterialExtension for TerrainSurfaceExtension {
    // 复用Bevy StandardMaterial主管线并只替换片元入口。
    fn fragment_shader() -> ShaderRef {
        // 前向视图读取通用地表Shader。
        TERRAIN_SURFACE_SHADER_HANDLE.clone().into() // 返回稳定内置句柄，宿主无需复制WGSL文件。
    } // 完成前向入口。

    fn deferred_fragment_shader() -> ShaderRef {
        // Solari与Deferred视图读取同一权重混合逻辑。
        TERRAIN_SURFACE_DEFERRED_SHADER_HANDLE.clone().into() // 同一正文但扩展指令位于组合前首token，且不污染Forward能力要求。
    } // 完成Deferred入口。
} // 完成材质扩展实现。

/// 白模StandardMaterial加通用地表权重扩展后的完整Bevy材质类型。
pub type TerrainSurfaceMaterial =
    ExtendedMaterial<bevy::pbr::StandardMaterial, TerrainSurfaceExtension>; // 场景层只创建一个资产并原子接管唯一地形primitive。

#[cfg(test)] // 固定GPU布局和描述符钳制逻辑由纯CPU测试锁定。
mod tests {
    // 测试与生产类型同文件以访问私有uniform字段。
    use super::*; // 复用当前模块全部固定类型和构造器。

    const TERRAIN_SURFACE_SHADER_SOURCE: &str = include_str!("shaders/terrain_surface.wesl"); // 测试直接读取生产Shader，不维护第二份索引采样算法。

    #[test] // 原生Blender地形必须跳过地表作者层，不能被中性回退覆盖成白色。
    fn passthrough_keeps_original_material_and_disables_layer_sampling() {
        // 无GPU构造合法类型句柄验证接口契约。
        let fallback = super::super::resources::TerrainSurfaceFallbackImages {
            // 测试不需要实际像素，只检查材质字段身份。
            zero_weight: Handle::default(), // 未取样权重图。
            white_base_color_array: Handle::default(), // 未取样基础色数组。
            neutral_arm_array: Handle::default(), // 未取样ARM数组。
            neutral_normal_array: Handle::default(), // 未取样法线数组。
            zero_index: Handle::default(),  // 未取样索引图。
            neutral_layer_table: Handle::default(), // 未取样层表。
        }; // 完成最小回退夹具。
        let base = bevy::pbr::StandardMaterial {
            // 模拟HN02带明确材质差异的原始Blender地表。
            base_color: Color::srgb(0.42, 0.25, 0.11), // 黄土原色不能被扩展重置。
            perceptual_roughness: 0.87,                // 干燥原粗糙度必须保留。
            metallic: 0.03,                            // 原PBR参数保持来源值。
            ..Default::default() // 未修改的其余标准材质字段继续使用合法默认值。
        }; // 完成原始材质夹具。
        let material = TerrainSurfaceMaterial {
            // 与HN02observer相同的完整包装方式。
            base: base.clone(), // 原材质通过完整克隆保留，不提取部分字段。
            extension: TerrainSurfaceExtension::new_passthrough(&fallback), // 只增加天气处理入口。
        }; // 完成直通扩展包装。
        assert_eq!(material.base.base_color, base.base_color); // 原颜色不能被白色地表替代。
        assert_eq!(
            material.base.perceptual_roughness,
            base.perceptual_roughness
        ); // 原干燥粗糙度不丢失。
        assert_eq!(material.base.metallic, base.metallic); // 原金属参数不丢失。
        assert_eq!(material.extension.params.appearance.w, 2.0); // 必须走独立直通模式。
        assert_eq!(material.extension.params.counts, Vec4::ZERO); // 没有虚构地表层。
        assert_eq!(material.extension.params.appearance.z, 0.0); // HN02不能误写动态光追元数据。
        assert!(material.extension.puddle_mask.is_none()); // 无作者积水图时不伪造积水范围。
    } // 完成原材质直通接口回归。

    #[test] // 动态四槽采样、三平面、法线和高度混合helper必须通过当前Bevy同版本Naga校验。
    fn indexed_surface_helpers_are_valid_wgsl() {
        let helper_start = TERRAIN_SURFACE_SHADER_SOURCE
            .find("struct TerrainSurfaceLayer")
            .expect("地表Shader必须保留公共类型起点");
        let fragment_start = TERRAIN_SURFACE_SHADER_SOURCE
            .find("@fragment")
            .expect("地表Shader必须保留正式片元入口");
        let validation_source = TERRAIN_SURFACE_SHADER_SOURCE[helper_start..fragment_start]
            .replace("#{MATERIAL_BIND_GROUP}", "2"); // Bevy预处理只负责绑定组占位符，helper正文逐字使用生产源码。
        let module = wgpu::naga::front::wgsl::parse_str(&validation_source)
            .unwrap_or_else(|error| panic!("动态索引地表WGSL语法错误：{error}"));
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("动态索引地表WGSL语义错误：{error}"));
    }

    #[test] // 八层uniform大小必须保持WebGPU对齐且不会随场景内容变化。
    fn terrain_surface_uniform_keeps_fixed_layout() {
        // 防止新增字段无意改变WGSL数组步长。
        assert_eq!(std::mem::size_of::<TerrainSurfaceLayerUniform>(), 32); // 每层固定两个vec4。
        assert_eq!(std::mem::size_of::<TerrainSurfaceUniform>(), 464); // 五个vec4、两个来源矩阵加八层三十二字节，与WGSL一致。
    } // 完成GPU布局测试。

    #[test] // F12着色预览只能改变自己的精确布尔槽并可无损恢复平滑模式。
    fn flat_shading_mode_encodes_exact_boolean_without_changing_surface_mode() {
        let fallback = super::super::resources::TerrainSurfaceFallbackImages {
            zero_weight: Handle::default(), // 直通构造不读取测试权重图。
            white_base_color_array: Handle::default(), // 基础色数组只需类型正确。
            neutral_arm_array: Handle::default(), // ARM数组不参与着色模式测试。
            neutral_normal_array: Handle::default(), // 纹理法线不参与几何模式测试。
            zero_index: Handle::default(),  // 直通模式不读取索引图。
            neutral_layer_table: Handle::default(), // 直通模式不读取层表。
        }; // 完成最小合法回退资源夹具。
        let mut extension = TerrainSurfaceExtension::new_passthrough(&fallback); // HN-02路径同样必须支持切换。
        assert!(!extension.flat_shading_enabled()); // 新材质默认保留作者平滑法线。
        assert_eq!(extension.params.appearance.w, 2.0); // 直通地表模式身份保持独立。
        assert!(!extension.volume_projection_enabled());
        extension.set_source_puddle_mask(Some(Handle::default()), Vec4::new(0.0, 0.0, 1.0, 1.0));
        assert!(!extension.follows_global_puddle_mask()); // 来源积水绑定不得误开三维投影。
        assert!(!extension.volume_projection_enabled());
        extension.set_volume_projection(true);
        extension.set_flat_shading(true); // 模拟F12切换为平直着色。
        assert!(extension.flat_shading_enabled()); // 开启值必须精确越过Shader阈值。
        assert_eq!(extension.params.appearance.w, 2.0); // 着色切换不得改写直通/索引模式。
        extension.set_flat_shading(false); // 模拟F12恢复平滑着色。
        assert!(!extension.flat_shading_enabled()); // 关闭后必须无损恢复默认模式。
        assert!(extension.volume_projection_enabled()); // F12平滑/平直切换不能撤掉体积洞壁投影。
        extension.set_volume_projection(false);
        assert!(!extension.volume_projection_enabled());
        assert!(!extension.follows_global_puddle_mask()); // 关闭体积投影不能撤掉子场景独立积水。
    } // 完成地表平滑/平直着色参数回归。

    #[test] // 会话开关必须只复用既有uniform保留槽，不能改变绑定布局或作者描述符。
    fn dynamic_terrain_shadow_terminator_data_mode_encodes_exact_boolean() {
        // 使用空句柄构造最小合法描述符，测试不需要启动AssetServer或RenderApp。
        let image = Handle::<Image>::default(); // 四个纹理槽只需要类型正确的占位句柄。
        let mut extension = TerrainSurfaceExtension::new(TerrainSurfaceDescriptor {
            // 填充不影响法线模式的最小固定参数。
            world_min: Vec2::ZERO, // 使用有限世界最小边界。
            world_max: Vec2::ONE,  // 使用非零世界最大边界。
            weight_maps: [image.clone(), image.clone()], // 两张权重槽共享测试占位图。
            weight_map_count: 1,   // 只启用第一张权重图。
            base_color_array: image.clone(), // 基础色数组不参与本测试。
            arm_array: image.clone(), // ARM数组不参与本测试。
            normal_array: image.clone(), // 索引模式绑定不参与布局测试。
            index_map: image,      // 索引模式绑定不参与布局测试。
            layer_table: Handle::default(), // 无 RenderApp 单元测试只需类型正确。
            layers: [TerrainSurfaceLayer::default(); MAX_TERRAIN_SURFACE_LAYERS], // 保持固定八槽布局。
            layer_count: 1,                   // 唯一默认层保持合法。
            residual_layer: 0,                // 残余权重归入唯一层。
            grass_layer: 0,                   // 草土索引落在有效层范围。
            grass_micro_normal_strength: 0.0, // 关闭无关微法线。
            grass_variation_cell_meters: 1.0, // 保持合法世界尺度。
        }); // 完成测试扩展构造。
        assert_eq!(extension.params.appearance.z, 0.0); // 作者材质默认不占用GBuffer发光通道或自定义flags。
        extension.set_dynamic_terrain_shadow_terminator_data(true); // 模拟动态场景Solari会话副本接管。
        assert_eq!(extension.params.appearance.z, 1.0); // 开启值必须精确越过Shader阈值。
        extension.set_dynamic_terrain_shadow_terminator_data(false); // 模拟会话回退或复用接口关闭。
        assert_eq!(extension.params.appearance.z, 0.0); // 关闭值必须精确恢复默认编码。
    } // 完成动态地形光追元数据模式编码测试。
} // 完成材质模块测试。
