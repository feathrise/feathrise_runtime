//! 开发者植被预设目录的数据格式、稳定身份和纯校验。
//!
//! 【功能设计】场景只保存稳定预设 ID；GPU 批次和局部四槽都是运行时派生值，
//! F12 则按目录顺序连续展示全部预设，因而追加预设不改 Rust 枚举，目录重排也不破坏场景引用。

use bevy::prelude::*; // Asset、TypePath 与 Vec4 纯值类型。
use serde::{Deserialize, Serialize}; // 预设由开发者维护的 JSON 热加载。

use crate::SceneVegetationKind; // 运行时后端类别继续复用场景正式枚举。

mod registry; // 已校验目录查询、稳定身份与场景输出派生。
mod validation; // 结构、数值、路径和跨项身份的原子校验。

pub use registry::{ResolvedVegetationPresetCatalog, stable_vegetation_identity};

/// 当前开发者植被预设目录格式版本。
pub const VEGETATION_PRESET_SCHEMA_VERSION: u32 = 2; // 第二版只接受完整 Blade/Mesh 数据，不再反序列化注册适配器。

/// Blade 预设可声明的最大 LOD 档数；GPU 紧凑实例用三位几何档号，最多八档。
pub const MAX_BLADE_PRESET_LODS: usize = 8;

/// Blade 预设至少保留一档可绘制几何，与 Mesh 至少一档对齐。
pub const MIN_BLADE_PRESET_LODS: usize = 1;

/// 开发者维护的全部植被预设目录资产。
#[derive(Asset, TypePath, Clone, Debug, Default, Serialize, Deserialize)]
pub struct VegetationPresetCatalogAsset {
    /// 目录格式版本。
    pub schema_version: u32,
    /// 按开发者声明顺序展示的全部预设。
    pub presets: Vec<VegetationPresetDescriptor>,
}

/// 一个可被场景引用和 F12 创建正式输出的开发者预设。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VegetationPresetDescriptor {
    /// 跨场景稳定、永不复用的技术 ID。
    pub id: String,
    /// 重命名前的历史 ID；旧场景可继续解析但新保存只写 `id`。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// F12 面向作者显示的中文或本地化名称。
    pub label: String,
    /// 新场景正式输出使用的稳定层 ID 基名。
    pub output_id: String,
    /// 作者资产追踪使用的近档 GLB/GLTF 路径。
    pub author_asset_path: String,
    /// 可选自由备注；缺省时编辑器首次借用旧来源，显式空串表示作者已经清空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // 不升级场景协议；空串必须序列化，不能复活旧来源。
    pub notes: Option<String>, // 仅作者元数据，不参与路径、LOD、模型加载或碰撞校验。
    /// 可选显式运行时层 ID；省略时由稳定预设 ID 确定性派生。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_layer_id: Option<u64>,
    /// Mesh 可选显式原型 ID；省略时由稳定预设 ID 确定性派生。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prototype_id: Option<u64>,
    /// 新建正式密度画布的默认分辨率。
    #[serde(default = "default_vegetation_preset_resolution")]
    pub resolution: [u32; 2],
    /// 满密度候选间距，单位为米。
    pub spacing_meters: f32,
    /// 确定性随机缩放范围。
    #[serde(default = "default_vegetation_preset_scale")]
    pub scale_range: [f32; 2],
    /// 可选显式散布种子；省略时由稳定预设 ID 确定性派生。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Blade 或 Mesh 的完整运行时来源。
    pub runtime: VegetationPresetRuntime,
}

