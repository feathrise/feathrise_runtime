//! 跨网格植被预览按稳定路径原子发布；检查器选择不决定其他网格的作者来源。
use super::super::vegetation::{DynamicBladeDraft, DynamicBladeDraftSnapshot};
use super::{TerrainObjectAuthoringTarget, resources::SubMapRuntime};
use bevy::{ecs::system::SystemParam, prelude::*};
use feathrise_map_content::{MapId, MapInstancePath, MapVegetationKind, TerrainId};
use feathrise_scene_core::residency::InstanceResourceTable;

#[derive(SystemParam)]
pub struct TerrainVegetationDrafts<'w> {
    runtime: Option<Res<'w, SubMapRuntime>>,
    target: Option<Res<'w, TerrainObjectAuthoringTarget>>,
    root: Option<ResMut<'w, DynamicBladeDraft>>,
    table: Option<ResMut<'w, InstanceResourceTable<DynamicBladeDraft>>>,
}
impl TerrainVegetationDrafts<'_> {
    fn destination(&self, map: &MapId, id: &TerrainId) -> Option<Option<Entity>> {
        let target = self.target.as_ref()?;
        if target.applied_to(map, id) && self.root.is_some() {
            return Some(None);
        }
        let entity = self
            .runtime
            .as_ref()?
            .active_entity(&MapInstancePath::root(map.clone()).terrain(id.clone()))?;
        self.table
            .as_ref()?
            .values
            .contains_key(&entity)
            .then_some(Some(entity))
    }
    /// 所有成员资源存在才同次发布；上传尚未齐备、实例换代时保留上一有效草地。
    pub fn publish(
        &mut self,
        map: &MapId,
        snapshots: Vec<(TerrainId, DynamicBladeDraftSnapshot)>,
    ) -> Result<(), Vec<(TerrainId, DynamicBladeDraftSnapshot)>> {
        let Some(destinations) = snapshots
            .iter()
            .map(|(id, _)| self.destination(map, id))
            .collect::<Option<Vec<_>>>()
        else {
            return Err(snapshots);
        };
        for ((_, snapshot), destination) in snapshots.into_iter().zip(destinations) {
            let output = if let Some(entity) = destination {
                self.table
                    .as_mut()
                    .unwrap()
                    .values
                    .get_mut(&entity)
                    .unwrap()
            } else {
                self.root.as_deref_mut().unwrap()
            };
            let density_only = output.for_map(map).is_some_and(|old| {
                old.definition.layers == snapshot.definition.layers
                    && snapshot
                        .definition
                        .layers
                        .iter()
                        .all(|l| l.kind != MapVegetationKind::Mesh)
            });
            if density_only {
                output.publish_density(snapshot);
            } else {
                output.publish(snapshot);
            }
        }
        Ok(())
    }
    pub fn ready(&self, map: &MapId, ids: impl Iterator<Item = TerrainId>) -> bool {
        ids.into_iter()
            .all(|id| self.destination(map, &id).is_some())
    }

    /// 只冻结目标的共享来源，供跨进程预览归还自身覆盖并把旧像素交后台回收。
    pub fn shared(&self, map: &MapId, id: &TerrainId) -> Option<std::sync::Arc<DynamicBladeDraftSnapshot>> {
        let draft = match self.destination(map, id)? {
            Some(entity) => self.table.as_ref()?.values.get(&entity)?,
            None => self.root.as_deref()?,
        };
        draft.frozen_for_map(map)
    }

    /// 完整作者来源接回预览时，按稳定terrain地址撤销覆盖，不扫描全体地图实例。
    pub fn clear(&mut self, map: &MapId, ids: impl Iterator<Item = TerrainId>) {
        for id in ids {
            match self.destination(map, &id) {
                Some(Some(entity)) => self.table.as_mut().unwrap().values.get_mut(&entity).unwrap().clear(),
                Some(None) => self.root.as_deref_mut().unwrap().clear(),
                None => {} // 已离场目标不再拥有任何覆盖发布权。
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
    };
    #[derive(Resource)]
    struct Request {
        map: MapId,
        values: Vec<(TerrainId, DynamicBladeDraftSnapshot)>,
        accepted: bool,
    }
    fn publish(mut request: ResMut<Request>, mut outputs: TerrainVegetationDrafts) {
        let values = std::mem::take(&mut request.values);
        request.accepted = outputs.publish(&request.map, values).is_ok();
    }
    fn snapshot(map: &MapId, revision: u64) -> DynamicBladeDraftSnapshot {
        DynamicBladeDraftSnapshot {
            source_revision: revision,
            definition: feathrise_map_content::MapVegetationAsset {
                schema_version: 1,
                map_id: map.clone(),
                ..default()
            },
            sources: BTreeMap::from([("grass".into(), Arc::from([revision as u8]))]),
            densities: BTreeMap::new(),
            nonzero_layers: BTreeSet::new(),
        }
    }
    #[test]
    fn cross_mesh_draft_publication_is_atomic_and_survives_inspector_owner_swap() {
        let mut world = World::new();
        let map = MapId::parse("MAP-grass-group").unwrap();
        let a = TerrainId::parse("a").unwrap();
        let b = TerrainId::parse("b").unwrap();
        let root = world.spawn_empty().id();
        let first = world.spawn_empty().id();
        let second = world.spawn_empty().id();
        world.insert_resource(SubMapRuntime::test_active(
            root,
            BTreeMap::from([
                (MapInstancePath::root(map.clone()).terrain(a.clone()), first),
                (
                    MapInstancePath::root(map.clone()).terrain(b.clone()),
                    second,
                ),
            ]),
        ));
        world.init_resource::<DynamicBladeDraft>();
        let mut table = InstanceResourceTable::<DynamicBladeDraft>::default();
        table.values.insert(first, default());
        table.values.insert(second, default());
        world.insert_resource(table);
        let mut target = TerrainObjectAuthoringTarget::default();
        target.map = Some(map.clone());
        target.terrain = Some(a.clone());
        world.insert_resource(target);
        super::super::authoring_target::sync_preview_owner(&mut world);
        world.insert_resource(Request {
            map: map.clone(),
            values: vec![
                (a.clone(), snapshot(&map, 11)),
                (b.clone(), snapshot(&map, 11)),
            ],
            accepted: false,
        });
        world.run_system_once(publish).unwrap();
        assert!(world.resource::<Request>().accepted);
        assert_eq!(
            world
                .resource::<DynamicBladeDraft>()
                .for_map(&map)
                .unwrap()
                .source_revision,
            11
        );
        assert_eq!(
            world
                .resource::<InstanceResourceTable<DynamicBladeDraft>>()
                .values[&second]
                .for_map(&map)
                .unwrap()
                .source_revision,
            11
        );
        world.resource_mut::<TerrainObjectAuthoringTarget>().terrain = Some(b.clone());
        super::super::authoring_target::sync_preview_owner(&mut world);
        assert_eq!(
            world
                .resource::<DynamicBladeDraft>()
                .for_map(&map)
                .unwrap()
                .source_revision,
            11
        );
        assert_eq!(
            world
                .resource::<InstanceResourceTable<DynamicBladeDraft>>()
                .values[&first]
                .for_map(&map)
                .unwrap()
                .source_revision,
            11
        );
        world
            .resource_mut::<InstanceResourceTable<DynamicBladeDraft>>()
            .values
            .remove(&first);
        world.resource_mut::<Request>().values =
            vec![(b, snapshot(&map, 12)), (a, snapshot(&map, 12))];
        world.run_system_once(publish).unwrap();
        assert!(!world.resource::<Request>().accepted);
        assert_eq!(
            world
                .resource::<DynamicBladeDraft>()
                .for_map(&map)
                .unwrap()
                .source_revision,
            11,
            "缺一个成员不能先覆盖当前网格"
        );
    }
}
