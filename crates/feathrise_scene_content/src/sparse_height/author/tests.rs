//! 共享稀疏作者核心回归；本模块不依赖场景编辑器 crate。

use std::path::PathBuf; // 每项测试使用独占临时资产根。
use std::sync::atomic::{AtomicU64, Ordering}; // 并行测试目录不能互相复用。

use super::{SparseTerrainAuthorSession, SparseTerrainBrushInput, SparseTerrainBrushKind}; // 直接验证公共作者接口。
use crate::{SceneId, SparseHeightDocument, SparseHeightSpec}; // 只用场景内容层创建正式空白清单。

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0); // 当前测试进程内的唯一目录流水号。

#[test]
fn scaled_object_brush_keeps_world_radius_and_does_not_write_outside_the_ellipse() {
    let (root, scene_id) = isolated_scene();
    let mut author = SparseTerrainAuthorSession::load(&root, &scene_id).unwrap();
    let input = SparseTerrainBrushInput { kind: SparseTerrainBrushKind::Raise, center_xz:[0.;2], radius_meters:1.1,
        strength:1., speed_meters_per_second:10., delta_seconds:0.05, flatten_target_meters:None };
    author.prefetch_scaled_brush_tiles(input, [2.,1.]).unwrap();
    let changed = author.apply_scaled_brush_loaded(input, [2.,1.]).unwrap();
    assert!(!changed.changes.is_empty());
    for point in changed.changes { let world = author.spec().grid_point_world(point.x,point.z).unwrap();
        assert!((world[0]*2.).hypot(world[1]) < 1.1); }
    assert!(author.prefetch_scaled_brush_tiles(input, [0.,1.]).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn complete_history_larger_than_one_dab_replays_atomically() {
    use crate::SparseTerrainHeightChange;
    let (root, scene_id) = isolated_scene();
    let mut author = SparseTerrainAuthorSession::load(&root, &scene_id).unwrap();
    let before = author.spec().flat_height_code();
    let after = author.spec().encode_height(5.).unwrap();
    let [cells_x, cells_z] = author.spec().cells;
    let changes = (0..=cells_z)
        .flat_map(|z| {
            (0..=cells_x).map(move |x| SparseTerrainHeightChange {
                x,
                z,
                before,
                after,
            })
        })
        .collect::<Vec<_>>(); // 一整笔跨多个刷印，合法历史可大于单次刷印的候选矩形。
    assert!(changes.len() as u64 > super::MAX_SPARSE_BRUSH_CANDIDATES);
    author.prefetch_change_tiles(&changes).unwrap();
    let candidate = author.prepare_history_changes_loaded(std::sync::Arc::new(changes.clone()), true).unwrap();
    assert!(!author.is_dirty());
    assert_eq!(author.sample_grid_encoded(cells_x, cells_z).unwrap(), Some(before));
    let applied = author.install_prepared_edit(candidate).unwrap();
    assert_eq!(author.sample_grid_encoded(cells_x, cells_z).unwrap(), Some(after));
    author.rollback_prepared_edit(applied.rollback).unwrap();
    assert!(!author.is_dirty());
    assert_eq!(author.sample_grid_encoded(cells_x, cells_z).unwrap(), Some(before));
    author.replay_changes_loaded(&changes, true).unwrap();

    let revision = author.revision;
    let mut invalid = changes.clone();
    invalid.last_mut().unwrap().after = after.wrapping_add(1); // 最后一项错误也必须在任何写入前发现。
    assert!(author.replay_changes_loaded(&invalid, false).is_err());
    assert_eq!(author.revision, revision);
    assert!(
        changes.iter().all(|change| {
            author.sample_grid_encoded(change.x, change.z).unwrap() == Some(after)
        })
    );

    author.replay_changes(&changes, false).unwrap();
    assert!(
        changes.iter().all(|change| {
            author.sample_grid_encoded(change.x, change.z).unwrap() == Some(before)
        })
    );
    let tiles = author.tile_snapshots_for_changes(&changes).unwrap();
    assert!(!tiles.is_empty()); // 全平撤销仍发布显式页，不能露出磁盘里的旧凸起。
    author.replay_changes(&changes, true).unwrap();
    assert!(
        changes.iter().all(|change| {
            author.sample_grid_encoded(change.x, change.z).unwrap() == Some(after)
        })
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_local_edit_is_readonly_until_commit_and_rollback_restores_dirty_responsibility() {
    use crate::SparseTerrainHeightChange;
    use std::sync::Arc;
    let (root, scene_id) = isolated_scene();
    let mut author = SparseTerrainAuthorSession::load(&root, &scene_id).unwrap();
    let before = author.spec().flat_height_code();
    let after = author.spec().encode_height(5.).unwrap();
    let changes = Arc::new(vec![
        SparseTerrainHeightChange {
            x: 255,
            z: 255,
            before,
            after,
        },
        SparseTerrainHeightChange {
            x: 256,
            z: 256,
            before,
            after,
        },
    ]);
    let edit = author
        .prepare_changes_loaded(changes.clone(), true)
        .unwrap();
    assert!(!author.is_dirty());
    assert_eq!(author.sample_grid_encoded(256, 256).unwrap(), Some(before));
    let applied = author.install_prepared_edit(edit).unwrap();
    assert_eq!(applied.live_tiles.len(), 2);
    assert!(author.is_dirty());
    author.rollback_prepared_edit(applied.rollback).unwrap();
    assert!(!author.is_dirty());
    assert_eq!(author.sample_grid_encoded(256, 256).unwrap(), Some(before));
    author.save_dirty().unwrap(); // 回滚后不能误保存取消的平台。
    let mut reopened = SparseTerrainAuthorSession::load(&root, &scene_id).unwrap();
    assert_eq!(reopened.sample_world_height(0., 0.).unwrap(), Some(0.));
    let edit = author
        .prepare_changes_loaded(changes.clone(), true)
        .unwrap();
    author.install_prepared_edit(edit).unwrap();
    let undo = author
        .prepare_changes_loaded(changes.clone(), false)
        .unwrap();
    author.install_prepared_edit(undo).unwrap();
    assert_eq!(author.sample_grid_encoded(256, 256).unwrap(), Some(before));
    let redo = author.prepare_changes_loaded(changes, true).unwrap();
    author.install_prepared_edit(redo).unwrap();
    assert_eq!(author.sample_grid_encoded(256, 256).unwrap(), Some(after));
    author.save_dirty().unwrap();
    reopened = SparseTerrainAuthorSession::load(&root, &scene_id).unwrap();
    reopened.prefetch_world_sample_tiles(0., 0.).unwrap();
    assert!(reopened.sample_world_height(0., 0.).unwrap().unwrap() > 4.99);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_edit_rejects_foreign_session_stale_revision_and_duplicate_points_without_writes() {
    use crate::SparseTerrainHeightChange;
    use std::sync::Arc;
    let (root, scene_id) = isolated_scene();
    let mut author = SparseTerrainAuthorSession::load(&root, &scene_id).unwrap();
    let before = author.spec().flat_height_code();
    let after = author.spec().encode_height(5.).unwrap();
    let changes = Arc::new(vec![SparseTerrainHeightChange {
        x: 256,
        z: 256,
        before,
        after,
    }]);
    let edit = author
        .prepare_changes_loaded(changes.clone(), true)
        .unwrap();
    let mut other = SparseTerrainAuthorSession::load(&root, &scene_id).unwrap();
    assert!(other.install_prepared_edit(edit).is_err());
    assert!(!other.is_dirty());
    let stale = author
        .prepare_changes_loaded(changes.clone(), true)
        .unwrap();
    let current = author
        .prepare_changes_loaded(changes.clone(), true)
        .unwrap();
    author.install_prepared_edit(current).unwrap();
    assert!(author.install_prepared_edit(stale).is_err());
    assert_eq!(author.sample_grid_encoded(256, 256).unwrap(), Some(after));
    assert!(
        other
            .prepare_changes_loaded(Arc::new(vec![changes[0], changes[0]]), true)
            .is_err()
    );
    assert_eq!(other.sample_grid_encoded(256, 256).unwrap(), Some(before));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_edit_and_prefetch_reject_fixed_budgets_without_allocating_pages() {
    use crate::SparseTerrainHeightChange;
    use std::sync::Arc;
    let (root, scene_id) = isolated_scene();
    let mut author = SparseTerrainAuthorSession::load(&root, &scene_id).unwrap();
    let change = SparseTerrainHeightChange {
        x: 0,
        z: 0,
        before: author.spec().flat_height_code(),
        after: author.spec().encode_height(5.0).unwrap(),
    };
    assert!(
        author
            .prepare_changes_loaded(
                Arc::new(vec![
                    change;
                    super::MAX_SPARSE_BRUSH_CANDIDATES as usize + 1
                ]),
                true
            )
            .is_err()
    );
    assert!(
        author
            .prefetch_prepared_change_tiles(&vec![
                change;
                super::MAX_SPARSE_BRUSH_CANDIDATES as usize + 1
            ])
            .is_err()
    ); // 普通整笔历史放开刷印长度后，联合候选仍须在 IO 前拒绝超限。
    assert_eq!(author.loaded_tile_count(), 0);
    assert!(!author.is_dirty());
    let large_spec = SparseHeightSpec::new([0., 0.], [8192., 64.], 0.125, -20., 80.).unwrap();
    let large_id = SceneId::parse("MAP-platform-budget").unwrap();
    let mut document =
        SparseHeightDocument::create_empty(&root, large_id.clone(), large_spec).unwrap();
    document.save_dirty().unwrap();
    let mut large = SparseTerrainAuthorSession::load(&root, &large_id).unwrap();
    assert!(large.prefetch_grid_rect([0, 0], [37 * 256, 1]).is_err());
    let scattered = (0..37)
        .map(|index| SparseTerrainHeightChange {
            x: index * 256,
            z: 1,
            before: large.spec().flat_height_code(),
            ..change
        })
        .collect::<Vec<_>>();
    assert!(large.prefetch_prepared_change_tiles(&scattered).is_err()); // 验证全部页上限后才能读取第一块。
    assert_eq!(large.loaded_tile_count(), 0);
    // 同一份37页长笔历史合法，但候选及失效回滚仍不得留下脏页。
    let scattered = Arc::new(scattered);
    large.prefetch_change_tiles(&scattered).unwrap();
    let edit = large.prepare_history_changes_loaded(scattered.clone(), true).unwrap();
    assert!(!large.is_dirty());
    let applied = large.install_prepared_edit(edit).unwrap();
    assert_eq!(applied.live_tiles.len(), 37);
    assert_eq!(large.sample_grid_encoded(36 * 256, 1).unwrap(), Some(change.after));
    large.rollback_prepared_edit(applied.rollback).unwrap();
    assert!(!large.is_dirty());
    assert!(scattered.iter().all(|point| large.sample_grid_encoded(point.x, point.z).unwrap() == Some(point.before)));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn external_manifest_change_rejects_old_author_baseline_before_platform_io() {
    let (root, scene_id) = isolated_scene();
    let author = SparseTerrainAuthorSession::load(&root, &scene_id).unwrap();
    author.verify_source_version().unwrap();
    let path = root.join(crate::sparse_height_manifest_asset_path(&scene_id));
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.push(b' ');
    std::fs::write(&path, bytes).unwrap();
    assert!(author.verify_source_version().is_err());
    assert!(!author.is_dirty());
    std::fs::remove_dir_all(root).unwrap();
}

fn isolated_scene() -> (PathBuf, SceneId) {
    let serial = TEST_COUNTER.fetch_add(1, Ordering::Relaxed); // 并发执行时仍生成不同目录。
    let root = std::env::temp_dir().join(format!(
        "bevy-map-content-sparse-author-{}-{serial}",
        std::process::id()
    )); // 测试数据只存在系统临时目录。
    std::fs::create_dir(&root).expect("共享作者测试应创建独占资产根"); // 不触碰项目内任何场景。
    let scene_id = SceneId::parse("MAP-shared-author-test").expect("固定测试场景 ID 合法"); // 使用正式路径校验。
    let spec = SparseHeightSpec::new([-32.0, -32.0], [32.0, 32.0], 0.125, -20.0, 80.0)
        .expect("测试规格应满足正式稀疏格式"); // 原点同时覆盖真实世界坐标换算。
    let mut document = SparseHeightDocument::create_empty(&root, scene_id.clone(), spec)
        .expect("测试应创建空白稀疏作者清单"); // 空白图不分配高度块。
    document.save_dirty().expect("空白清单应原子发布"); // 会话随后必须只从磁盘公共协议载入。
    (root, scene_id)
}

#[test]
fn shared_author_core_edits_saves_and_reopens_without_f12() {
    let (root, scene_id) = isolated_scene(); // 整项验证不构造 App、World 或 F12 资源。
    let mut author =
        SparseTerrainAuthorSession::load(&root, &scene_id).expect("公共作者会话应从正式清单按需载入"); // 初始只驻留小型清单。
    let input = SparseTerrainBrushInput {
        kind: SparseTerrainBrushKind::Raise, // 独立工具直接使用同一米制抬升语义。
        center_xz: [0.0, 0.0],               // 原点位于多个作者块共享边。
        radius_meters: 0.5,                  // 只触及局部格点和有限块。
        strength: 1.0,                       // 使用清晰可断言的完整强度。
        speed_meters_per_second: 20.0,       // 与 F12 当前默认速度一致。
        delta_seconds: 0.05,                 // 单次积分得到约一米中心高度。
        flatten_target_meters: None,         // 抬升工具不需要冻结压平目标。
    };
    let changed = author
        .apply_brush(input)
        .expect("公共核心应独立执行局部刷印"); // 串行入口负责预取后修改。
    assert!(!changed.changes.is_empty()); // 独立调用必须产生可用于外部历史栈的差分。
    assert!(author.is_dirty()); // 未保存状态由共享核心维护，不依赖 F12 资源。
    author
        .save_dirty()
        .expect("共享核心应独立提交内容寻址高度页"); // 保存仍遵守清单最后发布。
    assert!(!author.is_dirty()); // 完整提交后磁盘与作者修订一致。
    let mut reopened =
        SparseTerrainAuthorSession::load(&root, &scene_id).expect("独立工具应能重新打开同一作者格式"); // 新会话没有旧内存快照。
    reopened
        .prefetch_world_sample_tiles(0.0, 0.0)
        .expect("重开后应按世界点只预取必要页"); // 非平地页必须显式读取。
    let height = reopened
        .sample_world_height(0.0, 0.0)
        .expect("世界点查询应通过规格验证")
        .expect("预取后的真实页应可采样"); // 不允许退化为隐式平地。
    assert!((height - 1.0).abs() < 0.002); // 内容寻址保存和重载保留米制高度。
    std::fs::remove_dir_all(root).expect("只清理当前测试创建的临时场景"); // 项目资产始终不受影响。
}

/// 宽坡验收必须使用真实作者页与相同世界地形，孤立尖峰不足以暴露一源格平滑在细网格上的失效。
fn smoothing_author(spacing: f32, height: impl Fn([f64; 2]) -> f32) -> (PathBuf, SparseTerrainAuthorSession) {
    let (root, _) = isolated_scene();
    let map = SceneId::parse("MAP-world-scale-smoothing").unwrap();
    let spec = SparseHeightSpec::new([-32.; 2], [32.; 2], spacing, -20., 80.).unwrap();
    let mut document = SparseHeightDocument::create_empty(&root, map.clone(), spec.clone()).unwrap();
    for z in 0..=spec.cells[1] {
        for x in 0..=spec.cells[0] {
            let value = height(spec.grid_point_world(x, z).unwrap());
            if value != 0. {
                document.set_grid_encoded(x, z, spec.encode_height(value).unwrap()).unwrap();
            }
        }
    }
    document.save_dirty().unwrap();
    (root.clone(), SparseTerrainAuthorSession::load(&root, &map).unwrap())
}

fn smoothing_input(radius: f32) -> SparseTerrainBrushInput {
    SparseTerrainBrushInput { kind: SparseTerrainBrushKind::Smooth, center_xz: [0.; 2],
        radius_meters: radius, strength: 1., speed_meters_per_second: 24.,
        delta_seconds: 1. / 60., flatten_target_meters: None }
}

#[test]
fn smoothing_broad_hills_changes_world_heights_consistently_across_author_resolutions() {
    let mut drops = Vec::new();
    for spacing in [0.125, 0.25, 0.5] {
        let (root, mut author) = smoothing_author(spacing, |[x, z]| (10. * (1. - (x*x + z*z) / (21.5*21.5))).max(0.) as f32);
        let result = author.apply_brush(smoothing_input(21.5)).unwrap();
        assert!(!result.changes.is_empty());
        let height_drop = 10. - author.sample_world_height(0., 0.).unwrap().unwrap();
        assert!((height_drop - 0.4).abs() < 0.00001, "{spacing}m作者格距：24m/s每次1/60秒应移动0.4m，实际{height_drop}m");
        drops.push(height_drop);
        drop(author);
        std::fs::remove_dir_all(root).unwrap();
    }
    assert!(drops.iter().all(|value| (value - drops[0]).abs() / drops[0] < 0.08),
        "非整除半径仅允许取整到最近源格的偏差，改变基础格距不应缩小平滑作用尺度：{drops:?}");
}

#[test]
fn smoothing_wide_plateau_reaches_beyond_a_single_source_cell() {
    let (root, mut author) = smoothing_author(0.125, |[x, z]| if x.hypot(z) < 14.4 { 10. } else { 0. });
    assert_eq!(author.sample_world_height(10., 0.).unwrap(), None, "冷会话不能先借未加载页采样");
    author.apply_brush(smoothing_input(24.)).unwrap();
    let inside_shoulder = author.sample_world_height(10., 0.).unwrap().unwrap();
    assert!((inside_shoulder - (10. - 0.4 * (1. - 10. / 24.))).abs() < 0.00001,
        "距平台边缘4m的肩部也应圆滑，固定一格四邻在这里完全不变化：{inside_shoulder}");
    assert_eq!(author.sample_world_height(0., 0.).unwrap(), Some(10.), "仍然完全平坦的中心不能凭空制造曲率");
    drop(author);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn smoothing_world_neighborhood_respects_nonuniform_object_scale_and_preserves_planes() {
    let footprint = [2., 0.5];
    let (root, mut author) = smoothing_author(0.125, |[x, z]| {
        let (x, z) = (x * 2., z * 0.5);
        (10. * (1. - (x*x + z*z) / (24.*24.))).max(0.) as f32
    });
    let input = smoothing_input(24.);
    assert_eq!(input.smoothing_sample_steps(author.spec(), footprint).unwrap(), [24, 96]);
    author.prefetch_scaled_brush_tiles(input, footprint).unwrap();
    author.apply_scaled_brush_loaded(input, footprint).unwrap();
    assert!((author.sample_world_height(0., 0.).unwrap().unwrap() - 9.6).abs() < 0.00001,
        "缩放后的两轴仍在6m世界距离采样，不能随源椭圆的长短轴改变平滑强度");
    drop(author);
    std::fs::remove_dir_all(root).unwrap();

    let (root, mut author) = smoothing_author(0.125, |[x, z]| (16. + x * 0.125 + z * 0.0625) as f32);
    let input = smoothing_input(12.);
    let result = author.apply_brush(input).unwrap();
    assert!(result.changes.is_empty(), "对称五点平均应保持已经平整的斜面，不将平滑变成整块压平");
    drop(author);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn smoothing_prefetches_far_neighbors_outside_the_write_circle_and_clamps_scene_edges() {
    let (root, mut author) = smoothing_author(0.125, |point| if point == [0.125, 0.] { 10. } else { 0. });
    let input = SparseTerrainBrushInput { center_xz: [-2.25, 0.], delta_seconds: 0.05, ..smoothing_input(2.) };
    assert_eq!(author.prefetch_scaled_brush_tiles(input, [1.; 2]).unwrap(), 4,
        "写入只在左侧两页，但0.5m采样halo需要右侧已保存尖峰的所有者页");
    let result = author.apply_brush_loaded(input).unwrap();
    assert!((author.sample_world_height(-0.375, 0.).unwrap().unwrap() - 0.075).abs() < 0.000001,
        "冻结采样必须读到圆外相邻页，不可把缺页当成平地");
    assert_eq!(author.sample_world_height(0.125, 0.).unwrap(), Some(10.), "圆外只读样本不能被平滑写入");
    assert!(result.changes.iter().all(|change| {
        let [x, z] = author.spec().grid_point_world(change.x, change.z).unwrap();
        (x + 2.25).hypot(z) < 2.
    }));
    drop(author);
    std::fs::remove_dir_all(root).unwrap();

    let (root, mut author) = smoothing_author(0.125, |point| if point == [-32.; 2] { 10. } else { 0. });
    author.apply_brush(SparseTerrainBrushInput { center_xz: [-32.; 2], speed_meters_per_second: 1000., delta_seconds: 0.05, ..smoothing_input(2.) }).unwrap();
    assert!((author.sample_world_height(-32., -32.).unwrap().unwrap() - 10. / 3.).abs() < 0.00001,
        "角点只平均自身及两个有效远邻，钳制到自身的重复样本不能重复计权");
    drop(author);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn smooth_and_flatten_share_meter_speed_strength_time_and_stop_at_the_frozen_target() {
    for kind in [SparseTerrainBrushKind::Smooth, SparseTerrainBrushKind::Flatten] {
        let (root, mut author) = smoothing_author(1., |point| if point == [0.; 2] { 10. } else { 0. });
        let base = SparseTerrainBrushInput { kind, radius_meters: 0.5, flatten_target_meters: Some(2.),
            speed_meters_per_second: 0., strength: 0.5, delta_seconds: 0.05, ..smoothing_input(0.5) };
        assert!(author.apply_brush(base).unwrap().changes.is_empty(), "零速度不能修改作者数据");
        author.apply_brush(SparseTerrainBrushInput { speed_meters_per_second: 4., ..base }).unwrap();
        assert!((author.sample_world_height(0., 0.).unwrap().unwrap() - 9.9).abs() < 0.00001);
        author.apply_brush(SparseTerrainBrushInput { speed_meters_per_second: 40., ..base }).unwrap();
        assert!((author.sample_world_height(0., 0.).unwrap().unwrap() - 8.9).abs() < 0.00001);
        let target = if kind == SparseTerrainBrushKind::Smooth { 8.9 / 5. } else { 2. };
        author.apply_brush(SparseTerrainBrushInput { speed_meters_per_second: 1000., ..base }).unwrap();
        assert!((author.sample_world_height(0., 0.).unwrap().unwrap() - target).abs() < 0.00001,
            "大步长也必须停在本刷印冻结目标，不能反向越过");
        drop(author);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn smoothing_keeps_tiny_brush_steps_and_rejects_candidate_or_page_overflow_before_io() {
    let (root, map) = isolated_scene();
    let spec = SparseHeightSpec::new([0.; 2], [8192., 64.], 0.125, -20., 80.).unwrap();
    assert_eq!(smoothing_input(0.5).smoothing_sample_steps(&spec, [1.; 2]).unwrap(), [1; 2]);
    assert!(smoothing_input(2.).smoothing_sample_steps(&spec, [0., 1.]).is_err());
    let large_scene = SceneId::parse("MAP-smooth-page-budget").unwrap();
    SparseHeightDocument::create_empty(&root, large_scene.clone(), spec).unwrap().save_dirty().unwrap();
    let mut author = SparseTerrainAuthorSession::load(&root, &large_scene).unwrap();
    let input = SparseTerrainBrushInput { center_xz: [4096., 32.], ..smoothing_input(3000.) };
    let error = author.prefetch_scaled_brush_tiles(input, [1., 10000.]).unwrap_err();
    assert!(error.contains("64页"), "细长椭圆写候选未超262144，但完整平滑工作集不能挤掉64页LRU中的必需页：{error}");
    assert_eq!(author.loaded_tile_count(), 0);
    let mut ordinary = SparseTerrainAuthorSession::load(&root, &map).unwrap();
    assert!(ordinary.prefetch_brush_tiles(smoothing_input(40.)).unwrap_err().contains("262144"));
    assert_eq!(ordinary.loaded_tile_count(), 0);
    assert!(!author.is_dirty() && !ordinary.is_dirty());
    drop((author, ordinary));
    std::fs::remove_dir_all(root).unwrap();
}