/// 预设的运行时后端与来源。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VegetationPresetRuntime {
    /// 完全由目录声明可变档数 Blade 网格、外观和运动来源。
    Blade {
        /// 叶片候选窗口与网格规格；档数可自定义，level 在档内唯一。
        lods: Vec<BladePresetLod>,
        /// 共享 GPU 叶片后端使用的外观纯值。
        appearance: BladePresetAppearance,
        /// 当前预设使用的综合色纹理页；省略表示白纹理回退。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_color_texture_path: Option<String>,
        /// 风或水流运动来源。
        #[serde(default)]
        motion: BladePresetMotion,
        /// 叶片保守包围和屏幕参考尺寸。
        bounds: BladePresetBounds,
        /// 与 `lods` 档数对齐的屏幕空间 LOD 与常驻规格。
        view_lod: BladePresetViewLod,
        /// 密度图非零概率增益。
        #[serde(default = "default_density_gain")]
        density_gain: f32,
        /// 第四档世界稳定稀疏预算。
        #[serde(default)]
        ultra_lod_budget: BladePresetUltraLodBudget,
        /// 作物第四档复用远景网格时的额外水平覆盖；零表示不用。
        #[serde(default)]
        ultra_horizontal_sweep: f32,
    },
    /// 完全由目录声明任意档位和任意部件的通用 Mesh 原型。
    Mesh {
        /// 距离 LOD；数量不设产品硬上限。
        lods: Vec<MeshPresetLod>,
        /// 地表贴入深度，单位为米。
        #[serde(default)]
        ground_embed_depth: f32,
        /// 物理碰撞策略与渲染原型正交配置。
        #[serde(default)]
        collision: VegetationPresetCollision,
    },
}

impl VegetationPresetRuntime {
    /// 返回该运行时描述采用的正式植被后端。
    pub const fn kind(&self) -> SceneVegetationKind {
        match self {
            Self::Blade { .. } => SceneVegetationKind::Blade, // 海量叶片统一走 GPU 候选窗口。
            Self::Mesh { .. } => SceneVegetationKind::Mesh, // 木本、花卉和岩石统一走通用 Mesh 原型。
        }
    }
}

/// 一档数据驱动 Blade 候选窗口。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BladePresetLod {
    /// F3 与层级树使用的稳定档位名称。
    pub name: String,
    /// 档内唯一身份；归一化后按 level 升序对应几何 LOD 0..n-1。
    pub level: u32,
    /// GLB/GLTF 中的强类型 Mesh 资产路径。
    pub mesh_path: String,
    /// 正方形候选网格单边数量。
    pub grid_side: u32,
    /// 当前档世界候选格距，单位为米。
    pub spacing: f32,
    /// 内环空洞半径，最细档通常为零。
    #[serde(default)]
    pub inner_radius: f32,
    /// 当前窗口安全外半径。
    pub outer_radius: f32,
    /// 内环与更细档交接宽度。
    #[serde(default)]
    pub inner_fade_width: f32,
    /// 外环整株淡出宽度。
    pub outer_fade_width: f32,
    /// 当前档稳定候选盐。
    pub seed: u32,
    /// 玩家焦点移动多少候选格后吸附窗口原点。
    pub snap_cells: u32,
}

impl BladePresetLod {
    /// 返回端点式正方形候选窗口边长。
    pub fn patch_extent(&self) -> f32 {
        self.grid_side.saturating_sub(1) as f32 * self.spacing // 与正式 GPU 窗口构造保持同一公式。
    }
}

/// Blade 绘制着色器的一种完整外观。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BladePresetAppearance {
    /// xy 株高、z 根部下扎、w 最大静态倾斜。
    pub geometry: [f32; 4],
    /// xy 宽度范围、zw 柔韧度范围。
    pub width_flexibility: [f32; 4],
    /// xy 交互/总弯曲上限、zw 风与交互响应倍率。
    pub bending: [f32; 4],
    /// xy 图集缩放、zw 图集偏移。
    pub atlas_uv: [f32; 4],
    /// 线性根部综合色。
    pub root_color: [f32; 4],
    /// 线性尖端综合色。
    pub tip_color: [f32; 4],
    /// 冷色个体扰动；w 保留粗级覆盖开关。
    pub cool_tint: [f32; 4],
    /// 暖色个体扰动；w 保留作物远模复用开关。
    pub warm_tint: [f32; 4],
}

