//! 持久实时索引的版本隔离、常数捕获、空间裁剪及清图所有权回归；只创建有限测试块。

use super::*;
use crate::{SparseHeightLiveOverride, SparseHeightSpec};
use std::collections::BTreeMap;

fn spec() -> SparseHeightSpec {
    SparseHeightSpec::new([0.0; 2], [8192.0; 2], 0.125, -8.0, 16.0).unwrap()
}

fn tile(spec: &SparseHeightSpec, key: SparseHeightTileKey, meters: f32) -> Arc<SparseHeightTile> {
    let mut tile = SparseHeightTile::blank(spec, key).unwrap();
    tile.set([0, 0], spec.encode_height(meters).unwrap())
        .unwrap();
    Arc::new(tile)
}

fn selected(
    snapshot: &SparseHeightLiveSnapshot,
    level: u8,
    key: SparseHeightTileKey,
) -> BTreeMap<SparseHeightTileKey, Arc<SparseHeightTile>> {
    let mut result = BTreeMap::new();
    snapshot.visit_mip_tiles(level, key, |key, tile| {
        result.insert(key, Arc::clone(tile));
    });
    result
}

#[test]
fn live_snapshot_root_capture_is_constant_and_old_values_survive_later_updates() {
    let spec = spec();
    let map = MapId::parse("MAP-live-root-snapshot").unwrap();
    let a = SparseHeightTileKey { x: 3, z: 5 };
    let b = SparseHeightTileKey { x: 190, z: 230 };
    let c = SparseHeightTileKey { x: 80, z: 2 };
    let original = tile(&spec, a, 2.0);
    let unchanged = tile(&spec, b, 3.0);
    let mut live = SparseHeightLiveOverride::default();
    live.retarget(Some(map.clone()), 1);
    live.publish_tiles(&map, 2, [original.clone(), unchanged.clone()])
        .unwrap();
    let before = live.snapshot();
    let captured_again = live.snapshot();
    let clone = before.clone();
    assert!(Arc::ptr_eq(
        before.root.as_ref().unwrap(),
        captured_again.root.as_ref().unwrap()
    ));
    assert!(
        Arc::ptr_eq(before.root.as_ref().unwrap(), clone.root.as_ref().unwrap()),
        "捕获和Clone只共享根，不能复制整张作者键表或高度像素"
    );
    let replacement = tile(&spec, a, 6.0);
    INDEX_WRITES.with(|count| count.set(0));
    live.publish_tiles(&map, 3, [replacement.clone(), tile(&spec, c, 1.0)])
        .unwrap();
    assert_eq!(
        INDEX_WRITES.with(|count| count.get()),
        2 * (u32::BITS as usize + 1),
        "每个触及键只访问固定32层路径和叶，不受全图块数或旧快照数量影响"
    );
    let after = live.snapshot();
    assert!(!Arc::ptr_eq(
        before.root.as_ref().unwrap(),
        after.root.as_ref().unwrap()
    ));
    assert_eq!(before.tile_count(), 2);
    assert_eq!(before.revision(), 2);
    assert_eq!(after.tile_count(), 3);
    assert_eq!(after.revision(), 3);
    assert!(Arc::ptr_eq(&selected(&before, 0, a)[&a], &original));
    assert!(Arc::ptr_eq(&selected(&after, 0, a)[&a], &replacement));
    assert!(
        Arc::ptr_eq(&selected(&after, 0, b)[&b], &unchanged),
        "更新只复制触及地址的固定路径，其他块与旧快照共享原Arc"
    );
    assert!(
        selected(&before, 0, c).is_empty(),
        "后来的新块不能渗入在途旧快照"
    );
}

