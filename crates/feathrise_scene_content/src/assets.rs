//! 场景目录、单图清单、传送图和显式实例资产结构。

use std::collections::{BTreeMap, BTreeSet}; // 院落成员在整份场景内按稳定身份检查一致性。

use bevy::prelude::*; // Asset、Vec2和Vec3序列化桥接由纯数组避免反射耦合。
use bevy::reflect::TypePath; // 自定义资产通过TypePath注册到Bevy资产系统。
use serde::{Deserialize, Serialize}; // 所有作者文件均可由编辑器保存和运行时加载。

use crate::{
    SCENE_SCHEMA_VERSION, SceneCelestialLightingOverrides, SceneClipmapCollisionMode, SceneId,
    SceneTerrainKind, SceneVegetationAuthoring, SceneVegetationKind,
}; // 复用稳定场景ID、天体覆盖与地形、植被作者及运行时类型。

/// 新场景没有指定天空盒时仍保存的日间亮度编辑基准，单位为cd/m²。
pub const DEFAULT_SCENE_SKYBOX_BRIGHTNESS: f32 = 1_000.0; // 与Bevy官方天空盒示例的中性可见基准一致，昼夜系统会继续乘入时间与天气系数。
pub const DEFAULT_SCENE_ENVIRONMENT_DIFFUSE_INTENSITY: f32 = 300.0; // 人物和地形默认只接收较克制的天空漫反射补光，避免材质整体染成天空色。
pub const DEFAULT_SCENE_ENVIRONMENT_SPECULAR_INTENSITY: f32 = 1_000.0; // 水面与光滑材质默认保留中性GGX反射强度，斜视角仍能读到天空。

/// 游戏启动与热更新均从此目录发现可用场景。
#[derive(Asset, TypePath, Clone, Debug, Serialize, Deserialize)]
pub struct SceneCatalogAsset {
    /// 数据格式版本。
    #[serde(deserialize_with = "crate::types::deserialize_scene_schema_version")]
    pub schema_version: u32,
    /// 按作者顺序保存的场景入口。
    pub scenes: Vec<SceneCatalogEntry>,
}

impl Default for SceneCatalogAsset {
    fn default() -> Self {
        Self {
            schema_version: SCENE_SCHEMA_VERSION, // 内存中新建的空目录也必须能按当前格式保存。
            scenes: Vec::new(),
        }
    }
}

/// 场景目录中的一个稳定入口。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneCatalogEntry {
    /// 跨传送和存档稳定的场景ID。
    pub id: SceneId,
    /// UI显示名称，允许中文。
    pub display_name: String,
    /// 相对assets根的私有目录。
    pub folder: String,
    /// false表示编辑草稿，只在编辑器显示；true表示可被游戏运行时进入。
    #[serde(default)]
    pub published: bool,
    /// 场景摆放使用私有 scene.glb。缺省时沿用已存在的 JSON 工程。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub placements_glb: bool,
    /// 可选地表作者文件；旧场景未声明时继续使用自己的既有适配器。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_path: Option<String>,
    /// 可选植被作者文件；旧场景未声明时继续使用自己的既有适配器。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vegetation_path: Option<String>,
    /// 可选水域作者文件；旧场景未声明时不生成数据驱动水面。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub water_path: Option<String>,
    /// 可选区域边界作者文件；缺少字段表示当前场景没有数据驱动空气墙。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boundary_path: Option<String>,
    /// 原游戏历史扩展路径；普通编辑器原样往返，不解析或重写其角色文件。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub characters_path: Option<String>,
}

impl SceneCatalogEntry {
    /// 返回本场景清单的AssetServer相对路径。
    pub fn manifest_path(&self) -> String {
        format!(
            "{}/{}",
            self.folder.trim_end_matches('/'),
            crate::SCENE_MANIFEST_FILE_NAME
        )
    }

    /// 返回本场景传送图的AssetServer相对路径。
    pub fn portals_path(&self) -> String {
        format!(
            "{}/{}",
            self.folder.trim_end_matches('/'),
            crate::SCENE_PORTALS_FILE_NAME
        )
    }

    /// 返回本场景显式摆放实例文件的AssetServer相对路径。
    pub fn placements_path(&self) -> String {
        format!(
            "{}/{}",
            self.folder.trim_end_matches('/'),
            if self.placements_glb { crate::SCENE_PLACEMENTS_GLB_FILE_NAME } else { crate::SCENE_PLACEMENTS_FILE_NAME }
        )
    }
}

/// 单张动态场景的地表层引用、四槽权重与运行时KTX2派生资源。
///
/// 【功能设计】场景可引用任意数量的全局地表预设，不按目录顺序持久化 GPU 切片号。
/// 每个像素只保存当地贡献最高的四个场景层索引与归一化权重，因而运行时取样成本恒定，
/// 预设库和单图引用数量则没有 Rust 固定数组或产品上限。
#[derive(Asset, TypePath, Clone, Debug, Serialize, Deserialize)]
pub struct SceneSurfaceAsset {
    /// 数据格式版本。
    #[serde(deserialize_with = "crate::types::deserialize_surface_schema_version")]
    pub schema_version: u32,
    /// 文件所属场景。
    pub scene_id: SceneId,
    /// 作者可改写的无 Mip RGBA8 权重源；稀疏浅坑额外异步加载只读CPU快照。
    pub weight_source_path: String,
    /// 运行时加载的完整 Mip RGBA8_UNORM 权重 KTX2。
    pub weight_texture_path: String,
    /// 作者可改写的无 Mip RGBA32_UINT 局部场景层索引源，每像素四个小端 `u32`。
    pub index_source_path: String,
    /// 运行时加载的完整 Mip RGBA32_UINT 局部索引 KTX2。
    pub index_texture_path: String,
    /// 索引与权重作者源的共同宽高。
    pub resolution: [u32; 2],
    /// 当前场景引用的全部地表预设；数组下标是索引图中的场景局部索引。
    pub layers: Vec<SceneSurfaceLayer>,
}