/// Blade 动态形变来源。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "driver", rename_all = "snake_case")]
pub enum BladePresetMotion {
    /// 读取世界统一风场与玩家交互。
    Wind,
    /// 使用开发者声明的水平水流；玩家交互仍可叠加。
    WaterFlow {
        /// 归一化前的世界 X/Z 流向。
        direction: [f32; 2],
        /// 水流动画速度倍率。
        speed: f32,
        /// 草尖最大水流位移，单位为米。
        tip_strength: f32,
        /// 世界波形空间频率倍率。
        #[serde(default = "default_motion_spatial_scale")]
        spatial_scale: f32,
    },
}

impl Default for BladePresetMotion {
    fn default() -> Self {
        Self::Wind // 未声明运动来源的旧目录继续响应世界风。
    }
}

/// 单个 Blade 预设的保守运动包围。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BladePresetBounds {
    /// 最高叶尖相对地表高度。
    pub max_blade_height: f32,
    /// 原型、倾斜和动态形变共同形成的水平扫掠半径。
    pub max_horizontal_sweep: f32,
    /// 屏幕空间 LOD 估算使用的典型株高。
    pub reference_height: f32,
    /// 根部向地表下方延伸的深度。
    pub root_penetration: f32,
}

/// Blade 玩法视图可变档数 LOD 与常驻边界。
///
/// 数组一律按「细 → 粗」排列：`residency_radii[i]` 是第 i 细档，
/// `pixel_thresholds[i]` / `residency_transition_widths[i]` 连接第 i 细档与下一粗档。
#[derive(Clone, Debug, PartialEq)]
pub struct BladePresetViewLod {
    /// 相邻档屏幕空间像素阈值对，长度 = LOD 数 - 1。
    pub pixel_thresholds: Vec<[f32; 2]>,
    /// 镜头进入叶片时整株隐藏和完全恢复距离。
    pub camera_fade_distances: [f32; 2],
    /// 投影估算使用的参考株高。
    pub reference_grass_height: f32,
    /// 各档父簇共享的最细世界格距。
    pub finest_grid_spacing: f32,
    /// 每档常驻半径，长度 = LOD 数。
    pub residency_radii: Vec<f32>,
    /// 窗口外沿额外安全边距。
    pub residency_margin: f32,
    /// 相邻档向祖先代理交还的过渡宽度，长度 = LOD 数 - 1。
    pub residency_transition_widths: Vec<f32>,
}