#[test]
fn live_snapshot_mip_queries_visit_only_the_exact_requested_ancestor() {
    let spec = spec();
    let map = MapId::parse("MAP-live-root-prefix").unwrap();
    let keys = [
        (0, 0),
        (1, 0),
        (2, 0),
        (3, 3),
        (4, 4),
        (8, 2),
        (9, 3),
        (254, 255),
        (255, 255),
    ]
    .map(|(x, z)| SparseHeightTileKey { x, z });
    let mut live = SparseHeightLiveOverride::default();
    live.retarget(Some(map.clone()), 1);
    live.publish_tiles(&map, 2, keys.map(|key| tile(&spec, key, 1.0)))
        .unwrap();
    let snapshot = live.snapshot();
    QUERY_VISITS.with(|count| count.set(0));
    assert_eq!(selected(&snapshot, 0, keys[0]).len(), 1);
    assert_eq!(
        QUERY_VISITS.with(|count| count.get()),
        u32::BITS as usize + 1,
        "基础页查询只访问固定前缀和目标叶，不能扫描无关作者块"
    );
    for level in 0..=spec.max_mip_level() {
        for key in keys {
            let ancestor = spec.mip_ancestor_key(key, level).unwrap();
            let expected = keys
                .into_iter()
                .filter(|key| spec.mip_ancestor_key(*key, level) == Some(ancestor))
                .collect::<Vec<_>>();
            let actual = selected(&snapshot, level, ancestor);
            assert_eq!(
                actual.keys().copied().collect::<Vec<_>>(),
                expected,
                "L{level} {ancestor:?}"
            );
        }
    }
    assert!(selected(&snapshot, 0, SparseHeightTileKey { x: 5, z: 5 }).is_empty());
    assert_eq!(
        selected(&snapshot, 32, SparseHeightTileKey { x: 0, z: 0 }).len(),
        keys.len()
    );
    assert!(selected(&snapshot, 32, SparseHeightTileKey { x: 1, z: 0 }).is_empty());
    assert!(selected(&snapshot, 33, SparseHeightTileKey { x: 0, z: 0 }).is_empty());
}

#[test]
fn live_snapshot_bad_batches_clear_and_retarget_never_rewrite_frozen_versions() {
    let spec = spec();
    let map = MapId::parse("MAP-live-root-owner").unwrap();
    let other = MapId::parse("MAP-live-root-other").unwrap();
    let a = SparseHeightTileKey { x: 1, z: 1 };
    let b = SparseHeightTileKey { x: 6, z: 7 };
    let mut live = SparseHeightLiveOverride::default();
    live.retarget(Some(map.clone()), 1);
    live.publish_tiles(&map, 2, [tile(&spec, a, 2.0)]).unwrap();
    let before = live.snapshot();
    let duplicate = tile(&spec, b, 5.0);
    assert!(
        live.publish_tiles(&map, 3, [duplicate.clone(), duplicate.clone()])
            .is_err()
    );
    assert!(live.publish_tiles(&map, 1, [duplicate.clone()]).is_err());
    assert!(live.publish_tiles(&other, 3, [duplicate.clone()]).is_err());
    let unchanged = live.snapshot();
    assert_eq!(unchanged.revision(), 2);
    assert!(Arc::ptr_eq(
        before.root.as_ref().unwrap(),
        unchanged.root.as_ref().unwrap()
    ));
    assert!(!live.has_mip_tiles(0, b));
    live.clear(4);
    assert!(live.snapshot().root.is_none());
    assert_eq!(live.snapshot().tile_count(), 0);
    assert_eq!(before.map_id(), Some(&map));
    assert_eq!(
        selected(&before, 32, SparseHeightTileKey { x: 0, z: 0 }).len(),
        1
    );
    live.retarget(Some(other.clone()), 5);
    live.publish_tiles(&other, 6, [duplicate]).unwrap();
    let after = live.snapshot();
    assert_eq!(after.map_id(), Some(&other));
    assert!(selected(&after, 0, a).is_empty());
    assert!(selected(&before, 0, b).is_empty());
    live.retarget(Some(other), 7);
    assert!(
        Arc::ptr_eq(
            after.root.as_ref().unwrap(),
            live.snapshot().root.as_ref().unwrap()
        ),
        "同地图只更新修订，不能清空尚未保存的持久块"
    );
    live.retarget(Some(map), 8);
    assert_eq!(live.tile_count(), 0);
    assert!(live.snapshot().root.is_none());
    assert_eq!(
        selected(&after, 0, b).len(),
        1,
        "清图只交还所有权，旧worker仍读取冻结地图与像素"
    );
}