/// 一个场景局部层对全局可编辑 PBR 预设的稳定引用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneSurfaceLayer {
    /// 场景内稳定层ID。
    pub id: String,
    /// 编辑器显示名称。
    pub label: String,
    /// 全局地表预设的规范 ID 或历史别名；下次保存可规范化为当前 ID。
    pub preset_id: String,
}

/// 单个场景全部可绘制植被密度层。
#[derive(Asset, TypePath, Clone, Debug, Serialize, Deserialize)]
pub struct SceneVegetationAsset {
    /// 数据格式版本。
    #[serde(deserialize_with = "crate::types::deserialize_scene_schema_version")]
    pub schema_version: u32,
    /// 文件所属场景。
    pub scene_id: SceneId,
    /// 草、麦、树木和其他植被层；每层拥有独立密度源。
    pub layers: Vec<SceneVegetationLayer>,
    /// 可选作者图层图；缺少时F12从正式层无损生成一对一作者层，运行时始终忽略该字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authoring: Option<SceneVegetationAuthoring>,
    /// 模型种植面与二维密度共用图层规则；稳定对象身份不依赖碰撞或地形容器。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_surfaces: std::sync::Arc<Vec<std::sync::Arc<crate::ModelSurfaceVegetation>>>,
}

impl Default for SceneVegetationAsset {
    fn default() -> Self {
        Self {
            schema_version: SCENE_SCHEMA_VERSION,
            scene_id: SceneId::default(),
            layers: Vec::new(),
            authoring: None,
            model_surfaces: Vec::new().into(),
        }
    }
}

/// 一层植被的模型、密度与确定性散布规格。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneVegetationLayer {
    /// 场景内稳定层ID。
    pub id: String,
    /// 编辑器显示名称。
    pub label: String,
    /// 叶片或稀疏Mesh运行时类别。
    pub kind: SceneVegetationKind,
    /// 正式内容规格名称；存在时必须复用项目已注册的风、贴地、LOD和材质实现。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_profile: Option<String>,
    /// 正式通用植被层ID；预览与运行时共享同一竞争、诊断和生命周期身份。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_layer_id: Option<u64>,
    /// Mesh层引用的正式通用原型ID；编辑器不得用普通GLB场景替代该原型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prototype_id: Option<u64>,
    /// 相对assets根的作者源GLB/GLTF；仅供资产追踪，正式profile优先决定渲染实现。
    pub asset_path: String,
    /// 作者可改写的无Mip R8密度源。
    pub density_source_path: String,
    /// 运行时加载的完整Mip R8_UNORM KTX2。
    pub density_texture_path: String,
    /// 密度源宽高。
    pub resolution: [u32; 2],
    /// 满密度候选间距；动态Mesh逐格精确散布，不会因场景大小自动放大。
    pub spacing_meters: f32,
    /// 确定性随机缩放范围。
    pub scale_range: [f32; 2],
    /// 旧冻结场景和Blade profile的兼容容量；动态场景Mesh层不读取该字段截断作者密度。
    #[serde(
        default = "unlimited_vegetation_instances",
        skip_serializing_if = "vegetation_instances_are_unlimited"
    )] // 新动态Mesh省略该字段即为作者驱动；旧JSON中的具体数字继续完整可读。
    pub max_instances: u32,
    /// 稳定散布种子。
    pub seed: u64,
}

/// 缺少旧容量字段时采用不裁剪标记；具体后端仍可对Blade或冻结场景执行自己的受检规格。
const fn unlimited_vegetation_instances() -> u32 {
    u32::MAX // 该值只用于Serde兼容表达，动态Mesh散布实现不会拿它参与数量比较。
}

/// 新动态Mesh保存时不把类型最大值写成一个看似产品硬上限的魔数。
fn vegetation_instances_are_unlimited(value: &u32) -> bool {
    *value == unlimited_vegetation_instances() // 具体旧容量继续序列化，只有作者驱动标记被省略。
}

/// 场景作者文件使用的中性天气区域；退役地域名称只在反序列化边界兼容。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")] // JSON使用稳定小写名称，未来扩展区域时不绑定Rust显示文本。
pub enum SceneWeatherRegion {
    /// 当前场景使用中性的全局天气过程，不保留原游戏地域身份。
    #[default]
    #[serde(alias = "kaifeng")] // 旧文件可读；Serialize始终只输出global，不要求批量改写场景。
    Global,
}

impl SceneWeatherRegion {
    /// 返回F12环境面板使用的中文区域名。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Global => "全局天气区",
        }
    }
}

/// 场景的中性气候身份；不再选择原游戏的季节表或局部天气倍率。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")] // 作者文件保存稳定配置身份，不直接序列化中文标签。
pub enum SceneWeatherClimate {
    /// 不附带特定地域或场景局部修正的统一气候。
    #[default]
    #[serde(alias = "kaifeng_plain", alias = "yellow_river_floodplain")]
    Uniform, // 两种退役身份仅兼容读取，保存统一写uniform，运行时不再分支执行旧倍率。
}

impl SceneWeatherClimate {
    /// 返回F12环境面板使用的中文气候名。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Uniform => "统一气候",
        }
    }
}

/// 单个场景显式登记的天气区域和局部气候。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)] // 缺省只补中性身份；未知枚举值仍由Serde严格报错，不能静默回退。
pub struct SceneWeatherDescriptor {
    /// 决定天气连续状态是否与其他场景共享。
    pub region: SceneWeatherRegion,
    /// 当前只登记统一气候，不携带原游戏地域的季节表或场景局部倍率。
    pub climate: SceneWeatherClimate,
}

impl SceneWeatherDescriptor {
    /// 校验中性身份；旧文件名称已经在读取边界规范化，运行时不再保留地域分支。
    pub fn validate(self) -> Result<(), String> {
        match (self.region, self.climate) {
            (SceneWeatherRegion::Global, SceneWeatherClimate::Uniform) => Ok(()),
        }
    }
}

