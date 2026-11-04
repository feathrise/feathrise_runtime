//! 地图稳定标识与纯数据枚举。

use std::fmt; // 调试、日志和UI统一通过Display显示地图稳定ID。

use serde::{Deserialize, Serialize}; // 地图作者文件使用Serde读写JSON。

/// 公共地图运行方式；不使用任何具体游戏的持久场景枚举。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapRuntimeKind {
    /// 由工程清单、资产及子地图数据装配的普通场景。
    DataDriven,
}

impl MapRuntimeKind {
    /// 唯一旧值兼容边界；未知或已退役类型必须明确拒绝，不能落回普通地图。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "data_driven" | "DynamicMapScene" => Some(Self::DataDriven),
            _ => None,
        }
    }

    /// 新建和派生地图只写公共名称；读取旧地图不会调用此方法改写原字段。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DataDriven => "data_driven",
        }
    }
}

/// 跨地图清单、传送目标和编辑稿使用的稳定地图标识。
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MapId(pub String);

impl MapId {
    /// 构造并校验只含ASCII字母、数字、短横线和下划线的安全目录标识。
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into(); // 先取得所有权，成功时可直接保存而不重复分配。
        let valid = !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
        valid
            .then_some(Self(value))
            .ok_or_else(|| "地图ID必须为1至64个ASCII字母、数字、短横线或下划线".to_owned())
    }

    /// 返回可用于资源路径和日志的字符串切片。
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for MapId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 地图地形的权威作者源类型。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapTerrainKind {
    /// 地图容器没有主地形；真正的地形由独立对象列表持有。
    Empty,
    /// 现有Blender地图继续以GLB作为权威地形源。
    #[default]
    Gltf,
    /// 编辑器新建地图以高度场和河流参数作为权威源，GLB仅为派生产物。
    Heightfield,
    /// 新建地图的稀疏作者高度块由Clipmap显示；洞穴和碰撞使用独立世界空间数据。
    SparseClipmap,
    /// 独立三维密度地形；显示和碰撞由自己的体积源与LOD配置生成。
    Voxel,
}

/// 单张稀疏地图的物理路线；缺省必须保持旧地图原有的分块承托。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapClipmapCollisionMode {
    /// 世界固定分块、玩家中心的可配置多层区域共享碰撞。
    #[default] // 所有稀疏地图统一使用合并方案。
    #[serde(alias = "legacy_chunks", alias = "shared_display_mesh")]
    // 旧字符串仅兼容读取，不再对应运行时路线。
    LayeredSharedChunks,
}

/// 一层碰撞覆盖的累计最远距离与相对作者格距倍率。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] // 保存完整列表，比较配置身份。
pub struct MapCollisionLod {
    // 仅新碰撞模式消费，不改变显示LOD。
    pub max_distance_meters: f32, // 从玩家水平位置计算累计半径。
    pub spacing_multiplier: u32,  // 第一阶段只允许二次幂倍率。
} // 作者记录不携带运行时块坐标。

/// 一层Geometry Clipmap显示环带的累计方形半径与相对作者格距倍率。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] // 清单保存作者列表，运行时按完整值比较换代身份。
pub struct MapTerrainDisplayLod {
    // 仅SparseClipmap消费，GLB和旧Heightfield不得保存该字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // 最后一层省略字段即表示延伸至地图边界。
    pub outer_half_extent_meters: Option<f32>, // Some是距显示焦点的累计方形半径，None只允许位于末项。
    pub spacing_multiplier: u32, // 相对作者实际格距的二次幂倍率，不逐层相乘。
} // 作者数据不携带量化后的两轴格数或内部2:1过渡带。

/// 当前地图显示LOD从保存到GPU采用所处的公开诊断阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] // F12只读状态无需进入地图作者JSON。
pub enum MapTerrainDisplayLodPhase {
    // 具体失败原因仍由状态资源中的detail携带。
    #[default] // 没有活动SparseClipmap或尚未收到清单时保持空闲。
    Idle, // 当前没有显示LOD构建事务。
    LegacyAutomatic,     // 缺少作者字段，继续使用旧64半格自动层级。
    Queued,              // 已保存配置已经进入最新期望，但后台任务尚未启动。
    BuildingMesh,        // 有界后台任务正在生成完整组合Mesh。
    LoadingHeightPages,  // 正在等待当前显示范围所需的稀疏高度页。
    EncodingHeightPages, // 后台正在编码完整候选高度图集。
    Uploading,           // 主世界正在分帧登记候选Mesh与图集。
    WarmingGpu,          // RenderApp正在验证候选资产和实际管线。
    Applied,             // 已保存配置与当前正式显示配置一致。
    Failed,              // 候选失败且上一完整显示仍被保留。
} // 阶段枚举不用于驱动保存协议。

impl MapTerrainKind {
    /// 网格用途与几何能力分离：导入GLB仍能显示/碰撞，但没有可写的高度作者源。
    pub const fn supports_height_sculpt(self) -> bool {
        matches!(self, Self::Heightfield | Self::SparseClipmap)
    }

    /// 只有密度作者或带三维挖掘层的Clipmap能够改变体积，不能把任意模型投成高度图。
    pub const fn supports_volume_sculpt(self) -> bool {
        matches!(self, Self::SparseClipmap | Self::Voxel)
    }

    /// 数据驱动湖泊和池塘是否能使用该地图的作者画布。
    ///
    /// 【功能设计】Heightfield继续使用完整GLB派生高度；SparseClipmap改用已采用分页高度。
    /// Empty的独立R8平面不要求地形；专用Gltf场景仍保留各自既有水体实现。
    pub const fn supports_static_water(self) -> bool {
        matches!(self, Self::Empty | Self::Heightfield | Self::SparseClipmap | Self::Voxel)
    }

    /// 河流、急流和瀑布样条是否能使用世界坐标带状网格后端。
    ///
    /// 【功能设计】样条控制点已经保存完整世界XYZ，不需要从地形反推整图河床；
    /// Heightfield与SparseClipmap因此共用同一固定预算Ribbon构建器。
    pub const fn supports_spline_water(self) -> bool {
        matches!(self, Self::Heightfield | Self::SparseClipmap | Self::Voxel)
    }

    /// 旧数据水域后端是否能从该地形路线取得完整GLB河床。
    ///
    /// SparseClipmap的静水与样条水使用各自专用后端；这里仍返回false，避免误入
    /// 要求完整高度缓存与CPU地形网格的构建器。摆放GLB的整网格赋水只依赖对象
    /// 自身的网格、层级和UV，三类地形共用该入口，不受此地形河床能力门限制。
    pub const fn supports_glb_derived_water(self) -> bool {
        matches!(self, Self::Heightfield)
    }
}

/// 可供编辑器资产浏览器识别的资源用途。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapAssetKind {
    /// GLB或GLTF三维场景，可用于显式实例画笔。
    Scene,
    /// PNG/JPEG/KTX2纹理，可分配给密度、权重或材质槽。
    Texture,
    /// JSON或二进制作者数据，只允许导入对应数据槽。
    Data,
    /// 不属于地图编辑器可消费类型的文件。
    #[default]
    Unsupported,
}

/// 植被层的运行时实现类别。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapVegetationKind {
    /// 草、麦等海量叶片内容；正式运行时使用GPU候选窗口。
    #[default]
    Blade,
    /// 树木、灌木等稀疏三维模型；作者资产保存为稳定密度层。
    Mesh,
}