#[test]
fn live_snapshot_resource_clone_and_both_next_writes_share_only_fixed_paths() {
    let spec = spec();
    let map = MapId::parse("MAP-live-instance-inherit").unwrap();
    let a = SparseHeightTileKey { x: 1, z: 2 };
    let b = SparseHeightTileKey { x: 90, z: 120 };
    let c = SparseHeightTileKey { x: 220, z: 5 };
    let original = tile(&spec, a, 1.0);
    let shared = tile(&spec, b, 2.0);
    let mut old = SparseHeightLiveOverride::default();
    old.retarget(Some(map.clone()), 1);
    old.publish_tiles(&map, 2, [original.clone(), shared.clone()])
        .unwrap();
    INDEX_WRITES.with(|count| count.set(0));
    QUERY_VISITS.with(|count| count.set(0));
    let mut next = old.clone(); // inherit<T>::cloned的真实资源Clone，不能只测试snapshot的小包装。
    assert_eq!(INDEX_WRITES.with(|count| count.get()), 0);
    assert_eq!(QUERY_VISITS.with(|count| count.get()), 0);
    assert!(Arc::ptr_eq(
        old.index.root.as_ref().unwrap(),
        next.index.root.as_ref().unwrap()
    ));
    assert_eq!(
        Arc::strong_count(&original),
        2,
        "继承只共享根，不能逐块克隆Arc"
    );
    let frozen = old.snapshot();
    let replacement = tile(&spec, a, 6.0);
    next.publish_tiles(&map, 3, [replacement.clone(), tile(&spec, c, 3.0)])
        .unwrap();
    assert_eq!(
        INDEX_WRITES.with(|count| count.get()),
        2 * (u32::BITS as usize + 1)
    );
    assert!(Arc::ptr_eq(old.tile(a).unwrap(), &original));
    assert!(Arc::ptr_eq(next.tile(a).unwrap(), &replacement));
    assert!(Arc::ptr_eq(next.tile(b).unwrap(), &shared));
    assert!(old.tile(c).is_none());
    assert_eq!((old.tile_count(), next.tile_count()), (2, 3));
    INDEX_WRITES.with(|count| count.set(0));
    old.publish_tiles(&map, 4, [tile(&spec, b, 5.0)]).unwrap();
    assert_eq!(
        INDEX_WRITES.with(|count| count.get()),
        u32::BITS as usize + 1
    );
    assert_eq!(
        (old.revision(), next.revision(), frozen.revision()),
        (4, 3, 2)
    );
    assert!(Arc::ptr_eq(next.tile(b).unwrap(), &shared));
    assert!(Arc::ptr_eq(&selected(&frozen, 0, b)[&b], &shared));
}