/// 单个场景可见天空的互斥来源；环境 IBL 在两种模式下都可继续使用。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")] // 作者 JSON 使用稳定小写身份，避免把中文界面文案写入数据协议。
pub enum SceneSkyMode {
    /// Bevy 原生大气与独立动态云共同生成可见天空。
    #[default]
    Dynamic, // 旧场景缺少字段时采用当前动态天空行为，保证向后兼容。
    /// 主 KTX2 Cube 直接作为可见天空，动态大气、云层与天体圆盘停用。
    Skybox, // 离线 IBL 两件套仍与可见 Cube 一起按同一作者身份加载。
}

impl SceneSkyMode {
    /// 返回 F12 摘要与诊断报告使用的稳定中文名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Dynamic => "动态天空", // 原生大气和动态云模式。
            Self::Skybox => "天空盒",    // 离线可见 Cube 模式。
        }
    }
}

/// 单个场景可选的视觉环境参数；天气配置选择正式区域/气候，不复制天气状态机。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "SceneEnvironmentDescriptorSerde")] // 读取时把旧单一IBL强度复制到两个新通道；保存只输出新字段。
pub struct SceneEnvironmentDescriptor {
    /// 可见天空来源；旧场景缺少字段时默认动态天空。
    pub sky_mode: SceneSkyMode,
    /// 相对assets根的KTX2立方体纹理；动态模式只派生 IBL，天空盒模式还显示主 Cube。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skybox_path: Option<String>,
    /// 天空盒模式在完整日照下的作者亮度；动态模式只保留该值供模式切换。
    pub skybox_brightness: f32,
    /// 完整日照下的PBR天空漫反射补光强度，主要影响人物、地形和粗糙材质的天空染色。
    pub environment_diffuse_intensity: f32,
    /// 完整日照下的PBR天空镜面反射强度，主要影响水面与光滑材质读取天空的清晰程度。
    pub environment_specular_intensity: f32,
    /// 绕世界Y轴旋转的角度，编辑器以十五度步进但数据格式允许任意有限值。
    pub skybox_yaw_degrees: f32,
    /// 场景所属正式天气区与气候配置；当前天气本身仍由天气系统和存档维护。
    pub weather: SceneWeatherDescriptor,
    /// 动态场景对全局太阳/月光基础值的逐属性覆盖；空结构表示完整跟随全局。
    #[serde(
        default,
        skip_serializing_if = "SceneCelestialLightingOverrides::is_empty"
    )]
    pub celestial_overrides: SceneCelestialLightingOverrides,
}

impl Default for SceneEnvironmentDescriptor {
    fn default() -> Self {
        Self {
            sky_mode: SceneSkyMode::Dynamic, // 新旧场景都先使用可适配天气的动态天空。
            skybox_path: None,             // 新建场景不增加离线 Cube 或 IBL 依赖。
            skybox_brightness: DEFAULT_SCENE_SKYBOX_BRIGHTNESS, // 保留可直接启用的中性日间亮度。
            environment_diffuse_intensity: DEFAULT_SCENE_ENVIRONMENT_DIFFUSE_INTENSITY, // 新天空默认使用已验收的克制人物补光。
            environment_specular_intensity: DEFAULT_SCENE_ENVIRONMENT_SPECULAR_INTENSITY, // 新天空默认保留完整水面反射。
            skybox_yaw_degrees: 0.0, // 默认不改变立方体资产自身方向。
            weather: SceneWeatherDescriptor::default(), // 新场景只登记global/uniform中性身份。
            celestial_overrides: SceneCelestialLightingOverrides::default(), // 新旧场景默认逐字段跟随全局配置。
        }
    }
}

impl SceneEnvironmentDescriptor {
    /// 校验可由AssetServer安全加载并可进入渲染Uniform的天空参数。
    pub fn validate(&self) -> Result<(), String> {
        if self.sky_mode == SceneSkyMode::Skybox && self.skybox_path.is_none() {
            return Err("天空盒模式必须先设置主KTX2资源；可清除资源并自动切回动态天空".to_owned()); // 禁止保存必然只能回退的半套天空盒配置。
        }
        if !self.skybox_brightness.is_finite() || self.skybox_brightness < 0.0 {
            return Err("天空盒亮度必须是有限非负数".to_owned()); // 动态模式只保存该值，天空盒模式会把它写入可见 Cube。
        }
        if !self.environment_diffuse_intensity.is_finite()
            || self.environment_diffuse_intensity < 0.0
        {
            return Err("天空补光强度必须是有限非负数".to_owned()); // 漫反射比例不得由NaN、无穷或负值污染派生图。
        }
        if !self.environment_specular_intensity.is_finite()
            || self.environment_specular_intensity < 0.0
        {
            return Err("天空反射强度必须是有限非负数".to_owned()); // 镜面载体Uniform同样拒绝NaN、无穷和负值。
        }
        if !self.skybox_yaw_degrees.is_finite() {
            return Err("天空盒旋转角必须是有限数".to_owned()); // 非有限四元数会污染相机视图。
        }
        if let Some(path) = self.skybox_path.as_deref() {
            let lower = path.to_ascii_lowercase(); // 扩展名比较不限制作者文件名大小写。
            if path.is_empty()
                || path.starts_with('/')
                || path.contains("..")
                || path.contains('\\')
                || !lower.ends_with(".ktx2")
                || lower.ends_with("_diffuse.ktx2")
                || lower.ends_with("_specular.ktx2")
            {
                return Err("天空资源必须是assets内使用正斜杠的无派生后缀KTX2路径，不能直接选择_diffuse或_specular派生图".to_owned()); // 主路径同时作为 IBL 身份，并可在天空盒模式显示。
            }
        }
        self.weather.validate()?; // 区域与气候必须来自同一套已登记正式配置。
        self.celestial_overrides.validate()?; // 已声明的场景天体覆盖必须可安全进入光照与 Solari。
        Ok(()) // 图像是否确为Cube视图仍在资产加载完成后由统一天空运行时验证。
    }

    /// 返回适合构造稳定Y轴四元数的零至三百六十度角。
    pub fn normalized_skybox_yaw_degrees(&self) -> f32 {
        self.skybox_yaw_degrees.rem_euclid(360.0) // 保存允许负角，运行时统一折回等价旋转范围。
    }
}

