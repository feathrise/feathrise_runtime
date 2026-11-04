//! 关联解析索引在后台构造；逐条查询O(log N)，被删除或禁用地形绝不回退到其他地面。
use crate::{
    MapManifestAsset, MapPlacementRecord, MapTerrainBinding, TerrainId, TerrainObjectTransform,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct TerrainBindingIndex {
    poses: BTreeMap<TerrainId, TerrainObjectTransform>,
}
impl TerrainBindingIndex {
    /// 大集合只允许从作者/加载后台任务调用；主线程捕获共享清单后移交任务。
    pub fn from_manifest(manifest: &MapManifestAsset) -> Self {
        Self::from_objects(&manifest.terrain_objects_for_edit())
    }
    pub fn from_objects(objects: &[crate::MapTerrainObject]) -> Self {
        let poses = objects
            .iter()
            .filter(|object| object.enabled)
            .map(|object| (object.id.clone(), object.transform))
            .collect();
        Self { poses }
    }
    pub fn insert(&mut self, id: TerrainId, pose: TerrainObjectTransform) {
        self.poses.insert(id, pose);
    }
    pub fn pose(&self, id: &TerrainId) -> Option<TerrainObjectTransform> {
        self.poses.get(id).copied()
    }
    pub fn resolve(&self, binding: &MapTerrainBinding) -> Result<TerrainObjectTransform, String> {
        binding.resolve(self.pose(&binding.terrain_id).ok_or_else(|| {
            format!(
                "关联地形{}已删除、禁用或尚未就绪",
                binding.terrain_id.as_str()
            )
        })?)
    }
    pub fn resolve_placement(&self, record: &mut MapPlacementRecord) -> Result<(), String> {
        if record.terrain_patch.is_some() {
            return Ok(());
        } // 洞穴载体已有独立terrain_id，不允许双重变换。
        if let Some(binding) = record.terrain_binding.as_ref() {
            let pose = self.resolve(binding)?;
            record.translation = pose.translation;
            record.yaw_degrees = pose.yaw_degrees;
            record.scale = pose.scale;
        }
        Ok(())
    }
}
