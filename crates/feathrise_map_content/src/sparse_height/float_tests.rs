//! Clipmap无创建高度上限：真实旧页兼容、局部浮点保存、整笔历史及溢出零写入回归。

use super::*;
use std::sync::Arc;

fn small_spec() -> SparseHeightSpec {
    SparseHeightSpec::new([-32.0; 2], [32.0; 2], 0.125, -8.0, 16.0).unwrap()
}

fn brush(kind: SparseTerrainBrushKind, speed: f32) -> SparseTerrainBrushInput {
    SparseTerrainBrushInput { kind, center_xz: [0.0; 2], radius_meters: 0.3, strength: 1.0,
        speed_meters_per_second: speed, delta_seconds: 0.05, flatten_target_meters: None }
}

#[test]
fn clipmap_float_height_codes_preserve_finite_meters_and_exact_zero_beyond_legacy_domain() {
    let spec = small_spec();
    for height in [-f32::MAX, -100_000.25, -9.0, -0.001, 0.0, 9.0, 100_000.25, f32::MAX] {
        assert_eq!(spec.decode_height(spec.encode_height(height).unwrap()).to_bits(), height.to_bits());
    }
    assert_eq!(spec.encode_height(-0.0).unwrap(), 0);
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(spec.encode_height(invalid).is_err());
    }
    assert_eq!(spec.decode_legacy_height(32768), 0.0, "旧v2中点仍精确为零");
    assert_eq!(spec.decode_legacy_height(0), -8.0);
    assert_eq!(spec.decode_legacy_height(u16::MAX), 8.0);
}

#[test]
fn clipmap_float_tile_roundtrip_and_bad_bits_obey_the_new_bounded_file_format() {
    let spec = small_spec();
    let key = SparseHeightTileKey { x: 1, z: 1 };
    let mut tile = SparseHeightTile::blank(&spec, key).unwrap();
    for (local, height) in [([0, 0], -1234.125), ([256, 256], 9876.5)] {
        tile.set(local, spec.encode_height(height).unwrap()).unwrap();
    }
    let bytes = tile.encode_bytes();
    assert_eq!(bytes.len(), 24 + 257 * 257 * 4);
    assert_eq!(bytes.len(), super::super::tile::maximum_tile_file_bytes());
    assert!(tile.memory_bytes() >= 257 * 257 * 4);
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 3);
    assert_eq!(SparseHeightTile::decode_bytes(&spec, key, &bytes).unwrap(), tile);
    for invalid in [f32::NAN.to_bits(), f32::INFINITY.to_bits(), (-0.0_f32).to_bits()] {
        let mut broken = bytes.clone();
        broken[24..28].copy_from_slice(&invalid.to_le_bytes());
        assert!(SparseHeightTile::decode_asset_bytes(key, &broken).is_err());
        assert!(tile.set([1, 1], invalid).is_err());
    }
}

