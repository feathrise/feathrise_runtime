//! 用公共 API 验证最大空图、边界格点、局部保存和并发冲突。

use std::path::PathBuf; // 每项测试拥有独立临时 assets 根。
use std::sync::atomic::{AtomicU64, Ordering}; // 并行测试不复用目录名。

use super::*; // 保持新编辑器将调用的同一公开入口。

static TEST_ROOT_COUNTER: AtomicU64 = AtomicU64::new(0); // 仅为测试目录唯一性。

#[path = "float_tests.rs"]
mod float_tests;

#[test]
fn immutable_object_height_versions_preserve_original_and_reopen_the_same_namespace() {
    let root = test_asset_root(); let owner = test_scene_id(); let id = crate::TerrainId::parse("terrain-a").unwrap();
    let mut original = SparseHeightDocument::create_for_terrain(&root, owner.clone(), Some(id.clone()), test_spec(64.,0.5)).unwrap();
    original.save_dirty().unwrap(); let canonical = original.manifest().asset_path();
    let baseline = std::fs::read(root.join(&canonical)).unwrap();
    let path = format!("scenes/{owner}/terrains/{}/versions/edit-first.scene_height_manifest.json", id.as_str());
    original.set_grid_height(64,64,3.).unwrap(); original.fork_to(&path).unwrap(); original.save_dirty().unwrap();
    assert_eq!(std::fs::read(root.join(&canonical)).unwrap(), baseline);
    let mut version = SparseHeightDocument::load_source(&root, &owner, Some(&id), &path).unwrap();
    version.load_tile(SparseHeightTileKey {x:0,z:0}).unwrap();
    assert!((version.sample_grid(64,64).unwrap().unwrap()-3.).abs()<0.01);
    assert!(version.fork_to(&path).is_err());
    assert!(SparseHeightDocument::load_source(&root, &owner, None, &path).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn tiny_clipmap_preserves_boundary_samples_through_save_and_reopen() {
    // 小于一个请求格距时仍以真实边界的两个格点保存，不扩张到旧64米下限。
    for extent in [[0.01_f32, 0.01], [0.01, 1.0], [0.37, 0.83]] {
        let root = test_asset_root();
        let owner = test_scene_id();
        let min = extent.map(|value| -value * 0.5);
        let max = extent.map(|value| value * 0.5);
        let spec = SparseHeightSpec::new(min, max, 0.125, -8.0, 16.0).unwrap();
        let restored = SparseHeightSpec::from_resolution(min, max, spec.cells.map(|value| value as u32 + 1), -8.0, 16.0).unwrap();
        assert_eq!(restored.cells, spec.cells);
        assert_eq!(restored.actual_spacing_meters, spec.actual_spacing_meters);
        let end = spec.cells;
        let mut document = SparseHeightDocument::create_empty(&root, owner.clone(), spec.clone()).unwrap();
        document.set_grid_height(0, 0, -12.5).unwrap();
        document.set_grid_height(end[0], end[1], 34.25).unwrap();
        document.save_dirty().unwrap();
        let mut reopened = SparseHeightDocument::load(&root, &owner).unwrap();
        reopened.load_tile(SparseHeightTileKey { x: 0, z: 0 }).unwrap();
        let reopened_spec = &reopened.manifest().spec;
        assert_eq!(reopened_spec.world_min, spec.world_min);
        assert_eq!(reopened_spec.world_max, spec.world_max);
        assert_eq!(reopened_spec.cells, spec.cells);
        assert_eq!(reopened_spec.requested_spacing_meters, spec.requested_spacing_meters);
        assert_eq!(reopened_spec.base_height, spec.base_height);
        assert_eq!(reopened_spec.height_scale, spec.height_scale);
        // JSON 解析可能改变派生 f64 的最后一位；作者边界、格数与 f32 输入仍要求逐位一致。
        for (actual, expected) in reopened_spec.actual_spacing_meters.iter().zip(spec.actual_spacing_meters) {
            assert!((actual - expected).abs() <= f64::EPSILON * expected.abs().max(1.0));
        }
        assert_eq!(reopened.sample_world_height(f64::from(min[0]), f64::from(min[1])).unwrap(), Some(-12.5));
        assert_eq!(reopened.sample_world_height(f64::from(max[0]), f64::from(max[1])).unwrap(), Some(34.25));
        std::fs::remove_dir_all(root).unwrap(); // 只清理本用例新建的独占资产根。
    }
    assert!(SparseHeightSpec::new([0.0; 2], [0.009; 2], 0.125, -8.0, 16.0).is_err());
}

#[test]
fn terrain_namespaces_keep_equal_tile_coordinates_and_history_independent() {
    let root = test_asset_root(); let owner = test_scene_id();
    let a = crate::TerrainId::parse("terrain-a").unwrap(); let b = crate::TerrainId::parse("terrain-b").unwrap();
    let spec = test_spec(64.0, 0.5);
    for (id, height) in [(&a, 3.0), (&b, -4.0)] {
        let mut document = SparseHeightDocument::create_for_terrain(&root, owner.clone(), Some(id.clone()), spec.clone()).unwrap();
        document.set_grid_height(16, 16, height).unwrap(); document.save_dirty().unwrap();
    }
    for (id, height) in [(&a, 3.0), (&b, -4.0)] {
        let mut document = SparseHeightDocument::load_for_terrain(&root, &owner, Some(id)).unwrap();
        document.load_tile(SparseHeightTileKey { x: 0, z: 0 }).unwrap();
        assert!((document.sample_grid(16, 16).unwrap().unwrap() - height).abs() < 0.01);
        assert_eq!(document.manifest().terrain_id.as_ref(), Some(id));
        assert_eq!(crate::sparse_height_manifest_terrain_id(&owner, &document.manifest().asset_path()).unwrap().as_ref(), Some(id));
    }
    assert!(!root.join(sparse_height_manifest_asset_path(&owner)).exists());
    std::fs::remove_dir_all(root).unwrap();
}

fn test_asset_root() -> PathBuf {
    let serial = TEST_ROOT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "bevy-sparse-height-{}-{serial}",
        std::process::id()
    )); // 每项测试只删除自己创建的确切目录。
    std::fs::create_dir(&root).expect("创建独占测试资产根");
    root
}