impl Serialize for BladePresetViewLod {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // 只写新可变格式，旧三对命名字段不再回写。
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("BladePresetViewLod", 7)?;
        state.serialize_field("pixel_thresholds", &self.pixel_thresholds)?;
        state.serialize_field("camera_fade_distances", &self.camera_fade_distances)?;
        state.serialize_field("reference_grass_height", &self.reference_grass_height)?;
        state.serialize_field("finest_grid_spacing", &self.finest_grid_spacing)?;
        state.serialize_field("residency_radii", &self.residency_radii)?;
        state.serialize_field("residency_margin", &self.residency_margin)?;
        state.serialize_field(
            "residency_transition_widths",
            &self.residency_transition_widths,
        )?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for BladePresetViewLod {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // 同时接受新可变列表与旧三对命名字段，旧目录无需批量迁移。
        let mut data = serde_json::Value::deserialize(deserializer)?;
        let object = data
            .as_object_mut()
            .ok_or_else(|| serde::de::Error::custom("BladePresetViewLod 必须为 JSON 对象"))?;
        let pixel_thresholds = if object.contains_key("pixel_thresholds") {
            serde_json::from_value(
                object
                    .remove("pixel_thresholds")
                    .ok_or_else(|| serde::de::Error::custom("缺少 pixel_thresholds"))?,
            )
            .map_err(serde::de::Error::custom)?
        } else {
            // 旧格式三对按细→粗拼成列表，与 residency_* 顺序一致。
            let middle_to_near: [f32; 2] =
                take_pair(object, "middle_to_near_pixels").map_err(serde::de::Error::custom)?;
            let far_to_middle: [f32; 2] =
                take_pair(object, "far_to_middle_pixels").map_err(serde::de::Error::custom)?;
            let ultra_to_far: [f32; 2] =
                take_pair(object, "ultra_to_far_pixels").map_err(serde::de::Error::custom)?;
            vec![middle_to_near, far_to_middle, ultra_to_far]
        };
        let camera_fade_distances: [f32; 2] = serde_json::from_value(
            object
                .remove("camera_fade_distances")
                .ok_or_else(|| serde::de::Error::custom("缺少 camera_fade_distances"))?,
        )
        .map_err(serde::de::Error::custom)?;
        let reference_grass_height: f32 = serde_json::from_value(
            object
                .remove("reference_grass_height")
                .ok_or_else(|| serde::de::Error::custom("缺少 reference_grass_height"))?,
        )
        .map_err(serde::de::Error::custom)?;
        let finest_grid_spacing: f32 = serde_json::from_value(
            object
                .remove("finest_grid_spacing")
                .ok_or_else(|| serde::de::Error::custom("缺少 finest_grid_spacing"))?,
        )
        .map_err(serde::de::Error::custom)?;
        let residency_radii: Vec<f32> = serde_json::from_value(
            object
                .remove("residency_radii")
                .ok_or_else(|| serde::de::Error::custom("缺少 residency_radii"))?,
        )
        .map_err(serde::de::Error::custom)?;
        let residency_margin: f32 = serde_json::from_value(
            object
                .remove("residency_margin")
                .ok_or_else(|| serde::de::Error::custom("缺少 residency_margin"))?,
        )
        .map_err(serde::de::Error::custom)?;
        let residency_transition_widths: Vec<f32> = serde_json::from_value(
            object
                .remove("residency_transition_widths")
                .ok_or_else(|| serde::de::Error::custom("缺少 residency_transition_widths"))?,
        )
        .map_err(serde::de::Error::custom)?;
        Ok(Self {
            pixel_thresholds,
            camera_fade_distances,
            reference_grass_height,
            finest_grid_spacing,
            residency_radii,
            residency_margin,
            residency_transition_widths,
        })
    }
}

fn take_pair(
    object: &mut serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> Result<[f32; 2], serde_json::Error> {
    let value = object
        .remove(name)
        .ok_or_else(|| serde::de::Error::custom(format!("缺少{name}")))?;
    serde_json::from_value(value)
}

impl BladePresetViewLod {
    /// 构造与给定档数对齐的细→粗视图 LOD 配置；相邻数组长度必须为档数减一。
    pub fn new(
        pixel_thresholds: Vec<[f32; 2]>,
        camera_fade_distances: [f32; 2],
        reference_grass_height: f32,
        finest_grid_spacing: f32,
        residency_radii: Vec<f32>,
        residency_margin: f32,
        residency_transition_widths: Vec<f32>,
    ) -> Self {
        Self {
            pixel_thresholds,
            camera_fade_distances,
            reference_grass_height,
            finest_grid_spacing,
            residency_radii,
            residency_margin,
            residency_transition_widths,
        }
    }

    /// 返回当前视图配置描述的 LOD 档数。
    pub fn lod_count(&self) -> usize {
        self.residency_radii.len()
    }
}

/// 第四级叶片世界稳定稀疏档。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BladePresetUltraLodBudget {
    /// 每二乘二父格最多一个代理。
    High,
    /// 每三乘三父格最多一个代理。
    Dense,
    /// 每四乘四父格最多一个代理。
    #[default]
    Medium,
    /// 每八乘八父格最多一个代理。
    Low,
}

/// 通用 Mesh 原型的一档距离 LOD。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshPresetLod {
    /// 诊断使用的稳定档位编号。
    pub level: u8,
    /// 淡入起点与完全可见距离。
    pub start: [f32; 2],
    /// 开始淡出与完全隐藏距离。
    pub end: [f32; 2],
    /// 当前档任意数量的共享部件。
    pub parts: Vec<MeshPresetPart>,
}

