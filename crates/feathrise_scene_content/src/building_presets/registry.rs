//! 已校验建筑预设目录的稳定查询快照。

use std::collections::BTreeMap; // 有序索引让热更与测试结果保持确定。

use super::{BuildingPresetCatalogAsset, BuildingPresetDescriptor, validation::validate_catalog}; // 注册表只依赖同域数据与纯校验。

/// 已通过完整校验、可按规范 ID 或历史别名查询的建筑目录。
#[derive(Clone, Debug, Default)]
pub struct ResolvedBuildingPresetCatalog {
    presets: Vec<BuildingPresetDescriptor>, // 展示顺序与开发者 JSON 完全一致。
    lookup: BTreeMap<String, usize>,        // 规范 ID 和别名共同指向唯一规范项。
}

impl ResolvedBuildingPresetCatalog {
    /// 完整校验成功后一次性建立可采用快照。
    pub fn build(asset: &BuildingPresetCatalogAsset) -> Result<Self, String> {
        validate_catalog(asset)?; // 任一项损坏都拒绝整份修订，调用方继续保留旧快照。
        let mut lookup = BTreeMap::new(); // 已校验命名空间唯一，插入不会覆盖另一项。
        for (index, preset) in asset.presets.iter().enumerate() {
            lookup.insert(preset.id.clone(), index); // 规范身份始终可直接查询。
            for alias in &preset.aliases {
                lookup.insert(alias.clone(), index); // 历史身份只解析到当前规范项。
            }
        }
        Ok(Self {
            presets: asset.presets.clone(), // 每次成功热更只复制一次完整目录。
            lookup,                         // 稳定帧通过有序索引借用查询。
        })
    }

    /// 按开发者声明顺序返回全部规范预设。
    pub fn presets(&self) -> &[BuildingPresetDescriptor] {
        &self.presets // UI 连续列表借用切片，不为稳定帧复制标签或几何。
    }

    /// 通过规范 ID 或历史别名解析当前预设。
    pub fn resolve(&self, id: &str) -> Option<&BuildingPresetDescriptor> {
        self.lookup.get(id).map(|index| &self.presets[*index]) // 返回规范项，下一次保存可迁移身份。
    }
}