/// 场景环境的兼容读取形状；旧单字段只在反序列化边界存在，不重新写回作者文件。
#[derive(Deserialize)]
#[serde(default)] // 任意历史版本缺少新增字段时都由明确默认值或旧字段迁移补齐。
struct SceneEnvironmentDescriptorSerde {
    sky_mode: SceneSkyMode,        // 旧场景缺失时由枚举默认值迁移为动态天空。
    skybox_path: Option<String>, // 旧场景没有路径时继续使用清屏色。
    skybox_brightness: f32,      // 天空盒模式实际使用该值，动态模式也无损保留以便切换。
    environment_diffuse_intensity: Option<f32>, // 新格式可独立声明天空补光。
    environment_specular_intensity: Option<f32>, // 新格式可独立声明天空反射。
    environment_light_intensity: Option<f32>, // 旧格式的单一IBL强度仅供一次兼容迁移。
    skybox_yaw_degrees: f32,     // 方向字段保持原兼容行为。
    weather: SceneWeatherDescriptor, // 缺省与退役身份都只映射到中性天气配置。
    celestial_overrides: SceneCelestialLightingOverrides, // 旧场景缺失字段时完整跟随全局。
}

impl Default for SceneEnvironmentDescriptorSerde {
    fn default() -> Self {
        Self {
            sky_mode: SceneSkyMode::Dynamic, // 兼容所有尚未声明可见天空来源的旧清单。
            skybox_path: None,             // 与公开环境默认值保持一致。
            skybox_brightness: DEFAULT_SCENE_SKYBOX_BRIGHTNESS, // 旧环境缺少亮度时使用中性背景。
            environment_diffuse_intensity: None, // 缺少新字段时稍后优先读取旧单字段。
            environment_specular_intensity: None, // 缺少新字段时稍后优先读取旧单字段。
            environment_light_intensity: None, // 完全缺省时采用两个通道各自的新默认值。
            skybox_yaw_degrees: 0.0,       // 旧场景默认不旋转天空。
            weather: SceneWeatherDescriptor::default(), // 旧场景缺字段时同样采用中性身份。
            celestial_overrides: SceneCelestialLightingOverrides::default(), // 不要求旧场景升级或写回新增字段。
        }
    }
}

impl From<SceneEnvironmentDescriptorSerde> for SceneEnvironmentDescriptor {
    fn from(source: SceneEnvironmentDescriptorSerde) -> Self {
        let legacy = source.environment_light_intensity; // 同一个旧值可被两个新通道分别兜底。
        Self {
            sky_mode: source.sky_mode, // 显式模式原样采用，缺失时已在兼容形状中补为动态。
            skybox_path: source.skybox_path, // 路径无需迁移。
            skybox_brightness: source.skybox_brightness, // 天空盒亮度保持独立作者值，动态模式也无损保留以便切换。
            environment_diffuse_intensity: source
                .environment_diffuse_intensity
                .or(legacy)
                .unwrap_or(DEFAULT_SCENE_ENVIRONMENT_DIFFUSE_INTENSITY), // 旧显式值优先于新默认，避免旧场景外观突变。
            environment_specular_intensity: source
                .environment_specular_intensity
                .or(legacy)
                .unwrap_or(DEFAULT_SCENE_ENVIRONMENT_SPECULAR_INTENSITY), // 同一旧值复制到镜面通道，完整保存历史语义。
            skybox_yaw_degrees: source.skybox_yaw_degrees, // 旋转不参与IBL拆分。
            weather: source.weather, // 兼容名称已由枚举读取规范化，不保留第二套旧身份。
            celestial_overrides: source.celestial_overrides, // 显式覆盖保留，缺失时继续为空继承结构。
        }
    }
}

/// 单个场景的地形、范围和运行时绑定清单。
#[derive(Asset, TypePath, Clone, Debug, Serialize, Deserialize)]
pub struct SceneManifestAsset {
    /// 数据格式版本。
    #[serde(deserialize_with = "crate::types::deserialize_scene_schema_version")]
    pub schema_version: u32,
    /// 必须与目录条目一致的场景ID。
    pub id: SceneId,
    /// 场景显示名称。
    pub display_name: String,
    /// 可选作者备注；旧清单缺省视为空，空备注不增加磁盘字段。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// 场景运行类型，当前格式统一使用 data_driven。
    pub runtime_scene: String,
    /// 旧主地形X/Z采样范围最小端点；不是场景容器的编辑边界。
    pub world_min: [f32; 2],
    /// 旧主地形采样最大端点；新空容器保留单位占位值，实际范围由内容对象提供。
    pub world_max: [f32; 2],
    /// 没有传送目标时的安全出生点。
    pub default_spawn: [f32; 3],
    /// 可选天空盒及其可见亮度、漫反射补光、镜面反射和方向；旧清单缺省时保持无天空盒。
    #[serde(default)]
    pub environment: SceneEnvironmentDescriptor,
    /// 旧格式的单个主地形；新对象场景使用空描述，普通旧图保存保留原格式。
    #[serde(default = "SceneTerrainDescriptor::empty", skip_serializing_if = "SceneTerrainDescriptor::is_empty")]
    pub terrain: SceneTerrainDescriptor,
    /// None保留旧单地形语义，Some空列表表示真正空白的场景，不能回退为默认平地。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terrains: Option<std::sync::Arc<[crate::SceneTerrainObject]>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // 缺少字段兼容无积水，空层保存时不输出占位描述。
    pub puddles: Option<crate::ScenePuddleDescriptor>, // 可选独立R8潜力描述，不承载静水水位或植被密度。
    /// 资产浏览器默认展示的场景私有与共享目录。
    #[serde(default)]
    pub asset_roots: Vec<String>,
    /// 完整子场景引用；省略表示无引用，内部内容继续由来源场景独立保存。
    #[serde(default, skip_serializing_if = "crate::scene_instances::no_instances")]
    pub subscenes: std::sync::Arc<[crate::SceneInstancePlacement]>,
    /// 场景级绘制组只关联各网格自己的作者层；不合并或搬迁R8像素。
    #[serde(default, skip_serializing_if = "crate::vegetation_paint_groups::is_empty")]
    pub vegetation_paint_groups: std::sync::Arc<[crate::SceneVegetationPaintGroup]>,
}

