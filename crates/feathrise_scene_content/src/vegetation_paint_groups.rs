//! 场景级植被绘制组：共享预设和目标集合，实际密度始终归各TerrainId本地所有。
use crate::{SceneVegetationLayer, TerrainId};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneVegetationPaintGroup {
    pub id: String,
    pub label: String,
    /// 创建时冻结正式预设规格；网格移动、目录排序不改变种子和已画分布。
    pub template: SceneVegetationLayer,
    /// 显式勾选范围，与检查器当前选择分离；删除对象后保留引用供撤销恢复。
    pub targets: BTreeSet<TerrainId>,
    /// 只有真正落笔后才登记成员；不得把同名或同预设作者层自动认作同一成员。
    #[serde(default)]
    pub members: BTreeMap<TerrainId, String>,
}

pub(crate) fn is_empty(groups: &Arc<[SceneVegetationPaintGroup]>) -> bool {
    groups.is_empty()
}

pub(crate) fn validate(groups: &[SceneVegetationPaintGroup]) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    let mut members = BTreeSet::new();
    let safe = |id: &str| !id.is_empty() && !id.contains(['/', '\\']) && !id.contains("..");
    for group in groups {
        if !safe(&group.id) || group.label.trim().is_empty() || !ids.insert(&group.id) {
            return Err("植被绘制组ID无效、重复或名称为空".into());
        }
        if !safe(&group.template.id)
            || group
                .template
                .resolution
                .iter()
                .any(|v| !(2..=4096).contains(v))
            || !group.template.spacing_meters.is_finite()
            || group.template.spacing_meters <= 0.
        {
            return Err("植被绘制组预设或分辨率无效".into());
        }
        for id in group.targets.iter().chain(group.members.keys()) {
            TerrainId::parse(id.as_str())?;
        }
        for (id, layer) in &group.members {
            if !safe(layer) || !members.insert((id, layer)) {
                return Err("植被作者层不能被多个绘制组重复认领".into());
            }
        }
    }
    Ok(())
}
