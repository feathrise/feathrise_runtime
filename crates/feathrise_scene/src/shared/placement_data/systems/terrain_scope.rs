//! 来源域的摆放筛选合同；旧图三维地形与普通摆放在迁移后分别只拥有一个宿主。
use bevy::{ecs::system::SystemParam, prelude::*};
use feathrise_map_content::{MapId, MapInstanceAuthorPreview, MapManifestAsset};
use feathrise_scene_core::residency::{InstanceFrame, InstanceRes};
use super::super::resources::TerrainPlacementFilter;

#[derive(SystemParam)]
pub(crate) struct TerrainDomainInputs<'w, 's> {
    preview: Option<InstanceRes<'w, MapInstanceAuthorPreview>>,
    frame: InstanceFrame<'w>,
    objects: Query<'w, 's, &'static crate::TerrainObjectInstance>,
    queries: crate::TerrainObjectQueries<'w, 's>,
}
impl TerrainDomainInputs<'_, '_> {
    pub(super) fn is_instance_domain(&self) -> bool { self.frame.instance().is_some() }
    /// 内容根仍归地图容器；仅解析其关联地形姿态，再由既有InstanceFrame叠加子地图整体变换。
    pub(super) fn binding_pose(&self, binding: &feathrise_map_content::MapTerrainBinding, map: &MapId)
        -> Result<feathrise_map_content::TerrainObjectTransform, String> {
        self.queries.binding_pose(map,self.frame.instance(),binding)
    }
    pub(super) fn filter(&self, manifest: &MapManifestAsset, map: &MapId) -> TerrainPlacementFilter {
        if let Some(object) = self.frame.instance().and_then(|entity| self.objects.get(entity).ok()) {
            TerrainPlacementFilter::Terrain(object.definition.id.clone())
        } else if manifest.terrains.is_some() || self.preview.as_deref().is_some_and(|preview|
            preview.map_id.as_ref() == Some(map) && preview.terrains.is_some()) {
            TerrainPlacementFilter::Container
        } else { TerrainPlacementFilter::All }
    }
}