impl SceneManifestAsset {
    /// 作者、加载及运行入口共享同一类型判断。
    pub fn is_data_driven(&self) -> bool {
        crate::SceneRuntimeKind::parse(&self.runtime_scene) == Some(crate::SceneRuntimeKind::DataDriven)
    }

    /// 返回受检世界范围；损坏数据不进入视口和笔刷换算。
    pub fn world_bounds(&self) -> Option<Rect> {
        let min = Vec2::from_array(self.world_min);
        let max = Vec2::from_array(self.world_max);
        (min.is_finite() && max.is_finite() && min.cmplt(max).all())
            .then_some(Rect::from_corners(min, max))
    }

    /// 返回默认出生点。
    pub fn default_spawn_vec3(&self) -> Vec3 {
        Vec3::from_array(self.default_spawn)
    }
}

/// 地形源、分辨率和构建产物描述。
#[derive(Clone, Debug, PartialEq, Serialize)] // 自定义读取集中区分碰撞迁移与显示LOD兼容自动模式。
pub struct SceneTerrainDescriptor {
    /// 单网格投影的派生约束；只保存对象上的互指作者关系，不把派生数据重复写进源描述。
    #[serde(skip_serializing)]
    pub resolved_stitches: std::sync::Arc<[crate::ClipmapEdgeProfile]>,
    /// GLTF保留已有三维地形来源；可编辑高度地形统一由SparseClipmap提供。
    pub kind: SceneTerrainKind,
    /// 只兼容读取旧模式名；运行时唯一使用多层共享碰撞。
    #[serde(default, skip_serializing)] // 仅兼容旧文件读取；唯一新路线无需再保存模式字段。
    pub collision_mode: SceneClipmapCollisionMode,
    /// SparseClipmap唯一碰撞配置；缺字段统一初始化默认列表，显式空列表拒绝加载。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collision_lods: Vec<crate::SceneCollisionLod>, // 稀疏地形唯一共享碰撞配置；GLB不消费此列表。
    /// SparseClipmap显式显示LOD；None保留旧64半格自动算法且普通保存不得迁移。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // 只有作者主动编辑或新建场景才输出列表。
    pub display_lods: Option<Vec<crate::SceneTerrainDisplayLod>>, // Some列表的末层必须延伸至场景边界。
    /// 旧协议只读兼容字段，运行时与编辑器均不消费。
    #[serde(skip_serializing)] // 旧字段只兼容读；下一保存迁移为唯一collision_lods列表。
    pub player_collision_preload_radius_meters: f32,
    /// 旧角色预载字段仅兼容反序列化，不用于推导新默认值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(skip_serializing)] // 旧字段只兼容读；下一保存迁移为唯一collision_lods列表。
    pub role_collision_preload_radius_meters: Option<f32>,
    /// 旧玩家块格数只读兼容；新分块大小由规划器统一决定。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(skip_serializing)] // 旧字段只兼容读；下一保存迁移为唯一collision_lods列表。
    pub player_collision_chunk_cells: Option<u32>,
    /// 旧角色块格数只读兼容；新方案不按实体身份分块。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(skip_serializing)] // 旧字段只兼容读；下一保存迁移为唯一collision_lods列表。
    pub role_collision_chunk_cells: Option<u32>,
    /// 旧玩家联动跨度只读兼容，不再影响距离或精度。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(skip_serializing)] // 旧字段只兼容读；下一保存迁移为唯一collision_lods列表。
    pub player_collision_span_meters: Option<f32>,
    /// 旧角色联动跨度只读兼容，下次保存移除。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(skip_serializing)] // 旧字段只兼容读；下一保存迁移为唯一collision_lods列表。
    pub role_collision_span_meters: Option<f32>,
    /// 旧地形的GLB作者源；SparseClipmap场景留空，三维洞穴另存独立网格。
    pub glb_path: String,
    /// 旧高度场的R16路径；SparseClipmap场景指向稀疏作者清单。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_path: Option<String>,
    /// 静态/专用场景的GPU四角高度KTX2；动态GLB场景必须为空并在内存派生。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_texture_path: Option<String>,
    /// 作者逻辑格点数，端点对应world_min/world_max；稀疏场景不得据此分配整图数组。
    pub resolution: [u32; 2],
    /// 原始零值对应的世界Y。
    pub base_height: f32,
    /// 16位最大值对应base_height以上的米数。
    pub height_scale: f32,
}

/// 预载半径以米配置；单块格数使用独立场景字段。
pub const DEFAULT_PLAYER_COLLISION_PRELOAD_RADIUS_METERS: f32 = 16.0;
pub const MIN_PLAYER_COLLISION_PRELOAD_RADIUS_METERS: f32 = 1.0;
pub const MAX_PLAYER_COLLISION_PRELOAD_RADIUS_METERS: f32 = 32.0;
/// 新Clipmap场景的粗级预载半径与角色卸载距离解耦；旧清单的None仍走原有卸载距离。
pub const DEFAULT_ROLE_COLLISION_PRELOAD_RADIUS_METERS: f32 = 160.0;
pub const MIN_ROLE_COLLISION_PRELOAD_RADIUS_METERS: f32 = 1.0;
pub const MAX_ROLE_COLLISION_PRELOAD_RADIUS_METERS: f32 = 512.0;
pub const MIN_COLLISION_CHUNK_CELLS: u32 = 1;
pub const MAX_COLLISION_CHUNK_CELLS: u32 = 32;

/// 单个场景的传送点集合；目标通过场景ID和传送点ID组成稳定引用。
#[derive(Asset, TypePath, Clone, Debug, Serialize, Deserialize)]
pub struct ScenePortalsAsset {
    /// 数据格式版本。
    #[serde(deserialize_with = "crate::types::deserialize_scene_schema_version")]
    pub schema_version: u32,
    /// 文件所属场景。
    pub scene_id: SceneId,
    /// 当前场景全部传送点。
    pub portals: Vec<ScenePortalRecord>,
}

