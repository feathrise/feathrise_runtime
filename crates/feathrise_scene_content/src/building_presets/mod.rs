//! F12 建筑预设目录的数据格式、稳定身份与局部空间语义。
//!
//! 【功能设计】场景摆放记录继续保存可独立加载的模型路径，同时附带可选稳定预设 ID；
//! 因此目录损坏时既能保留上一份有效元数据快照，也不会让已经保存的房屋模型消失。

use bevy::prelude::*; // Asset 与 TypePath 让目录沿用 Bevy 异步热加载链。
use serde::{Deserialize, Serialize}; // 建筑预设由开发者维护的 JSON 持久化。

use crate::ScenePlacementAssetTarget; // 建筑模型目标复用普通摆放的 Scene0 或静态 primitive 协议。

mod geometry; // 占地、门前净空与正门标记使用同一局部到世界变换。
mod registry; // 已校验目录快照和规范 ID/历史别名查询。
mod validation; // 版本、身份、路径和局部几何的纯校验。

pub use geometry::BuildingWorldQuad; // F12 索引、Gizmo 与正式净空派生共享唯一几何结果。
pub use registry::ResolvedBuildingPresetCatalog; // F12 与运行时共享同一原子目录快照。
pub use validation::valid_building_preset_id; // 摆放记录基础校验复用稳定身份规则。

/// 当前建筑预设目录格式版本。
pub const BUILDING_PRESET_SCHEMA_VERSION: u32 = 1; // 连接点是带默认值的向后兼容扩展，不改变首版目录身份。

/// 单个建筑预设允许的地面平面连接点上限。
pub const MAX_BUILDING_CONNECTION_POINTS: usize = 32; // 视口吸附会按所选预设逐点比较，必须保持固定小上限。

/// 开发者维护的全部建筑预设目录资产。
#[derive(Asset, TypePath, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildingPresetCatalogAsset {
    /// 不兼容格式升级必须显式提高版本并提供迁移。
    pub schema_version: u32,
    /// 按开发者声明顺序显示的全部建筑预设。
    pub presets: Vec<BuildingPresetDescriptor>,
}

/// F12 建筑库使用的作者分类；分类只影响浏览，不改变摆放运行时。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildingPresetCategory {
    /// 普通住宅和院落正房。
    #[default]
    Residence,
    /// 客栈、酒楼和旅舍。
    Inn,
    /// 商铺、作坊和临街店面。
    Shop,
    /// 亭、廊等小型开放建筑。
    Pavilion,
    /// 城门、寨门和大型门楼。
    Gate,
    /// 独立围墙段或墙楼。
    Wall,
    /// 摊位、棚架和临时小构筑物。
    Stall,
    /// 未归入固定浏览类别的其他建筑。
    Other,
}

impl BuildingPresetCategory {
    /// 返回 F12 紧凑列表使用的中文分类名。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Residence => "民居", // 默认单体房屋类别。
            Self::Inn => "客栈",       // 住宿或大型公共房屋。
            Self::Shop => "商铺",      // 临街商业建筑。
            Self::Pavilion => "亭廊",  // 开放式小建筑。
            Self::Gate => "城门",      // 大型门楼或寨门。
            Self::Wall => "围墙",      // 独立墙体构筑物。
            Self::Stall => "摊位",     // 小型临时构筑物。
            Self::Other => "其他",     // 自定义扩展类别。
        }
    }
}

/// 建筑内部可达性的作者声明。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildingInteriorKind {
    /// 建筑只提供外观与外部碰撞。
    #[default]
    Inaccessible,
    /// 模型具有真实门洞和可直接行走的内部空间。
    DirectEntry,
    /// 门口将来连接另一场景或独立室内场景。
    PortalEntry,
}

impl BuildingInteriorKind {
    /// 返回 F12 摘要使用的中文短标签。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Inaccessible => "不可进入", // 只把建筑作为外部构筑物。
            Self::DirectEntry => "直接进入",  // 依赖模型门洞和正式碰撞。
            Self::PortalEntry => "传送进入",  // 实际传送内容仍由场景传送系统配置。
        }
    }
}

/// 建筑局部 X/Z 平面上的轴对齐矩形。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildingFootprintRect {
    /// 相对模型根的局部 X/Z 中心，单位为米。
    pub center: [f32; 2],
    /// 局部 X/Z 半尺寸，两个分量都必须为正。
    pub half_extents: [f32; 2],
}

