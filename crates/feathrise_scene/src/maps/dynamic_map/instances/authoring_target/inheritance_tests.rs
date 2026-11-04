//! 实例换代实际调用inherit<T>，验证根作者与未选对象都只共享完整持久版本，后续写入各自隔离。

use super::*;
use feathrise_map_content::{
    SparseHeightLiveOverride, SparseHeightSpec, SparseHeightTile, SparseHeightTileKey,
};
use feathrise_scene_core::residency::InstanceResourceTable;
use std::sync::Arc;

type Heights = InstanceResourceTable<SparseHeightLiveOverride>;

#[test]
fn water_owner_exchange_and_candidate_inheritance_keep_the_independent_root_channel() {
    use crate::maps::dynamic_map::water::{DynamicClipmapWaterDraft as Draft, DynamicClipmapWaterDraftSnapshot as Snapshot};
    let map = MapId::parse("MAP-water-independent-inheritance").unwrap();
    let snapshot = |revision| Snapshot { source_revision: revision,
        definition: feathrise_map_content::MapWaterAsset { map_id: map.clone(), ..default() },
        sources: default(), masks: default() };
    let mut world = World::new();
    let old = world.spawn_empty().id();
    let new = world.spawn_empty().id();
    let candidate = world.spawn_empty().id();
    let mut root = Draft::default(); root.publish(snapshot(11)); root.publish_root(snapshot(90));
    let root_revision = root.root_revision_for_map(&map);
    world.insert_resource(root);
    let mut next = Draft::default(); next.publish(snapshot(22));
    world.insert_resource(InstanceResourceTable::<Draft> { values: std::collections::BTreeMap::from([
        (old, Draft::default()), (new, next),
    ]) });
    swap_water_preview(&mut world, Some(old), new);
    let root = world.resource::<Draft>();
    assert_eq!(root.for_map(&map).unwrap().source_revision, 22);
    assert_eq!(root.root_for_map(&map).unwrap().source_revision, 90);
    assert_eq!(root.root_revision_for_map(&map), root_revision);
    let table = world.resource::<InstanceResourceTable<Draft>>();
    assert_eq!(table.values[&old].for_map(&map).unwrap().source_revision, 11);
    assert!(table.values.values().all(|draft| draft.root_for_map(&map).is_none()));
    inherit_water_preview(&mut world, candidate, None, true);
    let inherited = &world.resource::<InstanceResourceTable<Draft>>().values[&candidate];
    assert_eq!(inherited.for_map(&map).unwrap().source_revision, 22);
    assert!(inherited.root_for_map(&map).is_none());
    swap_water_preview(&mut world, None, Entity::PLACEHOLDER);
    assert!(world.resource::<Draft>().for_map(&map).is_none());
    assert_eq!(world.resource::<Draft>().root_for_map(&map).unwrap().source_revision, 90);
}

fn tile(spec: &SparseHeightSpec, key: SparseHeightTileKey, height: f32) -> Arc<SparseHeightTile> {
    let mut tile = SparseHeightTile::blank(spec, key).unwrap();
    tile.set([0, 0], spec.encode_height(height).unwrap())
        .unwrap();
    Arc::new(tile)
}