#[test]
fn clipmap_float_raise_lower_undo_redo_and_save_keep_exact_height_outside_creation_domain() {
    let root = test_asset_root();
    let map = test_map_id();
    let spec = small_spec();
    SparseHeightDocument::create_empty(&root, map.clone(), spec.clone()).unwrap().save_dirty().unwrap();
    let mut author = SparseTerrainAuthorSession::load(&root, &map).unwrap();
    let raised = author.apply_brush(brush(SparseTerrainBrushKind::Raise, 2000.0)).unwrap().changes;
    assert_eq!(author.sample_grid_encoded(256, 256).unwrap(), Some(100.0_f32.to_bits()));
    assert_eq!(author.intersect_loaded_height_ray([0.0, 200.0, 0.0], [0.0, -1.0, 0.0]), Some([0.0, 100.0, 0.0]));
    let lowered = author.apply_brush(brush(SparseTerrainBrushKind::Lower, 6000.0)).unwrap().changes;
    assert_eq!(author.sample_grid_encoded(256, 256).unwrap(), Some((-200.0_f32).to_bits()));
    author.replay_changes(&lowered, false).unwrap();
    author.replay_changes(&raised, false).unwrap();
    assert_eq!(author.sample_grid_encoded(256, 256).unwrap(), Some(0));
    author.replay_changes(&raised, true).unwrap();
    author.replay_changes(&lowered, true).unwrap();
    assert_eq!(author.sample_grid_encoded(256, 256).unwrap(), Some((-200.0_f32).to_bits()));
    assert!(author.height_bounds()[0] <= -200.0 && author.height_bounds()[1] >= 100.0);
    author.save_dirty().unwrap();
    let mut reopened = SparseTerrainAuthorSession::load(&root, &map).unwrap();
    reopened.prefetch_world_sample_tiles(0.0, 0.0).unwrap();
    assert_eq!(reopened.sample_grid_encoded(256, 256).unwrap(), Some((-200.0_f32).to_bits()));
    assert_eq!(reopened.spec().base_height, -8.0, "不能靠改大旧量程实现越界雕刻");
    assert_eq!(reopened.spec().height_scale, 16.0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn clipmap_float_extreme_brush_overflow_and_invalid_history_leave_all_author_values_unchanged() {
    let root = test_asset_root();
    let map = test_map_id();
    let spec = small_spec();
    let mut document = SparseHeightDocument::create_empty(&root, map.clone(), spec.clone()).unwrap();
    document.set_grid_height(256, 256, f32::MAX).unwrap();
    document.save_dirty().unwrap();
    let mut author = SparseTerrainAuthorSession::load(&root, &map).unwrap();
    author.prefetch_world_sample_tiles(0.0, 0.0).unwrap();
    assert!(author.apply_brush(brush(SparseTerrainBrushKind::Raise, f32::MAX)).is_err());
    assert!(!author.is_dirty());
    assert_eq!(author.sample_grid_encoded(256, 256).unwrap(), Some(f32::MAX.to_bits()));
    let changes = [SparseTerrainHeightChange { x: 256, z: 256, before: f32::MAX.to_bits(), after: 1.0_f32.to_bits() },
        SparseTerrainHeightChange { x: 257, z: 256, before: 0, after: f32::NAN.to_bits() }];
    assert!(author.replay_changes(&changes, true).is_err());
    assert_eq!(author.sample_grid_encoded(256, 256).unwrap(), Some(f32::MAX.to_bits()));
    assert_eq!(author.sample_grid_encoded(257, 256).unwrap(), Some(0));
    let mut flatten = brush(SparseTerrainBrushKind::Flatten, f32::MAX);
    flatten.flatten_target_meters = Some(-f32::MAX);
    author.apply_brush(flatten).unwrap(); // 两个有限极值的目标差值不能在中间f32减法溢出。
    let expected = (f64::from(f32::MAX) - f64::from(f32::MAX) * f64::from(0.05_f32)) as f32;
    assert_eq!(author.sample_grid_encoded(256, 256).unwrap(), Some(expected.to_bits()));
    let smooth = author.apply_brush(brush(SparseTerrainBrushKind::Smooth, f32::MAX)).unwrap();
    assert!(!smooth.changes.is_empty());
    assert!(author.sample_world_height(0.0, 0.0).unwrap().unwrap().is_finite());
    std::fs::remove_dir_all(root).unwrap();
}

fn legacy_bytes(spec: &SparseHeightSpec, key: SparseHeightTileKey, first: u16) -> Vec<u8> {
    let shape = spec.tile_shape(key).unwrap();
    let mut bytes = b"B5HTILE\0".to_vec();
    bytes.extend_from_slice(&2_u32.to_le_bytes());
    bytes.extend_from_slice(&key.x.to_le_bytes());
    bytes.extend_from_slice(&key.z.to_le_bytes());
    bytes.extend_from_slice(&shape[0].to_le_bytes());
    bytes.extend_from_slice(&shape[1].to_le_bytes());
    for index in 0..usize::from(shape[0]) * usize::from(shape[1]) {
        bytes.extend_from_slice(&(if index == 0 { first } else { 32768_u16 }).to_le_bytes());
    }
    bytes
}

#[test]
fn clipmap_float_partial_save_reuses_untouched_v2_files_and_builds_mips_from_mixed_sources() {
    let root = test_asset_root();
    let map = test_map_id();
    let spec = small_spec();
    let a = SparseHeightTileKey { x: 0, z: 0 };
    let b = SparseHeightTileKey { x: 1, z: 1 };
    let mut manifest = SparseHeightManifest::empty(map.clone(), spec.clone()).unwrap();
    manifest.schema_version = 2;
    manifest.height_bounds = None;
    let mut original = Vec::new();
    for (key, code) in [(a, 40000_u16), (b, 12345_u16)] {
        let bytes = legacy_bytes(&spec, key, code);
        let hash = crate::authoring_io::bytes_hash(&bytes);
        let path = root.join(manifest.tile_asset_path(key, &hash));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let raw = SparseHeightTile::decode_asset_bytes(key, &bytes).unwrap();
        assert!(raw.get([0, 0]).is_none(), "未知旧量程不能伪造米制bits");
        assert_eq!(raw.get_height(&spec, [0, 0]), Some(spec.decode_legacy_height(code).to_bits()));
        assert_eq!(raw.encode_bytes(), bytes, "只读资产保持真实v2原字节和摘要");
        manifest.tiles.push(SparseHeightTileRef { key, sha256: hash });
        original.push((path, bytes));
    }
    let manifest_path = root.join(manifest.asset_path());
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    let original_manifest = std::fs::read(&manifest_path).unwrap();
    let mut document = SparseHeightDocument::load(&root, &map).unwrap();
    document.load_tile(a).unwrap();
    assert_eq!(document.sample_grid_encoded(0, 0).unwrap(), Some(spec.decode_legacy_height(40000).to_bits()));
    document.save_dirty().unwrap();
    assert_eq!(std::fs::read(&manifest_path).unwrap(), original_manifest, "只读打开旧图不能偷偷升级或改文件");
    document.set_grid_height(0, 0, 1000.125).unwrap();
    document.save_dirty().unwrap();
    assert_eq!(document.loaded_tile_count(), 1, "父页按需读邻居，不把未触及作者页迁入编辑会话");
    assert_eq!(document.manifest().schema_version, 3);
    assert_eq!(document.manifest().spec, spec);
    assert_eq!(document.manifest().tile_ref(b), manifest.tile_ref(b));
    for (path, bytes) in &original { assert_eq!(&std::fs::read(path).unwrap(), bytes); }
    let parent = &document.manifest().mip_tiles[0];
    let bytes = std::fs::read(root.join(document.manifest().mip_tile_asset_path(parent.level, parent.key, &parent.sha256))).unwrap();
    let tile = SparseHeightTile::decode_bytes_at_level(&spec, parent.level, parent.key, &bytes).unwrap();
    assert_eq!(tile.get_height(&spec, [0, 0]), Some(1000.125_f32.to_bits()));
    assert_eq!(tile.get_height(&spec, [128, 128]), Some(spec.decode_legacy_height(12345).to_bits()));
    let mut reopened = SparseHeightDocument::load(&root, &map).unwrap();
    reopened.load_tile(a).unwrap(); reopened.load_tile(b).unwrap();
    assert_eq!(reopened.sample_grid_encoded(0, 0).unwrap(), Some(1000.125_f32.to_bits()));
    assert_eq!(reopened.sample_grid_encoded(256, 256).unwrap(), Some(spec.decode_legacy_height(12345).to_bits()));
    assert!(reopened.height_bounds()[1] >= 1000.125);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn clipmap_float_session_crater_can_cross_legacy_floor_without_changing_the_author_height() {
    let spec = small_spec();
    let map = test_map_id();
    let source = Arc::new(crate::SurfaceCraterResponse::new(bevy::prelude::Vec2::splat(-32.0),
        bevy::prelude::Vec2::splat(32.0), [1; 2], Arc::from([0_u8; 16]),
        Arc::from([255_u8, 0, 0, 0]), vec![1.0]).unwrap());
    let mut session = SparseHeightSessionCrater::default();
    session.clear(Some(map));
    for _ in 0..6 { session.apply(&spec, bevy::prelude::Vec2::ZERO, 4.0, 2.0, source.clone()).unwrap(); }
    assert_eq!(session.depth_bound(), 12.0);
    assert_eq!(sample_crater_height(&spec, session.tiles(), [256, 256], -7.0), -19.0);
    assert_eq!(spec.decode_height(spec.encode_height(-7.0).unwrap()), -7.0, "技能只叠会话层，不改作者码或旧兼容域");
    let revision = session.revision();
    assert!(session.apply(&spec, bevy::prelude::Vec2::ZERO, 9.0, 2.0, source).is_err());
    assert_eq!(session.depth_bound(), 12.0);
    assert_eq!(session.revision(), revision);
    session.clear(None);
    assert_eq!(session.depth_bound(), 0.0);
}