impl Default for BuildingFootprintRect {
    fn default() -> Self {
        Self {
            center: [0.0, 0.0],       // 新预设默认以模型根为占地中心。
            half_extents: [2.0, 2.0], // 四米见方提供可见且可继续编辑的安全起点。
        }
    }
}

/// 建筑正门及门前独立净空语义。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildingEntranceDescriptor {
    /// 门槛相对模型根的局部三维位置，单位为米。
    pub position: [f32; 3],
    /// 门朝外方向相对局部 +Z 的 Y 轴角度。
    pub outward_yaw_degrees: f32,
    /// 门前矩形的局部横向与向外纵向半尺寸，单位为米。
    pub clearance_half_extents: [f32; 2],
}

impl Default for BuildingEntranceDescriptor {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 2.0],          // 默认门位于四米占地的局部 +Z 边缘。
            outward_yaw_degrees: 0.0,           // 局部 +Z 是默认正门朝外方向。
            clearance_half_extents: [1.0, 1.5], // 默认保留两米宽、三米深门前空间。
        }
    }
}

/// 建筑模型底部与坡地容差。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildingFoundationDescriptor {
    /// 模型根空间中应贴合采用地基高度的基准 Y，单位为米。
    pub model_base_y: f32,
    /// 占地采样点允许的最大地面高差，单位为米。
    pub max_ground_height_delta: f32,
}

impl Default for BuildingFoundationDescriptor {
    fn default() -> Self {
        Self {
            model_base_y: 0.0,             // 大多数 GLB 以根节点 Y=0 作为地基基准。
            max_ground_height_delta: 0.35, // 默认允许轻微自然坡度但拒绝明显悬空。
        }
    }
}

/// 建筑模块在地面平面上的一个稳定手工吸附点。
///
/// 【功能设计】首期只表达模型地基平面中的 X/Z 位置，不承诺柱顶、梁端或楼层的三维
/// 装配；两个点仅在 `kind` 完全相同时兼容，吸附后两侧朝外方向相反。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildingConnectionPointDescriptor {
    /// 预设内稳定且唯一的技术 ID；调整位置时不得复用给另一处连接。
    pub id: String,
    /// 显式兼容类型；首期只有完全相同的类型才能连接。
    pub kind: String,
    /// 相对模型根的局部 X/Z 位置，Y 固定使用模型地基基准。
    pub position: [f32; 2],
    /// 从模块内部朝外的局部方向，只允许九十度档位。
    pub outward_yaw_degrees: f32,
}

impl Default for BuildingConnectionPointDescriptor {
    fn default() -> Self {
        Self {
            id: "connection-1".to_owned(), // 新增后可直接保存并继续改成语义名称。
            kind: "module".to_owned(),     // 默认类型只与同样明确声明的点兼容。
            position: [0.0, 0.0],          // 初始点位于模型根的地基平面。
            outward_yaw_degrees: 0.0,      // 局部 +Z 是默认朝外方向。
        }
    }
}

/// 一个可跨场景复用并由普通摆放链实例化的建筑预设。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildingPresetDescriptor {
    /// 跨场景稳定且不得复用的技术 ID。
    pub id: String,
    /// 重命名前的历史 ID；旧场景仍可解析，但新保存只写规范 ID。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// F12 面向场景作者显示的名称。
    pub label: String,
    /// 用于连续列表分组和摘要的作者分类。
    #[serde(default)]
    pub category: BuildingPresetCategory,
    /// 相对 assets 根的 GLB 或 GLTF 路径。
    pub asset_path: String,
    /// 文件内的完整 Scene0 或静态 primitive 目标。
    #[serde(default, skip_serializing_if = "ScenePlacementAssetTarget::is_scene0")]
    pub asset_target: ScenePlacementAssetTarget,
    /// 房屋实体占地；不包含门前净空。
    pub footprint: BuildingFootprintRect,
    /// 正门、朝向和门前独立净空。
    pub entrance: BuildingEntranceDescriptor,
    /// 地基基准和允许的坡地高差。
    #[serde(default)]
    pub foundation: BuildingFoundationDescriptor,
    /// 可选地面平面连接点；旧目录缺字段时保持普通点放行为。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<BuildingConnectionPointDescriptor>,
    /// 对最终植被结果生效、但不改写作者 R8 的局部净空矩形。
    pub vegetation_clearance: BuildingFootprintRect,
    /// 建筑内部的可达方式声明。
    #[serde(default)]
    pub interior: BuildingInteriorKind,
}

