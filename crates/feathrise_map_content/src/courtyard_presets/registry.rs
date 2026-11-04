//! 已通过完整校验的院落组合预设查询快照。

use std::collections::BTreeMap; // 确定性索引让作者列表和热更行为稳定。

use super::{CourtyardPresetCatalogAsset, CourtyardPresetDescriptor}; // 只借用同域的数据协议。

/// 可按规范 ID 查找、按作者顺序遍历的完整院落组合目录。
#[derive(Clone, Debug, Default)]
pub struct ResolvedCourtyardPresetCatalog {
    presets: Vec<CourtyardPresetDescriptor>, // 原始顺序用于 F12 列表和成员模板复现。
    lookup: BTreeMap<String, usize>,         // 每个稳定组合 ID 只指向一项完整预设。
}

impl ResolvedCourtyardPresetCatalog {
    /// 全量校验成功后才构建新的原子快照。
    pub fn build(asset: &CourtyardPresetCatalogAsset) -> Result<Self, String> {
        asset.validate()?; // 任一项损坏时不返回部分已解析目录。
        let mut lookup = BTreeMap::new(); // 验证阶段已确保所有组合 ID 唯一。
        for (index, preset) in asset.presets.iter().enumerate() {
            lookup.insert(preset.id.clone(), index); // 稳定 ID 与声明顺序分别保留。
        }
        Ok(Self {
            presets: asset.presets.clone(), // 成功热更只复制一次完整有界目录。
            lookup,                         // 新索引与新成员数组在同一返回边界发布。
        })
    }

    /// 按作者声明顺序借用全部组合。
    pub fn presets(&self) -> &[CourtyardPresetDescriptor] {
        &self.presets // 稳定帧不复制成员或字符串。
    }

    /// 解析一个规范组合 ID。
    pub fn resolve(&self, id: &str) -> Option<&CourtyardPresetDescriptor> {
        self.lookup.get(id).map(|index| &self.presets[*index]) // 每个 ID 精确命中一份完整组合。
    }
}