impl Default for ScenePortalsAsset {
    fn default() -> Self {
        Self {
            schema_version: SCENE_SCHEMA_VERSION,
            scene_id: SceneId::default(),
            portals: Vec::new(),
        }
    }
}

/// 一个可摆放、可作为其他传送点目标的传送点。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScenePortalRecord {
    /// 承托地形的源局部姿态；缺省保持旧世界固定点，不根据XZ重新挑选地形。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terrain_binding: Option<crate::SceneTerrainBinding>,
    /// 安全到达点独立保存关联，可与触发点处于不同地形或楼层。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrival_terrain_binding: Option<crate::SceneTerrainBinding>,
    /// 当前场景内唯一的稳定ID。
    pub id: String,
    /// 传送点作者名称；未自定义时保存创建时的默认名称快照，显示阶段仍按目标场景派生最新默认名。
    pub label: String,
    /// 作者是否显式改过名称；旧JSON默认视为已命名，但旧“传送点_N”和当前默认名仍由显示层兼容识别。
    #[serde(default = "default_portal_label_customized")]
    pub label_customized: bool,
    /// 传送触发物在当前场景的坐标。
    pub position: [f32; 3],
    /// 其他场景传入本点时的安全落点，通常位于触发范围外。
    pub arrival: [f32; 3],
    /// 目标场景与目标传送点引用。
    pub target: ScenePortalTarget,
    /// 路标颜色，使用线性无关的sRGBA浮点值。
    pub color_srgba: [f32; 4],
    /// 顺时针路标朝向角兼容快照；有主区域边界时编辑器与运行时按最近线段点自动覆盖。
    pub arrow_degrees: f32,
}

/// 旧场景可能已经保存“返回开封城郊”等真实作者名称，缺少新字段时必须保留而不能全部自动改名。
fn default_portal_label_customized() -> bool {
    true // 新建记录会显式写false；只有旧Serde输入走该兼容默认值。
}

/// 传送目标的稳定跨文件引用；没有传送点ID时使用目标场景清单的安全默认出生点。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenePortalTarget {
    /// 目标场景ID。
    pub scene_id: SceneId,
    /// 目标场景内的传送点ID；`None`明确表示使用清单`default_spawn`，并兼容旧JSON中的字符串字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portal_id: Option<String>,
}

impl ScenePortalTarget {
    /// 构造指向目标场景某个传送点安全到达坐标的稳定引用。
    pub fn portal(scene_id: SceneId, portal_id: impl Into<String>) -> Self {
        Self {
            scene_id,                            // 跨文件目标始终由稳定SceneId定位。
            portal_id: Some(portal_id.into()), // 传送点ID存在时由运行时读取该记录的arrival。
        }
    }

    /// 构造指向目标场景清单安全默认出生点的引用。
    pub fn default_spawn(scene_id: SceneId) -> Self {
        Self {
            scene_id,          // 默认出生点仍明确属于唯一目标场景。
            portal_id: None, // 缺少传送点ID是正式语义，不使用保留字符串伪造虚拟传送点。
        }
    }

    /// 返回目标是否使用场景清单中的安全默认出生点。
    pub fn uses_default_spawn(&self) -> bool {
        self.portal_id.is_none() // UI与运行时共享同一显式判定，避免各自约定魔法ID。
    }
}

/// 单个场景显式摆放的场景实例资产。
#[derive(Asset, TypePath, Clone, Debug, Serialize, Deserialize)]
pub struct ScenePlacementsAsset {
    /// 数据格式版本。
    #[serde(deserialize_with = "crate::types::deserialize_scene_schema_version")]
    pub schema_version: u32,
    /// 文件所属场景。
    pub scene_id: SceneId,
    /// 树木、建筑和其他模型实例。
    pub placements: Vec<ScenePlacementRecord>,
}

impl Default for ScenePlacementsAsset {
    fn default() -> Self {
        Self {
            schema_version: SCENE_SCHEMA_VERSION,
            scene_id: SceneId::default(),
            placements: Vec::new(),
        }
    }
}

/// 显式摆放实例的世界高度解释方式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")] // JSON只保存稳定英文枚举，中文名称由作者界面统一提供。
pub enum ScenePlacementHeightMode {
    /// 保持记录中的世界Y，不响应后续R16地形变化。
    #[default]
    FixedHeight, // 旧JSON缺少字段时必须保持历史世界高度，不能把桥梁或建筑自动压到地表。
    /// 以当前R16地表高度加作者离地偏移派生世界Y。
    FollowTerrain, // 新放置资产显式选择此模式，地形笔划完成后重新采样。
}

impl ScenePlacementHeightMode {
    /// 返回当前模式是否应随权威R16地形重新计算世界Y。
    pub const fn follows_terrain(self) -> bool {
        matches!(self, Self::FollowTerrain) // 调用方不再重复匹配数据格式枚举。
    }
}

/// 固定高度是兼容默认值，保存时省略即可保持旧文件简洁。
fn placement_height_mode_is_fixed(value: &ScenePlacementHeightMode) -> bool {
    *value == ScenePlacementHeightMode::FixedHeight // Serde只为真正跟随型实例写出模式字段。
}

/// 零离地偏移是绝大多数贴地实例的默认值，保存时不制造无意义字段。
fn placement_terrain_offset_is_zero(value: &f32) -> bool {
    *value == 0.0 // 正负零都具有相同作者语义，可安全省略。
}