fn test_scene_id() -> SceneId {
    SceneId::parse("MAP-sparse-test").expect("测试场景 ID 必须安全")
}

fn test_spec(extent: f32, spacing: f32) -> SparseHeightSpec {
    SparseHeightSpec::new(
        [-extent * 0.5, -extent * 0.5], // 沿用现有新图居中坐标约定。
        [extent * 0.5, extent * 0.5],   // 最外侧格点精确命中正半轴。
        spacing,                        // 当前测试请求的作者精度。
        -80.0,                          // 与现有新场景相同的可向下雕刻高度下界。
        160.0,                          // 零平面位于量程中点。
    )
    .expect("测试地形规格必须合法")
}

#[test]
fn rectangular_resolution_restores_the_same_grid_for_non_divisible_author_spacing() {
    for extent in [[97.1, 95.4], [64.01, 8191.9], [571.0, 592.0], [8192.0, 64.0]] {
        for spacing in [0.125, 0.126, 0.3, 0.333, 1.0, 64.0] {
            let min = extent.map(|value| -value * 0.5);
            let max = extent.map(|value| value * 0.5);
            let source = SparseHeightSpec::new(min, max, spacing, -46.345, 92.69).unwrap();
            let restored = SparseHeightSpec::from_resolution(
                min, max, source.cells.map(|count| count as u32 + 1), -46.345, 92.69,
            ).unwrap_or_else(|error| panic!("{extent:?} / {spacing}: {error}"));
            assert_eq!(restored.cells, source.cells);
            assert_eq!(restored.actual_spacing_meters, source.actual_spacing_meters);
            let edge = restored.grid_point_world(source.cells[0], source.cells[1]).unwrap();
            assert!(edge.into_iter().zip(max).all(|(actual, expected)| (actual - f64::from(expected)).abs() < 1e-9));
            restored.validate().unwrap();
        }
    }
    // 不能为接受矩形而放宽整数拓扑；两轴不存在共同合法请求时仍拒绝。
    for resolution in [[0, 513], [1, 513], [513, 257], [65_538, 65_538]] {
        assert!(SparseHeightSpec::from_resolution([-32.0; 2], [32.0; 2], resolution, -8.0, 16.0).is_err());
    }
}

