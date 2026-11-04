//! 完整来源中的纯TRS/编辑备注增量：保留真实根与玩法组件，结构改变仍交正式候选生命周期。

use bevy::prelude::*;
use feathrise_map_content::{MapId, MapPlacementRecord, MapPlacementsAsset};
use feathrise_scene::MapPlacementInstance;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Prepared {
    pub path: String,
    document: Option<MapPlacementsAsset>,
    updates: Vec<(MapPlacementRecord, Option<Transform>)>,
    resolved: Vec<Entity>,
    cursor: usize,
    verified: bool,
    handle: Option<Handle<MapPlacementsAsset>>,
}

/// 完整比较放在管道线程；任何派生碰撞或建筑净空的语义变化都不能走快捷路径。
pub(super) fn prepare(path: String, old: &[u8], document: MapPlacementsAsset,
    bounds: Rect) -> Option<Prepared>
{
    let previous: MapPlacementsAsset = serde_json::from_slice(old).ok()?;
    if previous.map_id != document.map_id || previous.schema_version != document.schema_version
        || previous.placements.len() != document.placements.len() { return None; }
    let before: BTreeMap<_, _> = previous.placements.iter().map(|record| (&record.id, record)).collect();
    if before.len() != previous.placements.len() { return None; }
    let mut ids = BTreeSet::new();
    let mut updates = Vec::new();
    for record in &document.placements {
        if !ids.insert(&record.id) { return None; }
        let old = *before.get(&record.id)?;
        let mut expected = old.clone();
        expected.translation = record.translation;
        expected.yaw_degrees = record.yaw_degrees;
        expected.scale = record.scale;
        expected.editor = record.editor.clone();
        if expected != *record { return None; }
        if old != record {
            // 变换会改变地形孔洞或建筑净空，必须由完整生产派生链一起提交。
            if (record.terrain_patch.is_some() || record.building_preset_id.is_some())
                && (old.translation != record.translation || old.yaw_degrees != record.yaw_degrees || old.scale != record.scale) {
                return None;
            }
            let changed_transform = old.translation != record.translation || old.yaw_degrees != record.yaw_degrees || old.scale != record.scale;
            let transform = record.validated_transform(&document.map_id, bounds).ok()?;
            updates.push((record.clone(), changed_transform.then_some(transform)));
        }
    }
    Some(Prepared { path, document: Some(document), updates, resolved: Vec::new(), cursor: 0,
        verified: false, handle: None })
}

impl Prepared {
    /// 全体身份先复核再修改。运行根没有当前版本时回退普通loader，不制造部分增量。
    pub(super) fn verify(&mut self, world: &World, map: &MapId) -> Result<bool, ()> {
        if self.verified { return Ok(true); }
        let handle = world.resource::<AssetServer>().get_handle::<MapPlacementsAsset>(self.path.clone()).ok_or(())?;
        if !world.resource::<Assets<MapPlacementsAsset>>().contains(handle.id()) { return Err(()); }
        self.handle = Some(handle);
        let end = (self.cursor + 128).min(self.updates.len());
        for (record, _) in &self.updates[self.cursor..end] {
            let entity = super::preview::active_root(world, map, &record.id).map_err(|_| ())?;
            if world.get::<MapPlacementInstance>(entity).and_then(|root| root.with_updated_transform_record(record)).is_none() { return Err(()); }
            self.resolved.push(entity);
        }
        self.cursor = end;
        if end != self.updates.len() { return Ok(false); }
        self.verified = true;
        self.cursor = 0;
        Ok(true)
    }

    pub(super) fn apply(&mut self, world: &mut World, previews: &mut super::preview::State)
        -> Result<Option<MapPlacementsAsset>, String>
    {
        let end = (self.cursor + 128).min(self.updates.len());
        for index in self.cursor..end {
            let (record, transform) = &self.updates[index];
            let entity = self.resolved[index];
            let root = world.get::<MapPlacementInstance>(entity)
                .filter(|root| root.is_active()).and_then(|root| root.with_updated_transform_record(record))
                .ok_or("增量采用期间对象根已换代")?;
            world.entity_mut(entity).insert(root);
            if let Some(transform) = transform {
                previews.release_placement(world, entity);
                world.entity_mut(entity).insert(*transform);
            }
        }
        self.cursor = end;
        if end != self.updates.len() { return Ok(None); }
        let document = self.document.take().ok_or("对象增量文档已被采用")?;
        let mut assets = world.resource_mut::<Assets<MapPlacementsAsset>>();
        let current = assets.get_mut_untracked(self.handle.as_ref().ok_or("对象增量缺少正式句柄")?)
            .ok_or("对象增量正式资产已卸载")?;
        // 更新正式源而不生成Modified；所有变化根已逐项完成原身份门复核，避免事件触发整批重建。
        Ok(Some(std::mem::replace(current, document)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Component)]
    struct GameplayCounter(u64);
    fn document() -> MapPlacementsAsset {
        serde_json::from_str(r#"{"schema_version":1,"map_id":"MAP-preview","placements":[{"id":"object-a","asset_path":"models/test.glb","translation":[0,0,0],"yaw_degrees":0,"scale":[1,1,1]}]}"#).unwrap()
    }
    fn bounds() -> Rect { Rect::from_corners(Vec2::splat(-100.0), Vec2::splat(100.0)) }
    #[test]
    fn transform_commit_keeps_runtime_entity_children_and_gameplay_state() {
        let before = document();
        let mut after = before.clone(); after.placements[0].translation = [9.0, 2.0, 3.0];
        let mut update = prepare("maps/preview/placements.map_placements.json".into(),
            &serde_json::to_vec(&before).unwrap(), after.clone(), bounds()).unwrap();
        let mut world = World::new();
        let mut assets = Assets::<MapPlacementsAsset>::default();
        update.handle = Some(assets.add(before.clone()));
        world.insert_resource(assets);
        let root = world.spawn((MapPlacementInstance::new(&before.map_id, &before.placements[0]),
            Transform::IDENTITY, GameplayCounter(123))).id();
        let child = world.spawn((ChildOf(root), GameplayCounter(456))).id();
        update.resolved.push(root); update.verified = true;
        update.apply(&mut world, &mut super::super::preview::State::default()).unwrap().unwrap();
        assert_eq!(world.get::<GameplayCounter>(root).unwrap().0, 123);
        assert_eq!(world.get::<GameplayCounter>(child).unwrap().0, 456);
        assert_eq!(world.get::<ChildOf>(child).unwrap().parent(), root);
        assert_eq!(world.get::<Transform>(root).unwrap().translation, Vec3::new(9.0, 2.0, 3.0));
        assert!(world.get::<MapPlacementInstance>(root).unwrap().matches_record(&after.map_id, &after.placements[0], &[]));
    }
    #[test]
    fn shape_change_and_derived_collision_change_keep_the_formal_candidate_path() {
        let before = document(); let old = serde_json::to_vec(&before).unwrap();
        let mut changed = before.clone(); changed.placements[0].asset_path = "models/other.glb".into();
        assert!(prepare("x".into(), &old, changed, bounds()).is_none());
        let mut before = before; before.placements[0].building_preset_id = Some("house".into());
        let mut changed = before.clone(); changed.placements[0].translation[0] = 10.0;
        assert!(prepare("x".into(), &serde_json::to_vec(&before).unwrap(), changed, bounds()).is_none());
    }
}