/// 通用 Mesh LOD 中的一个共享 PBR 部件。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshPresetPart {
    /// 原型内稳定部件编号。
    pub part_id: u32,
    /// GLB/GLTF 中的强类型 Mesh 资产路径。
    pub mesh_path: String,
    /// 当前部件如何从 `mesh_path` 的源网格取得最终运行时网格。
    #[serde(default)]
    pub mesh_derivation: MeshPresetMeshDerivation,
    /// GLB/GLTF 中的 StandardMaterial 资产路径。
    pub material_path: String,
    /// 对源 StandardMaterial 的显式运行时覆盖；未填写的属性保持资产原值。
    #[serde(default)]
    pub material_overrides: MeshPresetMaterialOverrides,
    /// 部件局部平移、Y 轴旋转角和逐轴缩放。
    #[serde(default)]
    pub transform: MeshPresetPartTransform,
    /// 是否投射方向光阴影。
    #[serde(default = "default_true")]
    pub casts_shadow: bool,
    /// 可选公告板模式。
    #[serde(default)]
    pub billboard: VegetationPresetBillboard,
    /// 可选高度权重风参数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind: Option<MeshPresetWind>,
}

/// Mesh 部件的局部变换。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshPresetPartTransform {
    /// 局部平移。
    pub translation: [f32; 3],
    /// 绕局部 Y 轴旋转角度。
    pub yaw_degrees: f32,
    /// 局部逐轴缩放。
    pub scale: [f32; 3],
}

impl Default for MeshPresetPartTransform {
    fn default() -> Self {
        Self {
            translation: [0.0; 3], // 默认部件与实例根重合。
            yaw_degrees: 0.0,      // 默认不追加局部旋转。
            scale: [1.0; 3],       // 默认保持资产原始尺寸。
        }
    }
}

/// Mesh 公告板朝向方式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VegetationPresetBillboard {
    /// 普通几何或固定交叉面保持作者姿态。
    #[default]
    Fixed,
    /// 绕世界 Y 轴朝向每个渲染视图。
    Cylindrical,
    /// 完整三维朝向每个渲染视图。
    Spherical,
}

/// Mesh 部件最终运行时网格的取得方式。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MeshPresetMeshDerivation {
    /// 直接使用 `mesh_path` 指向的作者网格。
    #[default]
    Direct,
    /// 从薄片植物源网格保留空间分散的真实三角形，适合灌木超远档。
    DispersedTriangles {
        /// 最多保留的源三角形数量；至少为一。
        max_triangles: usize,
    },
    /// 从源网格有限采样点生成封闭凸包，适合岩石超远档。
    ConvexHull {
        /// QuickHull 最多接收的分散采样点数量；至少为四。
        max_sample_points: usize,
    },
}

/// 对 GLTF 源 StandardMaterial 的可编辑覆盖。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshPresetMaterialOverrides {
    /// Alpha 模式覆盖；省略时保留源材质设置。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha_mode: Option<MeshPresetAlphaMode>,
    /// 是否双面绘制；省略时保留源材质设置。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub double_sided: Option<bool>,
}

/// 可由植被预设安全表达的 StandardMaterial Alpha 模式。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MeshPresetAlphaMode {
    /// 强制不透明。
    Opaque,
    /// 使用给定阈值执行 Alpha Mask。
    Mask {
        /// 小于该阈值的像素被丢弃，范围为零至一。
        cutoff: f32,
    },
    /// 使用常规透明混合。
    Blend,
}

/// Mesh 高度权重风参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshPresetWind {
    /// 该局部高度以下风权重为零。
    pub root_height: f32,
    /// 该局部高度以上达到完整权重。
    pub tip_height: f32,
    /// 最大摆幅。
    pub strength: f32,
    /// 动画频率。
    pub frequency: f32,
    /// 当前几何在共享风材质中的部件语义。
    #[serde(default)]
    pub part: MeshPresetWindPart,
}

