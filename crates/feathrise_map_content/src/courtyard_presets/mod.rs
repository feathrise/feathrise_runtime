//! 院落组合预设的全局作者格式；成员保留独立模型来源和相对院落枢轴的变换。
//!
//! 【功能设计】组合预设只描述可重复使用的摆放原型，地图仍保存每个成员的明确实例。
//! 地图原有摆放记录不依赖此目录，因此没有组合引用的旧地图可以原样加载。

use bevy::prelude::*; // Asset 和 TypePath 接入既有异步 JSON 资产链。
use serde::{Deserialize, Serialize}; // F12 保存和热更新共用同一序列化协议。

use crate::{MapPlacementAssetTarget, MapPlacementHeightMode}; // 成员模型与高度模式沿用普通显式摆放协议。

mod registry; // 原子构建已校验的稳定 ID 索引。
mod validation; // 版本、成员预算、路径及有限变换的完整校验。

pub use registry::ResolvedCourtyardPresetCatalog; // 编辑器与地图运行时借用同一完整快照。
pub use validation::valid_courtyard_preset_id; // 作者创建规范 ID 时复用目录规则。

/// 当前院落组合预设的 JSON 格式版本。
pub const COURTYARD_PRESET_SCHEMA_VERSION: u32 = 1; // 不兼容变更须升级版本并提供迁移。
/// 一个组合最多包含的显式摆放成员数。
pub const MAX_COURTYARD_MEMBERS_PER_PRESET: usize = 256; // 单次印章的候选规模必须有明确上限。
/// 全局目录最多保存的组合项数。
pub const MAX_COURTYARD_PRESETS: usize = 512; // 热更校验不扫描无界的组合列表。
/// 整份目录允许的成员总数。
pub const MAX_COURTYARD_TOTAL_MEMBERS: usize = 8_192; // 与单图显式摆放上限一致，控制主线程快照构建成本。

/// 开发者维护的全局院落组合目录资产。
#[derive(Asset, TypePath, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CourtyardPresetCatalogAsset {
    /// 目录格式版本。
    pub schema_version: u32,
    /// 按作者顺序展示的完整组合列表。
    pub presets: Vec<CourtyardPresetDescriptor>,
}

impl CourtyardPresetCatalogAsset {
    /// 保存前校验整份目录；任何损坏项都拒绝本次写入或热采用。
    pub fn validate(&self) -> Result<(), String> {
        validation::validate_catalog(self) // 与已解析注册表使用完全相同的规则。
    }
}

/// 一个由多个明确资产实例组成、可重复盖印的院落组合。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CourtyardPresetDescriptor {
    /// 跨地图稳定且不可复用的组合身份。
    pub id: String,
    /// F12 列表中的作者可读名称。
    pub label: String,
    /// 成员顺序用于稳定地复现组合和编辑列表。
    pub members: Vec<CourtyardMemberDescriptor>,
}

/// 组合中的一个独立摆放原型。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CourtyardMemberDescriptor {
    /// 保存所选实例为模板时保留其LOD覆盖；旧目录缺项继续继承全局。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh_lod: Option<crate::MeshLodPolicy>,
    /// 在本组合内稳定的成员身份；同一成员的局部修改可跨预设更新保留。
    pub id: String,
    /// 相对 assets 根的 GLB 或 GLTF 模型路径。
    pub asset_path: String,
    /// 完整 Scene0 或明确的静态 primitive；旧式完整目标可省略。
    #[serde(default, skip_serializing_if = "MapPlacementAssetTarget::is_scene0")]
    pub asset_target: MapPlacementAssetTarget,
    /// 可选规范建筑预设身份；装饰和全部普通资产保持 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub building_preset_id: Option<String>,
    /// 相对院落枢轴的局部 XYZ，单位为米；旋转院落时 XZ 同步旋转。
    pub local_translation: [f32; 3],
    /// 相对院落朝向的 Y 轴角度，单位为度。
    pub local_yaw_degrees: f32,
    /// 成员自身的三轴缩放，不叠乘院落整体比例。
    pub scale: [f32; 3],
    /// 固定高度或依当前权威地形重新贴地。
    #[serde(default)]
    pub height_mode: MapPlacementHeightMode,
    /// 贴地模式下相对采样地面的竖向偏移，单位为米。
    #[serde(default)]
    pub terrain_offset: f32,
}

#[cfg(test)]
mod tests; // JSON 往返、无效目录和热更原子保留回归。