/// 一条显式摆放记录采用的GLB/GLTF子资产或内置参数化几何。
///
/// 【功能设计】旧场景与完整模型继续省略本字段并默认实例化`Scene0`；作者在F12预览中
/// 明确选择单个primitive后，则把网格编号和当时的原关联材质编号一并写入场景数据，
/// 避免保存、撤销或重新进入场景后退化成完整场景或统一白材质。内置立方体保存整数规格，
/// 不伪造磁盘模型路径，仍沿用同一稳定实例、变换、历史和保存证明。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScenePlacementAssetTarget {
    /// 实例化源文件的第零个场景；与历史摆放协议完全一致。
    #[default]
    Scene0,
    /// 只实例化一个真实`MeshN/PrimitiveN`及其原关联标准材质。
    Primitive {
        /// GLTF根中父网格的稳定数字标签。
        mesh: u32,
        /// 父网格内部primitive的稳定数字标签。
        primitive: u32,
        /// `Some`使用`MaterialN/std`，`None`使用`DefaultMaterial/std`。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        material: Option<u32>,
        /// GLB 节点可显式启用与可见网格同源的静态碰撞。
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        collision: bool,
    },
    /// 内置参数化立方体；整数规格使撤销、缓存和保存证明具有精确身份，不依赖外部GLB。
    Cuboid {
        /// 本地XYZ全尺寸，单位毫米；缩放仍由场景实例单独保存。
        size_mm: [u32; 3],
        /// 不透明基础颜色，按sRGB字节保存。
        color_srgb: [u8; 3],
        /// PBR感知粗糙度的千分值，合法范围0..=1000。
        roughness_milli: u16,
        /// 是否生成与真实尺寸一致的静态盒碰撞。
        collision: bool,
    },
    /// 固定八顶点的可编辑立方体；只改变点坐标，边、四边面及对角线由公共拓扑固定。
    EditableCuboid {
        /// 本地XYZ坐标，每格半毫米；精确保留奇数毫米尺寸和中心，共享顶点不会拉开相邻面。
        vertices_half_mm: [[i32; 3]; 8],
        color_srgb: [u8; 3],
        roughness_milli: u16,
        /// 使用与可视网格同源的真实十二三角碰撞，而非变形后的包围盒。
        collision: bool,
    },
    /// 有界三角/四边/多边面编辑网格；拓扑操作只修改当前对象，不生成外部资产。
    EditableMesh {
        geometry: crate::EditableMeshGeometry,
        color_srgb: [u8; 3],
        roughness_milli: u16,
        collision: bool,
    },
}

impl ScenePlacementAssetTarget {
    /// 与Blender默认尺寸一致的两米灰色白模，默认带静态盒碰撞。
    pub const DEFAULT_CUBOID: Self = Self::Cuboid {
        size_mm: [2_000; 3],
        color_srgb: [160; 3],
        roughness_milli: 700,
        collision: true,
    };

    /// 内置几何无需磁盘模型路径或AssetServer依赖。
    pub const fn is_builtin(self) -> bool {
        matches!(self, Self::Cuboid { .. } | Self::EditableCuboid { .. } | Self::EditableMesh { .. })
    }

    /// GLB 节点的碰撞使用实际网格，跟随 Blender 的形状变化。
    pub const fn has_primitive_collision(self) -> bool {
        matches!(self, Self::Primitive { collision: true, .. })
    }

    /// 固定大小参数在作者预览、保存及正式采用前共用同一合法范围。
    pub fn validate(self) -> Result<(), String> {
        if let Self::Cuboid { size_mm, roughness_milli, .. } = self {
            if size_mm.iter().any(|size| !(1..=10_000_000).contains(size)) {
                return Err("立方体各轴尺寸必须在0.001～10000米之间".into());
            }
            if roughness_milli > 1_000 {
                return Err("立方体粗糙度必须在0～1之间".into());
            }
        }
        if let Self::EditableCuboid { vertices_half_mm, roughness_milli, .. } = self {
            if roughness_milli > 1_000 {
                return Err("可编辑网格粗糙度必须在0～1之间".into());
            }
            crate::editable_cuboid::validate_vertices(vertices_half_mm)?;
        }
        if let Self::EditableMesh { geometry, roughness_milli, .. } = self {
            if roughness_milli > 1_000 { return Err("可编辑网格粗糙度必须在0～1之间".into()); }
            geometry.validate()?;
        }
        Ok(())
    }

    /// 默认Scene0不写入JSON，保持既有场景文件简洁且无迁移噪音。
    pub const fn is_scene0(&self) -> bool {
        matches!(self, Self::Scene0)
    }

    /// 返回当前子资产的精确Bevy Mesh引用；完整场景没有单一网格引用。
    pub fn mesh_reference(&self, source: &str) -> Option<String> {
        match *self {
            Self::Scene0 | Self::Cuboid { .. } | Self::EditableCuboid { .. } | Self::EditableMesh { .. } => None,
            Self::Primitive {
                mesh, primitive, ..
            } => Some(format!("{source}#Mesh{mesh}/Primitive{primitive}")),
        }
    }

    /// 返回当前子资产原关联的Bevy StandardMaterial引用。
    pub fn material_reference(&self, source: &str) -> Option<String> {
        match *self {
            Self::Scene0 | Self::Cuboid { .. } | Self::EditableCuboid { .. } | Self::EditableMesh { .. } => None,
            Self::Primitive {
                material: Some(material),
                ..
            } => Some(format!("{source}#Material{material}/std")),
            Self::Primitive { material: None, .. } => Some(format!("{source}#DefaultMaterial/std")),
        }
    }

    /// 返回作者界面可直接辨认的目标短标签。
    pub fn label(self) -> String {
        match self {
            Self::Scene0 => "Scene0".to_owned(),
            Self::Cuboid { .. } => "立方体".to_owned(),
            Self::EditableCuboid { .. } | Self::EditableMesh { .. } => "可编辑网格".to_owned(),
            Self::Primitive {
                mesh, primitive, ..
            } => format!("Mesh{mesh}/Primitive{primitive}"),
        }
    }
}

/// 一条摆放记录所属的院落副本及其原始成员身份。
///
/// 【功能设计】院落仍由独立的普通摆放记录渲染和碰撞；成员身份只标识同一个印章副本，
/// 原点和朝向冻结落印时的整体变换，使单独调整某栋房后仍能找到原组合且不改写其他副本。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScenePlacementCourtyardMembership {
    /// 当前场景内本次落印副本的稳定 ID。
    pub instance_id: String,
    /// 全局院落预设的稳定 ID。
    pub preset_id: String,
    /// 预设内部成员的稳定 ID，不依赖摆放数组顺序。
    pub member_id: String,
    /// 本次落印的世界原点；单个成员后续移动时仍保留原锚点。
    pub origin: [f32; 3],
    /// 本次落印绕世界 Y 轴的旋转角度。
    pub yaw_degrees: f32,
}