#[test]
fn maximum_scene_keeps_blank_height_implicit_and_at_world_zero() {
    let root = test_asset_root();
    let scene_id = test_scene_id();
    let spec = test_spec(8192.0, 0.125);
    assert_eq!(spec.cells, [65_536, 65_536]); // 任意合法图尺寸仍允许最细作者格距。
    assert_eq!(spec.tile_counts(), [256, 256]); // 只计算块坐标，不分配 65537² 格点。
    assert_eq!(spec.max_mip_level(), 8); // 八级父页把 256 个作者块收敛到一个全图页。
    assert_eq!(spec.flat_height_code(), 0.0_f32.to_bits()); // 新米制页的隐式零值不再取决于旧编码域。
    let mut document = SparseHeightDocument::create_empty(&root, scene_id.clone(), spec)
        .expect("最大空图无需创建满图高度");
    assert_eq!(document.loaded_tile_count(), 0); // 没有任何已加载或已分配块。
    assert_eq!(document.sample_grid(65_536, 65_536).unwrap(), Some(0.0)); // 场景终点仍是精确世界 Y=0。
    assert_eq!(document.sample_world_height(0.0, 0.0).unwrap(), Some(0.0)); // 世界查询同样不触发块读取。
    document.save_dirty().expect("空白场景只需保存清单");
    assert_eq!(document.manifest().tiles.len(), 0); // 磁盘清单不罗列 65536 个空块。
    assert_eq!(document.manifest().mip_tiles.len(), 0); // 空白父页同样完全隐式。
    let path = root.join(sparse_height_manifest_asset_path(&scene_id));
    assert!(std::fs::metadata(path).unwrap().len() < 2048); // 最大空图持久量仍为常量级。
    std::fs::remove_dir_all(root).expect("只清理当前测试独占根");
}

#[test]
fn one_dirty_base_tile_builds_and_clears_only_its_mip_ancestor_chain() {
    let root = test_asset_root();
    let scene_id = test_scene_id();
    let spec = test_spec(8192.0, 0.125); // 最大图用于验证完整八级父页链仍是常量工作量。
    let mut document =
        SparseHeightDocument::create_empty(&root, scene_id.clone(), spec.clone()).unwrap();
    document.set_grid_height(0, 0, 12.0).unwrap(); // 原点被每一级抽样，便于逐页核对同一作者高度。
    document.save_dirty().unwrap();
    assert_eq!(document.manifest().tiles.len(), 1);
    assert_eq!(
        document.manifest().mip_tiles.len(),
        usize::from(spec.max_mip_level())
    ); // 一个脏基础页每层只产生一个祖先页，而不是扫描整个场景。
    for reference in &document.manifest().mip_tiles {
        assert_eq!(reference.key, SparseHeightTileKey { x: 0, z: 0 });
        let path = root.join(sparse_height_mip_tile_asset_path(
            &scene_id,
            reference.level,
            reference.key,
            &reference.sha256,
        ));
        let bytes = std::fs::read(path).unwrap();
        let tile =
            SparseHeightTile::decode_bytes_at_level(&spec, reference.level, reference.key, &bytes)
                .unwrap();
        assert!((spec.decode_height(tile.get([0, 0]).unwrap()) - 12.0).abs() < 0.002);
    }

    let mut reopened = SparseHeightDocument::load(&root, &scene_id).unwrap();
    reopened
        .load_tile(SparseHeightTileKey { x: 0, z: 0 })
        .unwrap();
    reopened.set_grid_height(0, 0, 0.0).unwrap();
    reopened.save_dirty().unwrap();
    assert!(reopened.manifest().tiles.is_empty());
    assert!(reopened.manifest().mip_tiles.is_empty()); // 回平沿同一祖先链移除所有正式父页引用。
    std::fs::remove_dir_all(root).expect("只清理当前测试独占根");
}

#[test]
fn mip_boundaries_keep_the_same_single_owner_rule() {
    let root = test_asset_root();
    let scene_id = test_scene_id();
    let spec = test_spec(1024.0, 1.0); // 四乘四基础页、两级父页。
    let mut document =
        SparseHeightDocument::create_empty(&root, scene_id.clone(), spec.clone()).unwrap();
    document.set_grid_height(512, 512, 6.0).unwrap(); // 基础页交点属于 (2,2)。
    document.save_dirty().unwrap();
    assert!(
        document
            .manifest()
            .mip_tile_ref(1, SparseHeightTileKey { x: 1, z: 1 })
            .is_some()
    );
    assert!(
        document
            .manifest()
            .mip_tile_ref(1, SparseHeightTileKey { x: 0, z: 0 })
            .is_none()
    );
    assert!(
        document
            .manifest()
            .mip_tile_ref(2, SparseHeightTileKey { x: 0, z: 0 })
            .is_some()
    );
    assert_eq!(
        spec.mip_tile_for_grid_point(1, 256, 256).unwrap().0,
        SparseHeightTileKey { x: 1, z: 1 }
    ); // 父页内部边界仍归右下页，不重复保存共享格点。
    std::fs::remove_dir_all(root).expect("只清理当前测试独占根");
}

