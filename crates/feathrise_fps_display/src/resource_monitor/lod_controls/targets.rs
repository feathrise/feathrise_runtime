//! 固定九槽的已解析诊断身份；绑定或目录退出时仍能精确归还上一批被调试隐藏的节点。

use super::{LodControlContent, LodControlPresetBindings};
use feathrise_scene_content::VegetationPresetRegistry;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::resource_monitor) struct Targets {
    pub(super) layers: [Option<u64>; 9],
    pub(super) prototypes: [Option<u64>; 9],
    pub(super) bound: [bool; 9],
}

impl Targets {
    pub(super) fn resolve(
        presets: Option<&VegetationPresetRegistry>,
        bindings: Option<&LodControlPresetBindings>,
    ) -> Self {
        let mut targets = Self::default();
        if let Some(bindings) = bindings {
            for (content, id) in bindings.iter() {
                targets.bound[content.slot()] = true;
                if let Some(preset) = presets.and_then(|presets| presets.resolve(id)) {
                    targets.layers[content.slot()] = Some(preset.resolved_runtime_layer_id());
                    targets.prototypes[content.slot()] = preset.resolved_prototype_id();
                }
            }
        }
        targets
    }

    pub(super) fn blade(&self, layer: u64) -> Option<LodControlContent> {
        LodControlContent::ALL
            .into_iter()
            .find(|content| self.layers[content.slot()] == Some(layer))
    }

    pub(super) fn mesh(&self, prototype: u64) -> Option<LodControlContent> {
        LodControlContent::ALL
            .into_iter()
            .find(|content| self.prototypes[content.slot()] == Some(prototype))
    }
}
