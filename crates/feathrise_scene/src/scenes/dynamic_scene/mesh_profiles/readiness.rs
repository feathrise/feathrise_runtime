//! 只读工程预设的精确资产就绪证据；不授予场景、实例或作者结果的采用权限。

use bevy::{ecs::system::SystemParam, prelude::*};
use feathrise_environment::vegetation::{VegetationMeshPrototypeRegistry, VegetationPrototypeId};
use feathrise_scene_content::VegetationPresetRegistry;
use super::resources::{DynamicMeshProfileCache, DynamicMeshProfileKey};

/// 按规范预设、目录修订和当前原型登记三者联合查询，不能把全局 Ready 当作任意预设就绪。
#[derive(SystemParam)]
pub struct DynamicMeshProfileReadiness<'w> {
    cache: Option<Res<'w, DynamicMeshProfileCache>>,
}

impl DynamicMeshProfileReadiness<'_> {
    pub fn ready_revision(&self, profile: &str, expected_prototype: VegetationPrototypeId, presets: &VegetationPresetRegistry,
        registry: &VegetationMeshPrototypeRegistry) -> Option<u64> {
        self.cache.as_deref()?.ready_revision(profile, expected_prototype, presets, registry)
    }

    pub fn failure_detail<'a>(&'a self, profile: &str, presets: &VegetationPresetRegistry) -> Option<&'a str> {
        let preset = presets.resolve(profile)?;
        let key = DynamicMeshProfileKey { catalog_revision: presets.revision, preset_id: preset.id.clone() };
        self.cache.as_deref()?.entries.get(&key)?.failure.as_deref()
    }
}

impl DynamicMeshProfileCache {
    pub(super) fn ready_revision(&self, profile: &str, expected_prototype: VegetationPrototypeId, presets: &VegetationPresetRegistry,
        registry: &VegetationMeshPrototypeRegistry) -> Option<u64> {
        let preset = presets.resolve(profile)?;
        let prototype_id = VegetationPrototypeId(preset.resolved_prototype_id()?);
        let key = DynamicMeshProfileKey { catalog_revision: presets.revision, preset_id: preset.id.clone() };
        let entry = self.entries.get(&key)?;
        (prototype_id == expected_prototype && entry.prototype_id == prototype_id && entry.ready(registry))
            .then(|| registry.revision(prototype_id)).flatten()
    }
}
