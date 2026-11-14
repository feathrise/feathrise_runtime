//! 不启动GPU的F12证据门回归：模板计数、过期快照和兄弟实例均不能取得三维信用。

use super::*;
use std::collections::BTreeMap;

fn fixture() -> (World, Entity, DynamicVegetationStatus, Requirements, Vec<Entity>) {
    let mut world = World::new();
    let scene = SceneId::parse("volume-fixture").unwrap();
    let instance = world.spawn((Visibility::Inherited,
        feathrise_scene_core::residency::SceneSourceFrame::default())).id();
    let root = world.spawn((ChildOf(instance), Visibility::Inherited)).id();
    let field = world.spawn((ChildOf(instance), Visibility::Inherited, VolumeSurfaceFields::default())).id();
    let snapshot = Arc::as_ptr(&world.get::<VolumeSurfaceFields>(field).unwrap().0) as usize;
    let target = SavedTerrain { id: TerrainId::parse("thin-plate").unwrap(), source_path: "source.json".into(),
        layers: vec![SavedLayer { id: "top".into(), runtime_id: 11 }, SavedLayer { id: "bottom".into(), runtime_id: 12 }] };
    let mut windows = Vec::new();
    for layer in &target.layers {
        windows.push(world.spawn((ChildOf(root), Visibility::Inherited,
            VegetationBladeLayer(feathrise_vegetation::VegetationLayerId(layer.runtime_id)),
            GpuBladeVolumeSource(Handle::default()), GpuBladeSourceRevision(7),
            GpuBladeDensityMap::new(Handle::default()),
            GpuBladePatch::square(Vec3::ZERO, 1., 2, 1).with_volume_source(3),
            Transform::IDENTITY, GlobalTransform::IDENTITY)).id());
    }
    let status = DynamicVegetationStatus { scene_id: Some(scene.clone()), phase: DynamicVegetationPhase::Ready,
        blade_window_count: 8, volume_ready: true, volume_generation: 7, volume_root: Some(root),
        volume_ground_revision: Some([1; 7]), volume_fields: vec![(field, snapshot)],
        volume_sources: BTreeMap::from([("top".into(), Handle::default()), ("bottom".into(), Handle::default())]),
        volume_source_counts: BTreeMap::from([("top".into(), 3), ("bottom".into(), 4)]),
        volume_source_positive_counts: BTreeMap::from([("top".into(), 1), ("bottom".into(), 1)]),
        volume_window_counts: BTreeMap::from([("top".into(), 1), ("bottom".into(), 1)]), ..Default::default() };
    (world, instance, status, Requirements { scene, terrains: vec![target] }, windows)
}

#[test]
fn current_double_sided_volume_requires_real_windows_after_cached_adoption() {
    let (mut world, instance, status, expected, windows) = fixture();
    let target = &expected.terrains[0];
    assert!(cached_ready(&world, &expected.scene, instance, &status, Some([1; 7]), target));
    let report = snapshot(&world, &expected, &target.id, instance, &status, Some([1; 7])).unwrap();
    assert!(report_ready(&report));
    world.entity_mut(windows[1]).insert(GpuBladeSourceRevision(6));
    assert!(snapshot(&world, &expected, &target.id, instance, &status, Some([1; 7])).is_err(),
        "旧三维窗不能借状态最新采用代通过截图门");
    world.entity_mut(windows[1]).insert(GpuBladeSourceRevision(7));
    world.entity_mut(windows[1]).remove::<GpuBladeVolumeSource>();
    assert!(snapshot(&world, &expected, &target.id, instance, &status, Some([1; 7])).is_err(),
        "缓存窗口非零不能代替实际三维窗口");
}

#[test]
fn nonzero_hidden_templates_reject_empty_missing_or_stale_volume_sources() {
    let (world, instance, mut status, expected, _) = fixture();
    let target = &expected.terrains[0];
    let current = |status: &DynamicVegetationStatus, ground| cached_ready(&world, &expected.scene, instance, status, ground, target);
    status.volume_source_counts.insert("bottom".into(), 0);
    assert!(!current(&status, Some([1; 7])));
    status.volume_source_counts.insert("bottom".into(), 4);
    status.volume_sources.remove("bottom");
    assert!(!current(&status, Some([1; 7])));
    status.volume_sources.insert("bottom".into(), Handle::default());
    assert!(!current(&status, Some([2; 7])), "旧地表身份不能借模板非零窗口通过");
    status.volume_ready = false;
    assert!(!current(&status, Some([1; 7])));
    assert_eq!(status.blade_window_count, 8, "所有负例仍保留旧模板计数");
}

#[test]
fn current_bound_windows_with_only_zero_density_support_fail_both_gates() {
    let (world, instance, mut status, expected, _) = fixture();
    let target = &expected.terrains[0];
    status.volume_source_positive_counts.insert("bottom".into(), 0);
    assert_eq!(status.volume_source_counts.get("bottom"), Some(&4));
    assert!(!cached_ready(&world, &expected.scene, instance, &status, Some([1; 7]), target));
    let error = snapshot(&world, &expected, &target.id, instance, &status, Some([1; 7])).unwrap_err();
    assert!(error.contains("\"actual_windows\":1"));
    assert!(error.contains("\"positive_source_samples\":0"));
}

#[test]
fn adopted_field_pointer_and_owning_instance_must_still_match() {
    let (mut world, instance, status, expected, _) = fixture();
    let target = &expected.terrains[0];
    let field = status.volume_fields[0].0;
    let retained = world.get::<VolumeSurfaceFields>(field).unwrap().clone();
    world.entity_mut(field).insert(VolumeSurfaceFields::default());
    assert!(!cached_ready(&world, &expected.scene, instance, &status, Some([1; 7]), target));
    world.entity_mut(field).insert(retained);
    assert!(cached_ready(&world, &expected.scene, instance, &status, Some([1; 7]), target));
    let sibling = world.spawn(Visibility::Inherited).id();
    world.entity_mut(field).insert(ChildOf(sibling));
    assert!(!cached_ready(&world, &expected.scene, instance, &status, Some([1; 7]), target),
        "同源兄弟字段不能替当前地形取得信用");
}

#[test]
fn report_rejects_a_missing_saved_side_even_with_nonzero_other_side() {
    let (world, instance, status, expected, _) = fixture();
    let mut report = snapshot(&world, &expected, &expected.terrains[0].id, instance, &status, Some([1; 7])).unwrap();
    report["layers"].as_array_mut().unwrap().pop();
    assert!(!report_ready(&report));
}