#[test]
fn shared_grid_boundary_has_one_owner_and_round_trips_locally() {
    let root = test_asset_root();
    let scene_id = test_scene_id();
    let spec = test_spec(64.0, 0.125); // 两轴各 512 格，恰好穿过两个 256 格块。
    assert_eq!(
        spec.tile_for_grid_point(255, 255).unwrap().0,
        SparseHeightTileKey { x: 0, z: 0 }
    );
    assert_eq!(
        spec.tile_for_grid_point(256, 255).unwrap().0,
        SparseHeightTileKey { x: 1, z: 0 }
    );
    assert_eq!(
        spec.tile_for_grid_point(256, 256).unwrap().0,
        SparseHeightTileKey { x: 1, z: 1 }
    );
    assert_eq!(
        spec.tile_for_grid_point(512, 512).unwrap().0,
        SparseHeightTileKey { x: 1, z: 1 }
    );
    assert_eq!(
        spec.tile_shape(SparseHeightTileKey { x: 0, z: 0 }),
        Some([256, 256])
    );
    assert_eq!(
        spec.tile_shape(SparseHeightTileKey { x: 1, z: 1 }),
        Some([257, 257])
    );
    let mut document = SparseHeightDocument::create_empty(&root, scene_id.clone(), spec).unwrap();
    document.set_grid_height(255, 255, 5.0).unwrap(); // 第一块局部格点。
    document.set_grid_height(256, 255, 7.0).unwrap(); // 共享边界只写右块。
    document.set_grid_height(256, 256, 9.0).unwrap(); // 四块交点只写右下块。
    assert_eq!(document.dirty_tile_count(), 3); // 保存工作量与实际笔划触及块数一致。
    document.save_dirty().unwrap();
    assert_eq!(document.manifest().tiles.len(), 3); // 其余平地块不落盘。
    let mut reopened = SparseHeightDocument::load(&root, &scene_id).unwrap();
    assert_eq!(reopened.loaded_tile_count(), 0); // 重开只解析清单，不立即读取高度块。
    assert_eq!(reopened.sample_grid(256, 256).unwrap(), None); // 未载入的真实块不可伪装为平地。
    assert_eq!(reopened.sample_grid(512, 512).unwrap(), None); // 同块未编辑点仍须等待真实块加载。
    for reference in reopened.manifest().tiles.clone() {
        reopened.load_tile(reference.key).unwrap(); // 外部 IO 调度器可按需逐块加载。
    }
    assert!((reopened.sample_grid(256, 256).unwrap().unwrap() - 9.0).abs() < 0.002);
    assert_eq!(reopened.sample_grid(512, 512).unwrap(), Some(0.0)); // 已载块的初始点仍精确世界零高。
    reopened.set_grid_height(256, 255, 0.0).unwrap(); // 单个块回平地后可从索引消失。
    reopened.save_dirty().unwrap();
    assert_eq!(reopened.manifest().tiles.len(), 2); // 只保留仍非平地的块索引。
    std::fs::remove_dir_all(root).expect("只清理当前测试独占根");
}

#[test]
fn world_sampling_uses_the_same_triangle_diagonal_as_collision() {
    let root = test_asset_root(); // 每项使用独立临时资产根。
    let scene_id = test_scene_id(); // 内容只存在当前临时目录，不接触正式场景。
    let spec =
        SparseHeightSpec::new([0.0, 0.0], [64.0, 64.0], 64.0, 0.0, 10.0).expect("单单元规格应合法");
    let mut document = SparseHeightDocument::create_empty(&root, scene_id, spec).unwrap();
    document.set_grid_height(0, 0, 0.0).unwrap(); // NW 保持最低点。
    document.set_grid_height(1, 0, 10.0).unwrap(); // NE 是高点。
    document.set_grid_height(0, 1, 10.0).unwrap(); // SW 是高点。
    document.set_grid_height(1, 1, 0.0).unwrap(); // SE 回到低点，构成双线性鞍形反例。
    let center = document
        .sample_world_height(32.0, 32.0)
        .unwrap()
        .expect("当前作者块已经驻留");
    assert!((center - 10.0).abs() < 0.002); // 中心位于共享 NE-SW 对角线，应为十米而不是双线性的五米。
    std::fs::remove_dir_all(root).expect("只清理当前测试创建的临时场景");
}

