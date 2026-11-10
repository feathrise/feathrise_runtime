//! 保存交接按稳定地形路径撤回内存水网覆盖；其他未保存画布继续使用自己的作者草稿。
use super::super::water::DynamicClipmapWaterDraft;
use super::{TerrainObjectAuthoringTarget, resources::SubsceneRuntime};
use bevy::{ecs::system::SystemParam, prelude::*};
use feathrise_scene_content::{SceneId, SceneInstancePath, TerrainId};
use feathrise_scene_core::residency::InstanceResourceTable;

#[derive(SystemParam)]
pub struct TerrainWaterDrafts<'w> {
    runtime: Option<Res<'w, SubsceneRuntime>>,
    target: Option<Res<'w, TerrainObjectAuthoringTarget>>,
    root: Option<ResMut<'w, DynamicClipmapWaterDraft>>,
    table: Option<ResMut<'w, InstanceResourceTable<DynamicClipmapWaterDraft>>>,
}
impl TerrainWaterDrafts<'_> {
    fn destination(&self, map: &SceneId, id: &TerrainId) -> Option<Option<Entity>> {
        if self.target.as_ref().is_some_and(|target| target.applied_to(map, id)) && self.root.is_some() {
            return Some(None);
        }
        let entity = self.runtime.as_ref()?.active_entity(&SceneInstancePath::root(map.clone()).terrain(id.clone()))?;
        self.table.as_ref()?.values.contains_key(&entity).then_some(Some(entity))
    }

    /// 实时桥和原作者共用正式水草稿槽，稳定路径定位不扫描实例表。
    pub fn ready(&self, map: &SceneId, id: &TerrainId) -> bool { self.destination(map, id).is_some() }

    pub fn shared(&self, map: &SceneId, id: &TerrainId) -> Option<std::sync::Arc<super::super::water::DynamicClipmapWaterDraftSnapshot>> {
        match self.destination(map, id)? {
            Some(entity) => self.table.as_ref()?.values.get(&entity)?.shared_for_scene(map),
            None => self.root.as_ref()?.shared_for_scene(map),
        }
    }

    pub fn publish(&mut self, map: &SceneId, id: &TerrainId,
        snapshot: super::super::water::DynamicClipmapWaterDraftSnapshot)
        -> Result<(), super::super::water::DynamicClipmapWaterDraftSnapshot> {
        let Some(destination) = self.destination(map, id) else { return Err(snapshot); };
        let output = match destination {
            Some(entity) => self.table.as_mut().unwrap().values.get_mut(&entity).unwrap(),
            None => self.root.as_deref_mut().unwrap(),
        };
        output.publish(snapshot);
        Ok(())
    }
    /// 实时桥只归还自己确切发布的Arc；候选同修订外部来源也不能借活动根的信用被清除。
    pub fn clear_owned(&mut self, map: &SceneId, id: &TerrainId,
        owned: &std::sync::Arc<super::super::water::DynamicClipmapWaterDraftSnapshot>) {
        let clear = |draft: &mut DynamicClipmapWaterDraft| {
            if draft.shared_for_scene(map).is_some_and(|current| std::sync::Arc::ptr_eq(&current, owned)) { draft.clear(); }
        };
        if self.target.as_ref().is_some_and(|target| target.applied_to(map, id))
            && let Some(root) = self.root.as_deref_mut() { clear(root); }
        let path = SceneInstancePath::root(map.clone()).terrain(id.clone());
        let entities = self.runtime.as_ref().map_or([None; 2], |runtime| [runtime.active_entity(&path),
            runtime.candidate.as_ref().and_then(|batch| batch.index.get(&path).copied())]);
        if let Some(table) = self.table.as_deref_mut() {
            for entity in entities.into_iter().flatten() {
                if let Some(draft) = table.values.get_mut(&entity) { clear(draft); }
            }
        } // 只有三个确定槽位；owned仍由调用方持有，最后一份大像素必须由其后台回收。
    }
    /// 保存门存续期间持续调用；换代候选继承的旧草稿也必须撤回，才能让正式JSON/R8亲自完成采用。
    pub fn clear_saved(&mut self, map: &SceneId, ids: &[TerrainId]) {
        for id in ids {
            if self.target.as_ref().is_some_and(|target| target.applied_to(map, id))
                && let Some(root) = self.root.as_deref_mut()
                && root.for_scene(map).is_some() {
                root.clear();
            }
            let path = SceneInstancePath::root(map.clone()).terrain(id.clone());
            let entities = self.runtime.as_ref().map_or([None; 2], |runtime| [
                runtime.active_entity(&path),
                runtime.candidate.as_ref().and_then(|batch| batch.index.get(&path).copied()),
            ]);
            if let Some(table) = self.table.as_deref_mut() {
                for entity in entities.into_iter().flatten() {
                    if let Some(draft) = table.values.get_mut(&entity)
                        && draft.for_scene(map).is_some() {
                        draft.clear();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use std::collections::BTreeMap;
    #[derive(Resource)]
    struct Saved { map: SceneId, ids: Vec<TerrainId> }
    fn clear(saved: Res<Saved>, mut drafts: TerrainWaterDrafts) { drafts.clear_saved(&saved.map, &saved.ids); }
    fn draft(map: &SceneId, revision: u64) -> DynamicClipmapWaterDraft {
        let mut draft = DynamicClipmapWaterDraft::default();
        draft.publish(super::super::super::water::DynamicClipmapWaterDraftSnapshot {
            source_revision: revision,
            definition: feathrise_scene_content::SceneWaterAsset { scene_id: map.clone(), ..default() },
            sources: BTreeMap::new(), masks: BTreeMap::new(),
        });
        draft
    }
    #[test]
    fn water_preview_clear_owned_keeps_same_revision_external_candidate() {
        #[derive(Resource)] struct Owned { map: SceneId, id: TerrainId, snapshot: std::sync::Arc<super::super::super::water::DynamicClipmapWaterDraftSnapshot> }
        fn clear_owned(owned: Res<Owned>, mut drafts: TerrainWaterDrafts) { drafts.clear_owned(&owned.map, &owned.id, &owned.snapshot); }
        let mut world = World::new();
        let map = SceneId::parse("MAP-water-owned").unwrap(); let id = TerrainId::parse("canvas").unwrap();
        let batch = world.spawn_empty().id(); let active = world.spawn_empty().id(); let candidate = world.spawn_empty().id();
        let path = SceneInstancePath::root(map.clone()).terrain(id.clone());
        let mut runtime = SubsceneRuntime::test_active(batch, BTreeMap::from([(path.clone(), active)]));
        runtime.candidate = SubsceneRuntime::test_active(batch, BTreeMap::from([(path, candidate)])).active;
        let value = draft(&map, 8); let owned = value.shared_for_scene(&map).unwrap();
        let external = draft(&map, 8); let external_snapshot = external.shared_for_scene(&map).unwrap();
        world.insert_resource(runtime);
        world.insert_resource(InstanceResourceTable::<DynamicClipmapWaterDraft> { values: BTreeMap::from([(active,value),(candidate,external)]) });
        world.insert_resource(Owned { map: map.clone(), id, snapshot: owned });
        world.run_system_once(clear_owned).unwrap();
        let table = world.resource::<InstanceResourceTable<DynamicClipmapWaterDraft>>();
        assert!(table.values[&active].for_scene(&map).is_none());
        assert!(std::sync::Arc::ptr_eq(&table.values[&candidate].shared_for_scene(&map).unwrap(), &external_snapshot));
    }
    #[test]
    fn saved_water_clears_only_its_current_root_and_active_or_preparing_paths() {
        let mut world = World::new();
        let map = SceneId::parse("MAP-water-save-drafts").unwrap();
        let ids = ["saved", "dirty-sibling"].map(|id| TerrainId::parse(id).unwrap());
        let batch = world.spawn_empty().id();
        let entities: [Entity; 4] = std::array::from_fn(|_| world.spawn_empty().id());
        let index = |start| BTreeMap::from([
            (SceneInstancePath::root(map.clone()).terrain(ids[0].clone()), entities[start]),
            (SceneInstancePath::root(map.clone()).terrain(ids[1].clone()), entities[start + 1]),
        ]);
        let mut runtime = SubsceneRuntime::test_active(batch, index(0));
        runtime.candidate = SubsceneRuntime::test_active(batch, index(2)).active;
        world.insert_resource(runtime);
        world.insert_resource(TerrainObjectAuthoringTarget::test_applied(map.clone(), ids[0].clone(), entities[0]));
        world.insert_resource(draft(&map, 11));
        world.insert_resource(InstanceResourceTable::<DynamicClipmapWaterDraft> {
            values: entities.into_iter().enumerate().map(|(index, entity)| (entity, draft(&map, index as u64 + 20))).collect(),
        });
        world.insert_resource(Saved { map: map.clone(), ids: vec![ids[0].clone()] });
        world.run_system_once(clear).unwrap();
        assert!(world.resource::<DynamicClipmapWaterDraft>().for_scene(&map).is_none());
        for index in [0, 2] {
            assert!(world.resource::<InstanceResourceTable<DynamicClipmapWaterDraft>>().values[&entities[index]].for_scene(&map).is_none());
        }
        for index in [1, 3] {
            assert_eq!(world.resource::<InstanceResourceTable<DynamicClipmapWaterDraft>>().values[&entities[index]]
                .for_scene(&map).unwrap().source_revision, index as u64 + 20);
        }
        // 新候选后来继承到旧覆盖时，持续保存门再次精确撤回，不需要新建交接资源。
        world.resource_mut::<InstanceResourceTable<DynamicClipmapWaterDraft>>().values.insert(entities[2], draft(&map, 99));
        world.run_system_once(clear).unwrap();
        assert!(world.resource::<InstanceResourceTable<DynamicClipmapWaterDraft>>().values[&entities[2]].for_scene(&map).is_none());
    }
}