#[test]
fn live_preview_inheritance_keeps_complete_independent_old_and_new_instances() {
    let spec = SparseHeightSpec::new([0.0; 2], [8192.0; 2], 0.125, -8.0, 16.0).unwrap();
    let map = MapId::parse("MAP-live-instance-clone").unwrap();
    let a = SparseHeightTileKey { x: 1, z: 2 };
    let b = SparseHeightTileKey { x: 90, z: 120 };
    let c = SparseHeightTileKey { x: 220, z: 5 };
    let first = tile(&spec, a, 1.0);
    let shared = tile(&spec, b, 2.0);
    let mut old_value = SparseHeightLiveOverride::default();
    old_value.retarget(Some(map.clone()), 1);
    old_value
        .publish_tiles(&map, 2, [first.clone(), shared.clone()])
        .unwrap();
    let mut world = World::new();
    let old = world.spawn_empty().id();
    let new = world.spawn_empty().id();
    world.init_resource::<Heights>();
    world
        .resource_mut::<Heights>()
        .values
        .insert(old, old_value);
    world
        .resource_mut::<Heights>()
        .values
        .insert(new, SparseHeightLiveOverride::default());
    inherit::<SparseHeightLiveOverride>(&mut world, new, Some(old), false);
    assert_eq!(
        Arc::strong_count(&first),
        2,
        "正式继承调用不得遍历全部块逐一克隆Arc"
    );
    assert_eq!(Arc::strong_count(&shared), 2);
    let replacement = tile(&spec, a, 6.0);
    world
        .resource_mut::<Heights>()
        .values
        .get_mut(&new)
        .unwrap()
        .publish_tiles(&map, 3, [replacement.clone(), tile(&spec, c, 3.0)])
        .unwrap();
    {
        let table = world.resource::<Heights>();
        let old = &table.values[&old];
        let new = &table.values[&new];
        assert_eq!((old.tile_count(), new.tile_count()), (2, 3));
        assert_eq!((old.revision(), new.revision()), (2, 3));
        assert!(Arc::ptr_eq(old.tile(a).unwrap(), &first));
        assert!(Arc::ptr_eq(new.tile(a).unwrap(), &replacement));
        assert!(Arc::ptr_eq(new.tile(b).unwrap(), &shared));
        assert!(old.tile(c).is_none());
    }
    world
        .resource_mut::<Heights>()
        .values
        .get_mut(&old)
        .unwrap()
        .publish_tiles(&map, 4, [tile(&spec, b, 5.0)])
        .unwrap();
    assert!(
        Arc::ptr_eq(
            world.resource::<Heights>().values[&new].tile(b).unwrap(),
            &shared
        ),
        "旧活动实例最后一笔也不能改写新候选继承的版本"
    );
    world.resource_mut::<Heights>().values.remove(&old);
    assert_eq!(
        world.resource::<Heights>().values[&new].tile_count(),
        3,
        "旧上下文退役不影响新实例的完整高度"
    );
}

#[test]
fn live_preview_inheritance_from_selected_root_does_not_copy_or_consume_it() {
    let spec = SparseHeightSpec::new([0.0; 2], [8192.0; 2], 0.125, -8.0, 16.0).unwrap();
    let map = MapId::parse("MAP-live-selected-clone").unwrap();
    let key = SparseHeightTileKey { x: 5, z: 8 };
    let original = tile(&spec, key, 1.0);
    let mut root = SparseHeightLiveOverride::default();
    root.retarget(Some(map.clone()), 1);
    root.publish_tiles(&map, 2, [original.clone()]).unwrap();
    let mut world = World::new();
    let new = world.spawn_empty().id();
    world.insert_resource(root);
    world.init_resource::<Heights>();
    inherit::<SparseHeightLiveOverride>(&mut world, new, None, true);
    assert_eq!(Arc::strong_count(&original), 2);
    let updated = tile(&spec, key, 4.0);
    world
        .resource_mut::<SparseHeightLiveOverride>()
        .publish_tiles(&map, 3, [updated.clone()])
        .unwrap();
    assert!(Arc::ptr_eq(
        world.resource::<Heights>().values[&new].tile(key).unwrap(),
        &original
    ));
    assert!(Arc::ptr_eq(
        world
            .resource::<SparseHeightLiveOverride>()
            .tile(key)
            .unwrap(),
        &updated
    ));
    assert_eq!(world.resource::<Heights>().values[&new].revision(), 2);
}

#[test]
fn live_preview_missing_old_owner_moves_last_height_root_to_existing_retirement() {
    for deleted_old in [false, true] {
        let spec = SparseHeightSpec::new([0.0; 2], [8192.0; 2], 0.125, -8.0, 16.0).unwrap();
        let map = MapId::parse("MAP-live-orphan-retire").unwrap();
        let key = SparseHeightTileKey { x: 5, z: 8 };
        let original = tile(&spec, key, 1.0);
        let weak = Arc::downgrade(&original);
        let mut root = SparseHeightLiveOverride::default();
        root.retarget(Some(map.clone()), 1);
        root.publish_tiles(&map, 2, [original]).unwrap();
        let mut world = World::new();
        let old = world.spawn_empty().id();
        let new = world.spawn_empty().id();
        world.insert_resource(root);
        world.init_resource::<Heights>();
        world.init_resource::<crate::shared::scene_residency::ResidentScenes>();
        world
            .resource_mut::<Heights>()
            .values
            .insert(new, SparseHeightLiveOverride::default());
        if deleted_old {
            world.entity_mut(old).despawn();
        }
        swap_height_preview(&mut world, deleted_old.then_some(old), new);
        assert_eq!(world.resource::<SparseHeightLiveOverride>().tile_count(), 0);
        assert_eq!(world.resource::<Heights>().values[&new].tile_count(), 0);
        assert!(
            weak.upgrade().is_some(),
            "没有新候选持有的最后根也必须留在既有回收队列，不能由交换主帧释放"
        );
    }
}