#[test]
fn live_snapshot_float_height_bounds_are_cached_per_root_and_follow_atomic_replacement() {
    let spec = spec();
    let map = MapId::parse("MAP-live-float-bounds").unwrap();
    let a = SparseHeightTileKey { x: 1, z: 2 };
    let b = SparseHeightTileKey { x: 90, z: 120 };
    let mut live = SparseHeightLiveOverride::default();
    live.retarget(Some(map.clone()), 1);
    assert_eq!(live.height_bounds(), None);
    live.publish_tiles(&map, 2, [tile(&spec, a, 1000.0), tile(&spec, b, -400.0)]).unwrap();
    let frozen = live.snapshot();
    INDEX_WRITES.with(|count| count.set(0));
    QUERY_VISITS.with(|count| count.set(0));
    for _ in 0..64 {
        assert_eq!(live.height_bounds(), Some([-400.0, 1000.0]));
        assert_eq!(frozen.height_bounds(), Some([-400.0, 1000.0]));
    }
    assert_eq!(QUERY_VISITS.with(|count| count.get()), 0, "范围只读根摘要，不遍历整树或任何高度像素");
    assert_eq!(INDEX_WRITES.with(|count| count.get()), 0);
    live.publish_tiles(&map, 3, [tile(&spec, a, 2000.0)]).unwrap();
    assert_eq!(live.height_bounds(), Some([-400.0, 2000.0]));
    assert_eq!(frozen.height_bounds(), Some([-400.0, 1000.0]));
    live.publish_tiles(&map, 4, [Arc::new(SparseHeightTile::blank(&spec, b).unwrap())]).unwrap();
    assert_eq!(live.height_bounds(), Some([0.0, 2000.0]));
    live.clear(5);
    assert_eq!(live.height_bounds(), None);
    assert_eq!(frozen.height_bounds(), Some([-400.0, 1000.0]), "清图/新修订不能污染仍在worker使用的旧范围");
}

#[test]
fn live_snapshot_successor_and_exact_iterator_cover_all_quadrants_and_high_bits() {
    let sample = tile(&spec(), SparseHeightTileKey { x: 0, z: 0 }, 1.0);
    let keys = [
        (0, 0),
        (1, 0),
        (0, 1),
        (1, 1),
        (2, 0),
        (0, 2),
        (1 << 31, 0),
        (u32::MAX, 0),
        (0, 1 << 31),
        (1 << 31, 1 << 31),
        (u32::MAX, u32::MAX),
    ]
    .map(|(x, z)| SparseHeightTileKey { x, z });
    let mut index = LiveTileIndex::default();
    for key in keys {
        index.insert(key, sample.clone());
    }
    let mut iter = index.tiles();
    let mut expected = Vec::new();
    for remaining in (1..=keys.len()).rev() {
        assert_eq!(iter.len(), remaining);
        assert_eq!(iter.size_hint(), (remaining, Some(remaining)));
        expected.push(iter.next().unwrap().0);
    }
    assert_eq!(iter.len(), 0);
    assert!(iter.next().is_none());
    assert!(iter.next().is_none());
    let mut actual = Vec::new();
    let mut cursor = None;
    loop {
        QUERY_VISITS.with(|count| count.set(0));
        let next = index.tile_after(cursor);
        assert!(
            QUERY_VISITS.with(|count| count.get()) <= 2 * (u32::BITS as usize + 1),
            "后继查询只走游标及第一条后继路径，不遍历其他象限"
        );
        let Some((key, _)) = next else {
            break;
        };
        actual.push(key);
        cursor = Some(key);
    }
    assert_eq!(
        actual, expected,
        "游标与完整确定性遍历必须不漏不重且顺序一致"
    );
    let set = actual
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(set, keys.into_iter().collect());
    for key in keys {
        assert!(Arc::ptr_eq(index.tile(key).unwrap(), &sample));
    }
    let missing = SparseHeightTileKey { x: 100, z: 0 };
    assert!(index.tile(missing).is_none());
    let mut rank = keys.to_vec();
    rank.push(missing);
    let mut ordered = LiveTileIndex::default();
    for key in rank {
        ordered.insert(key, sample.clone());
    }
    let expected_after_missing = ordered
        .tiles()
        .map(|(key, _)| key)
        .skip_while(|key| *key != missing)
        .nth(1);
    assert_eq!(
        index.tile_after(Some(missing)).map(|(key, _)| key),
        expected_after_missing,
        "游标块后来缺失也应定位严格后继，不能从头重复"
    );
    assert_eq!(LiveTileIndex::default().tiles().len(), 0);
    assert!(LiveTileIndex::default().tile_after(None).is_none());
}