impl ScenePlacementCourtyardMembership {
    /// 院落三个身份均使用有界、无路径含义的稳定字符串。
    pub fn validate(&self) -> Result<(), String> {
        if !crate::valid_courtyard_preset_id(&self.instance_id)
            || !crate::valid_courtyard_preset_id(&self.preset_id)
            || !crate::valid_courtyard_preset_id(&self.member_id)
        {
            return Err("院落副本、预设或成员 ID 无效".to_owned()); // 场景实例与全局院落目录共用128字节稳定身份规则。
        }
        if !self.origin.iter().all(|value| value.is_finite()) || !self.yaw_degrees.is_finite() {
            return Err("院落原点或朝向不是有限数值".to_owned()); // NaN不能成为组比较、保存或场景变换的稳定锚点。
        }
        Ok(()) // 成员归属仍由整份场景进一步确认唯一和一致。
    }
}

impl ScenePlacementsAsset {
    /// 保存前确认同一院落副本的预设和整体锚点一致，且成员身份没有重复。
    pub fn validate_courtyards(&self) -> Result<(), String> {
        let mut instances = BTreeMap::<&str, &ScenePlacementCourtyardMembership>::new(); // 每个副本只保留首个成员的共享元数据引用。
        let mut members = BTreeSet::<(&str, &str)>::new(); // 同一副本中一个预设成员只能出现一次。
        for record in &self.placements {
            let Some(courtyard) = record.courtyard.as_ref() else {
                continue; // 全部旧场景与独立摆放没有组合语义。
            };
            courtyard
                .validate()
                .map_err(|error| format!("实例{}的{error}", record.id))?; // 单项坏数据在访问集合前失败。
            if let Some(first) = instances.get(courtyard.instance_id.as_str()) {
                if first.preset_id != courtyard.preset_id
                    || first.origin != courtyard.origin
                    || first.yaw_degrees != courtyard.yaw_degrees
                {
                    return Err(format!(
                        "院落副本{}的预设或整体锚点不一致",
                        courtyard.instance_id
                    )); // 同一副本必须共享一次落印的身份。
                }
            } else {
                instances.insert(courtyard.instance_id.as_str(), courtyard); // 首个合法成员冻结共享基准。
            }
            if !members.insert((courtyard.instance_id.as_str(), courtyard.member_id.as_str())) {
                return Err(format!(
                    "院落副本{}的成员{}重复",
                    courtyard.instance_id, courtyard.member_id
                )); // 成员身份不能因复制单体而悄悄重用。
            }
        }
        Ok(()) // 无组合记录时保留历史场景的原始语义。
    }
}

/// 编辑器直接摆放的一个稳定场景实例。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] // 院落整组采用前可按完整作者快照复核原成员未变化。
pub struct ScenePlacementRecord {
    /// 实例显示与碰撞的独立简化策略；不属于编辑器显隐或锁定元数据。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh_lod: Option<crate::MeshLodPolicy>,
    /// 普通摆放与建筑的完整地形关联；地形载体不使用本字段，避免重复变换。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terrain_binding: Option<crate::SceneTerrainBinding>,
    /// 当前场景中这一份摆放的管理信息；旧文件缺省、默认值不新增磁盘字段。
    #[serde(default, skip_serializing_if = "crate::ScenePlacementEditorMetadata::is_empty")]
    pub editor: std::sync::Arc<crate::ScenePlacementEditorMetadata>,
    /// 可选静态三维地形；缺字段保持普通摆放，洞口与实例共用撤销和保存身份。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terrain_patch: Option<crate::ScenePlacementTerrainPatch>,
    /// 实例稳定ID，用于撤销、热更和存档差分。
    pub id: String,
    /// 相对assets根的GLB或GLTF路径；内置几何使用空字符串。
    #[serde(default)]
    pub asset_path: String,
    /// 文件内具体摆放目标；旧记录缺失时兼容为完整`Scene0`。
    #[serde(default, skip_serializing_if = "ScenePlacementAssetTarget::is_scene0")]
    pub asset_target: ScenePlacementAssetTarget,
    /// 可选建筑预设规范 ID；模型路径仍独立保存，目录损坏时实例不会消失。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub building_preset_id: Option<String>, // 普通资产和全部旧场景缺少该字段时继续按 None 读取。
    /// 可选院落副本身份；旧场景和独立摆放保持省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub courtyard: Option<ScenePlacementCourtyardMembership>, // 单体复制会清除归属，整组落印才赋予新副本身份。
    /// 世界平移。
    pub translation: [f32; 3],
    /// 世界Y是固定值还是由R16地形与离地偏移共同派生。
    #[serde(default, skip_serializing_if = "placement_height_mode_is_fixed")]
    pub height_mode: ScenePlacementHeightMode, // 缺失字段按固定高度读取，保护所有历史摆放结果。
    /// 跟随地形模式下，实例原点相对采样地表的垂直偏移，单位为米。
    #[serde(default, skip_serializing_if = "placement_terrain_offset_is_zero")]
    pub terrain_offset: f32, // 固定高度模式忽略此值；作者切换模式时会规范化为零。
    /// Y轴旋转角度。
    pub yaw_degrees: f32,
    /// 三轴缩放。
    pub scale: [f32; 3],
    /// 由Blender完成拓扑后，在当前实例内按节点路径与网格名赋予的整网格水体覆盖。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    // 旧摆放JSON缺少字段时保持普通模型；空覆盖也不污染新保存文件。
    pub water_surfaces: Vec<crate::placement_water::ScenePlacementWaterSurface>, // 每个节点路径与网格名组合最多出现一次，正式采用前由公共纯校验确认。
}