#[test]
fn concurrent_author_cannot_overwrite_newer_manifest() {
    let root = test_asset_root();
    let scene_id = test_scene_id();
    let mut initial =
        SparseHeightDocument::create_empty(&root, scene_id.clone(), test_spec(64.0, 2.0)).unwrap();
    initial.save_dirty().unwrap();
    let mut first = SparseHeightDocument::load(&root, &scene_id).unwrap();
    let mut stale = SparseHeightDocument::load(&root, &scene_id).unwrap();
    first.set_grid_height(1, 1, 3.0).unwrap();
    stale.set_grid_height(2, 2, 8.0).unwrap();
    first.save_dirty().unwrap();
    let error = stale.save_dirty().unwrap_err(); // 旧清单版本不得覆盖新作者的高度。
    assert!(error.contains("版本冲突"));
    let mut latest = SparseHeightDocument::load(&root, &scene_id).unwrap();
    let key = latest.manifest().tiles[0].key;
    latest.load_tile(key).unwrap();
    assert!((latest.sample_grid(1, 1).unwrap().unwrap() - 3.0).abs() < 0.002);
    assert_eq!(latest.sample_grid(2, 2).unwrap(), Some(0.0)); // 失败事务不改变正式清单。
    std::fs::remove_dir_all(root).expect("只清理当前测试独占根");
}

#[test]
fn corrupted_content_addressed_tile_is_rejected_before_adoption() {
    let root = test_asset_root();
    let scene_id = test_scene_id();
    let mut document =
        SparseHeightDocument::create_empty(&root, scene_id.clone(), test_spec(64.0, 2.0)).unwrap();
    document.set_grid_height(1, 1, 4.0).unwrap();
    document.save_dirty().unwrap();
    let reference = &document.manifest().tiles[0];
    let path = root.join(sparse_height_tile_asset_path(
        &scene_id,
        reference.key,
        &reference.sha256,
    ));
    std::fs::write(&path, b"corrupted").unwrap(); // 只损坏当前测试独占文件。
    let mut reopened = SparseHeightDocument::load(&root, &scene_id).unwrap();
    assert!(reopened.load_tile(reference.key).is_err()); // 哈希或长度不符不进入可查询集合。
    assert_eq!(reopened.sample_grid(1, 1).unwrap(), None); // 错块失败后不能当作零高度继续使用。
    std::fs::remove_dir_all(root).expect("只清理当前测试独占根");
}

#[test]
fn live_override_publishes_explicit_flat_tile_and_clears_between_scenes() {
    let root = test_asset_root();
    let scene_id = test_scene_id();
    let mut document =
        SparseHeightDocument::create_empty(&root, scene_id.clone(), test_spec(64.0, 2.0)).unwrap();
    let key = SparseHeightTileKey { x: 0, z: 0 };
    document.set_grid_height(1, 1, 6.0).unwrap();

    let mut live = SparseHeightLiveOverride::default();
    live.retarget(Some(scene_id.clone()), 4);
    assert_eq!(
        live.publish_tiles(&scene_id, 5, [document.tile_snapshot(key).unwrap()])
            .unwrap(),
        1
    );
    assert_eq!(live.revision(), 5);
    assert_ne!(
        live.tile(key).unwrap().get([1, 1]).unwrap(),
        document.manifest().spec.flat_height_code()
    );

    document.set_grid_height(1, 1, 0.0).unwrap();
    live.publish_tiles(&scene_id, 6, [document.tile_snapshot(key).unwrap()])
        .unwrap();
    assert!(
        live.tile(key)
            .unwrap()
            .height_codes(&document.manifest().spec)
            .all(|height| height == document.manifest().spec.flat_height_code())
    ); // 显式平地仍保留一块，以遮蔽场景可能尚持有的旧非平地磁盘页。
    assert!(
        live.publish_tiles(&scene_id, 5, [document.tile_snapshot(key).unwrap()])
            .is_err()
    ); // 迟到作者结果不能倒退共享修订。

    let next_scene = SceneId::parse("MAP-sparse-next").unwrap();
    live.retarget(Some(next_scene.clone()), 7);
    assert_eq!(live.scene_id(), Some(&next_scene));
    assert_eq!(live.tile_count(), 0); // 切换场景不沿用旧块，尤其不能沿用显式平地覆盖。
    std::fs::remove_dir_all(root).expect("只清理当前测试独占根");
}