impl BuildingPresetDescriptor {
    /// 使用已选 GLB/GLTF 目标构造可立即保存并继续编辑的安全初始预设。
    pub fn from_asset(
        id: String,                            // 调用方负责生成目录内唯一稳定 ID。
        label: String,                         // 初始名称通常来自文件名。
        asset_path: String,                    // 保留用户在右侧明确选择的资产路径。
        asset_target: ScenePlacementAssetTarget, // 保留 Scene0 或静态 primitive 精确目标。
    ) -> Self {
        Self {
            id,                                                     // 目录和场景实例共享该稳定身份。
            aliases: Vec::new(),                                    // 新建项尚无历史名称。
            label,                                           // 作者可在后续编辑器中修改显示名。
            category: BuildingPresetCategory::default(),     // 新房屋从民居类别开始。
            asset_path,                                      // 模型仍能独立于预设目录直接加载。
            asset_target,                                    // 子资产选择不退化成 Scene0 猜测。
            footprint: BuildingFootprintRect::default(),     // 初始四米占地可直接预览。
            entrance: BuildingEntranceDescriptor::default(), // 初始正门位于局部 +Z。
            foundation: BuildingFoundationDescriptor::default(), // 使用通用贴地与坡差基线。
            connections: Vec::new(),                         // 普通完整房屋不被误当成拼装模块。
            vegetation_clearance: BuildingFootprintRect::default(), // 初始净空与实体占地一致。
            interior: BuildingInteriorKind::default(),       // 未确认门洞前保持不可进入声明。
        }
    }

    /// 返回当前实例的世界占地四边形。
    pub fn world_footprint(
        &self,
        translation: [f32; 3], // 实例世界根位置。
        yaw_degrees: f32,      // 实例世界 Y 轴角度。
        scale: [f32; 3],       // 实例三轴缩放；只取 X/Z 构造平面几何。
    ) -> BuildingWorldQuad {
        BuildingWorldQuad::from_local_rect(self.footprint, translation, yaw_degrees, scale) // 占地、索引与预览不得使用不同公式。
    }

    /// 返回当前实例的世界门前净空四边形。
    pub fn world_entrance_clearance(
        &self,
        translation: [f32; 3], // 实例世界根位置。
        yaw_degrees: f32,      // 实例世界 Y 轴角度。
        scale: [f32; 3],       // 非均匀缩放会得到仍可精确 SAT 的凸四边形。
    ) -> BuildingWorldQuad {
        BuildingWorldQuad::from_entrance_clearance(
            self.entrance, // 正门局部位置、朝向和净空半尺寸来自同一预设快照。
            translation,   // 世界根位置与实际摆放记录一致。
            yaw_degrees,   // 世界旋转与模型预览一致。
            scale,         // 世界平面尺寸与模型 X/Z 缩放一致。
        )
    }

    /// 返回当前实例正门门槛世界位置与归一化朝外方向。
    pub fn world_entrance_marker(
        &self,
        translation: [f32; 3], // 实例世界根位置。
        yaw_degrees: f32,      // 实例 Y 轴旋转。
        scale: [f32; 3],       // 门槛位置随三轴缩放，方向只随旋转。
    ) -> (Vec3, Vec3) {
        geometry::world_entrance_marker(&self.entrance, translation, yaw_degrees, scale) // Gizmo 与后续沿街布局复用唯一入口方向。
    }

    /// 返回一个地面连接点的世界位置与归一化朝外方向。
    pub fn world_connection_marker(
        &self,
        connection: &BuildingConnectionPointDescriptor, // 当前预设内已校验连接点。
        translation: [f32; 3],                          // 实例世界根位置。
        yaw_degrees: f32,                               // 实例世界 Y 轴角度。
        scale: [f32; 3],                                // 位置遵循模型 S→R→T 变换。
    ) -> (Vec3, Vec3) {
        geometry::world_connection_marker(
            connection,
            self.foundation.model_base_y, // 首期连接点统一落在模型地基平面。
            translation,
            yaw_degrees,
            scale,
        )
    }
}

#[cfg(test)]
mod tests; // 数据兼容、原子校验、别名与几何纯函数回归。