/// Mesh 风材质的部件语义；树干与叶簇使用不同的形变细节。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshPresetWindPart {
    /// 木质树干或刚性茎干只做宏观弯曲。
    Stem,
    /// 树冠、灌木和花叶可叠加局部细摆。
    #[default]
    Foliage,
}

/// Mesh 预设的物理碰撞策略。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VegetationPresetCollision {
    /// 草、花、灌木或装饰岩石不生成近场阻挡。
    #[default]
    None,
    /// 以实例根为中心生成圆柱近场碰撞。
    Cylinder {
        /// 圆柱半径。
        radius: f32,
        /// 圆柱完整高度。
        height: f32,
        /// 圆柱中心相对实例根的 Y 偏移。
        center_y: f32,
        /// 形状尺寸和中心偏移是否跟随当前实例的统一缩放。
        #[serde(default = "default_collision_scale_with_instance")]
        scale_with_instance: bool,
        /// 玩家进入该水平距离后生成近场碰撞，单位为米。
        #[serde(default = "default_collision_enter_distance")]
        enter_distance: f32,
        /// 玩家离开该水平距离后回收碰撞，必须不小于进入距离。
        #[serde(default = "default_collision_exit_distance")]
        exit_distance: f32,
    },
    /// 以实例根为中心生成长方体近场碰撞。
    Cuboid {
        /// 长方体完整三轴尺寸。
        size: [f32; 3],
        /// 长方体中心相对实例根的偏移。
        center: [f32; 3],
        /// 形状尺寸和中心偏移是否跟随当前实例的统一缩放。
        #[serde(default = "default_collision_scale_with_instance")]
        scale_with_instance: bool,
        /// 玩家进入该水平距离后生成近场碰撞，单位为米。
        #[serde(default = "default_collision_enter_distance")]
        enter_distance: f32,
        /// 玩家离开该水平距离后回收碰撞，必须不小于进入距离。
        #[serde(default = "default_collision_exit_distance")]
        exit_distance: f32,
    },
}

impl VegetationPresetCollision {
    /// 返回近场碰撞创建半径；无碰撞策略不参与物理常驻。
    pub const fn enter_distance(&self) -> Option<f32> {
        match self {
            Self::None => None,
            Self::Cylinder { enter_distance, .. } | Self::Cuboid { enter_distance, .. } => {
                Some(*enter_distance)
            }
        }
    }

    /// 返回带滞回的近场碰撞回收半径。
    pub const fn exit_distance(&self) -> Option<f32> {
        match self {
            Self::None => None,
            Self::Cylinder { exit_distance, .. } | Self::Cuboid { exit_distance, .. } => {
                Some(*exit_distance)
            }
        }
    }
}

const fn default_vegetation_preset_resolution() -> [u32; 2] {
    [512, 512] // 新场景与现有 F12 密度画布保持相同默认清晰度。
}

const fn default_vegetation_preset_scale() -> [f32; 2] {
    [1.0, 1.0] // 未声明时保持资产原始尺度。
}

const fn default_collision_enter_distance() -> f32 {
    72.0 // 角色靠近前一块范围内开始生成，避免穿入时才出现。
}

const fn default_collision_scale_with_instance() -> bool {
    true // 数据 Mesh 默认把碰撞规格视为模型局部尺寸，作者可按模型需要显式关闭。
}

const fn default_collision_exit_distance() -> f32 {
    96.0 // 比进入距离多二十四米形成稳定滞回，减少边界反复创建。
}

const fn default_density_gain() -> f32 {
    1.0 // 默认严格服从作者 R8 概率。
}

const fn default_motion_spatial_scale() -> f32 {
    1.0 // 水流默认沿用正式叶片世界波形尺度。
}

const fn default_true() -> bool {
    true // Mesh 新部件默认参与方向光阴影，与标准 PBR 预期一致。
}

#[cfg(test)]
mod tests;
