//! 连续越过旧盒子底部与区块边界，共享接缝、缓存复用和有界文件往返。
use super::*;
use crate::volume::{VolumeBrush, VolumeBrushKind};
use bevy::math::{Rect, Vec2};
use std::sync::Arc;
fn brush(from: [f32; 3], to: [f32; 3]) -> VolumeBrush {
    VolumeBrush {
        kind: VolumeBrushKind::Dig,
        from,
        to,
        radius: 2.,
        strength: 1.,
        ..Default::default()
    }
}
fn bounds() -> Rect {
    Rect::from_corners(Vec2::splat(-256.), Vec2::splat(256.))
}
#[test]
fn sparse_volume_continues_downward_and_sideways_without_internal_guard_walls() {
    let mut source = new_sparse_volume(0.25, Arc::default()).unwrap();
    for y in (0..=24).step_by(4) {
        source = apply_sparse_brush(
            &source,
            brush([0., -(y as f32), 0.], [0., -(y as f32 + 4.), 0.]),
            bounds(),
            |_| Ok(0.),
        )
        .unwrap();
    }
    source = apply_sparse_brush(
        &source,
        brush([0., -24., 0.], [32., -24., 0.]),
        bounds(),
        |_| Ok(0.),
    )
    .unwrap();
    assert!(source.value_at([0, -96, 0]).unwrap() > 0.);
    assert!(source.value_at([128, -96, 0]).unwrap() > 0.);
    assert!(source.value_at([64, -120, 0]).unwrap() < 0.);
    let source = VolumeSource::Sparse(source);
    let mesh = build_author_volume_mesh(&source).unwrap();
    assert_welded(&mesh, [-8., -8.], [40., 8.]);
    let index = crate::build_terrain_editing_index(&mesh).unwrap();
    let roof = index
        .ray_intersection(
            &mesh.positions,
            &mesh.indices,
            [16., -24., 0.],
            [0., 1., 0.],
        )
        .unwrap();
    let floor = index
        .ray_intersection(
            &mesh.positions,
            &mesh.indices,
            [16., -24., 0.],
            [0., -1., 0.],
        )
        .unwrap();
    assert!((roof.position[1] + 22.).abs() < 0.1);
    assert!((floor.position[1] + 26.).abs() < 0.1);
    let bytes = encode_author_volume_glb(&source, &mesh).unwrap();
    let reopened = decode_author_volume_glb(&bytes, Arc::default()).unwrap();
    assert_eq!(reopened, source);
    assert_eq!(build_author_volume_mesh(&reopened).unwrap(), mesh);
}

/// 地表外缘允许开边，区块共享面、洞壁和洞顶内部只能由恰好两个三角面共用一条边。
pub(super) fn assert_welded(mesh: &crate::TerrainMeshInput, min: [f32; 2], max: [f32; 2]) {
    use std::collections::{HashMap, HashSet};
    let unique: HashSet<_> = mesh
        .positions
        .iter()
        .map(|p| p.map(|v| if v == 0. { 0 } else { v.to_bits() }))
        .collect();
    assert_eq!(
        unique.len(),
        mesh.positions.len(),
        "共享交点必须焊成一个顶点"
    );
    let mut edges = HashMap::new();
    for tri in mesh.indices.chunks_exact(3) {
        for [a, b] in [[tri[0], tri[1]], [tri[1], tri[2]], [tri[2], tri[0]]] {
            *edges.entry([a.min(b), a.max(b)]).or_insert(0) += 1;
        }
    }
    for ([a, b], count) in edges {
        assert!(
            count == 1 || count == 2,
            "非流形边数量{count}：{:?} -> {:?}",
            mesh.positions[a as usize],
            mesh.positions[b as usize]
        );
        if count == 1 {
            let (a, b) = (mesh.positions[a as usize], mesh.positions[b as usize]);
            assert!(
                [(0, min[0]), (0, max[0]), (2, min[1]), (2, max[1])]
                    .iter()
                    .any(|&(axis, edge)| (a[axis] - edge).abs() < 1e-5
                        && (b[axis] - edge).abs() < 1e-5),
                "内部共享面有未焊接裂缝：{a:?} -> {b:?}"
            );
        }
    }
}

#[test]
fn sparse_volume_budget_failure_and_missing_height_keep_source_and_accounting_unchanged() {
    let budget = Arc::new(super::types::VolumeMemoryBudget::limited(16 * 1024));
    let source = new_sparse_volume(0.25, budget.clone()).unwrap();
    assert!(apply_sparse_brush(&source, brush([0.; 3], [0.; 3]), bounds(), |_| Ok(0.)).is_err());
    assert_eq!(budget.used_bytes(), 0);
    assert_eq!(source.column_count(), 0);
    let source = new_sparse_volume(0.25, Arc::default()).unwrap();
    assert!(
        apply_sparse_brush(&source, brush([0.; 3], [0.; 3]), bounds(), |_| Err(
            "缺少同代页".into()
        ))
        .is_err()
    );
    assert!(source.blocks.is_empty() && source.meshes.is_empty() && source.columns.is_empty());
}

#[test]
fn sparse_volume_budget_reserves_legacy_history_before_allocating_migrated_blocks() {
    let budget = Arc::new(super::types::VolumeMemoryBudget::limited(16 * 1024));
    let retained = budget.retain(vec![0u8; 8192], 8192).unwrap();
    budget.reserve_legacy_bytes(8192).unwrap();
    assert!(budget.retain(vec![0u8; 1024], 1024).is_err());
    assert!(budget.reserve_legacy_bytes(8193).is_err());
    assert_eq!(budget.used_bytes(), 8192);
    drop(retained);
    assert_eq!(budget.used_bytes(), 0);
    let retained = budget.retain(vec![0u8; 8192], 8192).unwrap();
    assert!(
        budget.retain(vec![0u8; 1], 1).is_err(),
        "失败的旧历史预留不能清除上一完整额度"
    );
    drop(retained);
    budget.reserve_legacy_bytes(0).unwrap();
    let retained = budget.retain(vec![0u8; 16384], 16384).unwrap();
    assert_eq!(budget.used_bytes(), 16384);
    drop(retained);
    assert_eq!(budget.used_bytes(), 0);
}

#[test]
fn sparse_volume_new_columns_keep_frozen_shared_edges_and_non_binary_grid_has_no_seams() {
    let initial = new_sparse_volume(0.1, Arc::default()).unwrap();
    let first = apply_sparse_brush(&initial, brush([0.; 3], [0.; 3]), bounds(), |p| {
        Ok(p.x * 0.01)
    })
    .unwrap();
    let next = apply_sparse_brush(&first, brush([3.2, 0., 0.], [3.2, 0., 0.]), bounds(), |p| {
        Ok(p.x * 0.01 + 0.1)
    })
    .unwrap();
    next.validate().unwrap();
    let mesh = build_author_volume_mesh(&VolumeSource::Sparse(next.clone())).unwrap();
    let rect = next.footprint()[0];
    assert_eq!(next.footprint().len(), 1);
    assert_welded(&mesh, rect[0], rect[1]);
    let mut rebuilt = next.clone();
    super::brush::rebuild_all(&mut rebuilt).unwrap();
    assert_eq!(
        mesh,
        build_author_volume_mesh(&VolumeSource::Sparse(rebuilt)).unwrap(),
        "局部重建必须与完整源重建相同"
    );
}

#[test]
fn sparse_volume_disconnected_edits_leave_middle_clipmap_columns_intact() {
    let initial = new_sparse_volume(0.25, Arc::default()).unwrap();
    let first =
        apply_sparse_brush(&initial, brush([0.; 3], [0.; 3]), bounds(), |_| Ok(0.)).unwrap();
    let next = apply_sparse_brush(
        &first,
        brush([40., 0., 0.], [40., 0., 0.]),
        bounds(),
        |_| Ok(0.),
    )
    .unwrap();
    assert_eq!(next.footprint().len(), 2);
    assert!(
        !next
            .footprint()
            .iter()
            .any(|r| 20. > r[0][0] && 20. < r[1][0])
    );
    let source = VolumeSource::Sparse(next);
    let mesh = build_author_volume_mesh(&source).unwrap();
    let reopened = decode_author_volume_glb(
        &encode_author_volume_glb(&source, &mesh).unwrap(),
        Arc::default(),
    )
    .unwrap();
    assert_eq!(source, reopened);
    assert_eq!(source.footprint(), reopened.footprint());
}

#[test]
fn sparse_volume_migrates_legacy_density_and_preserves_original_source() {
    let original = crate::volume::volume_from_heights(64, 0.25, vec![8.; 65 * 65]).unwrap();
    let original =
        crate::volume::apply_volume_brush(&original, brush([8., 8., 8.], [8., 6., 8.])).unwrap();
    let saved = original.clone();
    let migrated = migrate_dense_volume(&original, Arc::default()).unwrap();
    for z in 0..64 {
        for y in 0..64 {
            for x in 0..64 {
                assert_eq!(
                    migrated.sample([x, y, z]).unwrap(),
                    original.density[original.index(x as usize, y as usize, z as usize)]
                );
            }
        }
    }
    let next = apply_sparse_brush(
        &migrated,
        brush([8., 6., 8.], [8., -4., 8.]),
        bounds(),
        |_| Ok(8.),
    )
    .unwrap();
    assert!(next.value_at([32, -16, 32]).unwrap() > 0.);
    assert_eq!(original, saved);
}

#[test]
fn sparse_volume_real_floor_hits_keep_digging_beyond_the_old_box_and_fill_smooth_cross_seams() {
    let mut source = new_sparse_volume(0.25, Arc::default()).unwrap();
    let mut point = [0.; 3];
    for _ in 0..10 {
        source = apply_sparse_brush(&source, brush(point, point), bounds(), |_| Ok(0.)).unwrap();
        let mesh = build_author_volume_mesh(&VolumeSource::Sparse(source.clone())).unwrap();
        point = crate::build_terrain_editing_index(&mesh)
            .unwrap()
            .ray_intersection(&mesh.positions, &mesh.indices, [0., 4., 0.], [0., -1., 0.])
            .unwrap()
            .position;
    }
    assert!(point[1] < -18., "每一笔都必须继续命中新洞底：{point:?}");
    source = apply_sparse_brush(
        &source,
        brush([0., -8., 0.], [12., -8., 0.]),
        bounds(),
        |_| Ok(0.),
    )
    .unwrap();
    let fill = VolumeBrush {
        kind: VolumeBrushKind::Fill,
        from: [6., -8., 0.],
        to: [10., -8., 0.],
        ..brush([0.; 3], [0.; 3])
    };
    let filled = apply_sparse_brush(&source, fill, bounds(), |_| {
        panic!("已有列的填充不应重读高度")
    })
    .unwrap();
    assert!(filled.value_at([32, -32, 0]).unwrap() < 0.);
    assert!(source.value_at([32, -32, 0]).unwrap() > 0.);
    let smooth = VolumeBrush {
        kind: VolumeBrushKind::Smooth,
        strength: 0.2,
        ..fill
    };
    let smoothed = apply_sparse_brush(&filled, smooth, bounds(), |_| {
        panic!("已有列的平滑不应重读高度")
    })
    .unwrap();
    let mesh = build_author_volume_mesh(&VolumeSource::Sparse(smoothed.clone())).unwrap();
    let rect = smoothed.footprint()[0];
    assert_welded(&mesh, rect[0], rect[1]);
    let flat = new_sparse_volume(0.25, Arc::default()).unwrap();
    let flat = apply_sparse_brush(
        &flat,
        VolumeBrush {
            kind: VolumeBrushKind::Smooth,
            ..brush([0.; 3], [0.; 3])
        },
        bounds(),
        |_| Ok(0.),
    )
    .unwrap();
    assert_eq!(flat.column_count(), 0, "无效平滑不能仅为白模接管分配地表列");
}

#[test]
fn sparse_volume_corrupt_directories_and_extreme_heights_are_rejected_before_mesh_work() {
    let initial = new_sparse_volume(0.25, Arc::default()).unwrap();
    let source = VolumeSource::Sparse(
        apply_sparse_brush(&initial, brush([0.; 3], [0.; 3]), bounds(), |_| Ok(0.)).unwrap(),
    );
    let mesh = build_author_volume_mesh(&source).unwrap();
    let bytes = encode_author_volume_glb(&source, &mesh).unwrap();
    let (document, binary) = crate::glb::parse_glb_chunks(&bytes).unwrap();
    for reason in 0..4 {
        let mut damaged = document.clone();
        let mut bin = binary.to_vec();
        let meta = &mut damaged["asset"]["extras"]["rpg_editor_volume_v2"];
        match reason {
            0 => {
                let duplicate = meta["columns"][0].clone();
                meta["columns"].as_array_mut().unwrap().push(duplicate);
            }
            1 => {
                meta["blocks"] = serde_json::json!(vec![[0, 0, 0, 0]; 513]);
            }
            2 => {
                let view = meta["columns"][0][2].as_u64().unwrap() as usize;
                let offset = damaged["bufferViews"][view]["byteOffset"].as_u64().unwrap() as usize;
                bin[offset..offset + 4].copy_from_slice(&f32::MAX.to_le_bytes());
            }
            _ => {
                damaged["asset"]["extras"]["padding"] = serde_json::json!("x".repeat(256 * 1024));
            }
        }
        let damaged =
            super::super::codec::pack(damaged, &bin, MAX_AUTHOR_VOLUME_GLB_BYTES).unwrap();
        let budget: Arc<VolumeMemoryBudget> = Arc::default();
        assert!(
            decode_author_volume_glb(&damaged, budget.clone()).is_err(),
            "损坏情形{reason}不应被接管"
        );
        assert_eq!(budget.used_bytes(), 0, "解码失败不能泄漏额度");
    }
}
#[test]
fn sparse_volume_only_rebuilds_edited_blocks_and_shares_other_sources() {
    let initial = new_sparse_volume(0.25, Arc::default()).unwrap();
    let first = apply_sparse_brush(
        &initial,
        brush([0., 0., 0.], [32., 0., 0.]),
        bounds(),
        |_| Ok(0.),
    )
    .unwrap();
    assert_eq!(
        first.budget.used_bytes(),
        first.memory_bytes()
            + first
                .meshes
                .values()
                .map(|m| m.positions.capacity() * 12 + m.indices.capacity() * 4)
                .sum::<usize>(),
        "缓存按容量计费，不能只计算已用长度"
    );
    let next = apply_sparse_brush(
        &first,
        brush([0., -2., 0.], [0., -4., 0.]),
        bounds(),
        |_| panic!("已有列不能重新取基础高度"),
    )
    .unwrap();
    let shared = first
        .meshes
        .iter()
        .filter(|(k, v)| next.meshes.get(*k).is_some_and(|w| Arc::ptr_eq(v, w)))
        .count();
    assert!(shared > 0);
    assert!(next.last_rebuilt_blocks < first.meshes.len() + 16);
    assert!(first.value_at([0, -16, 0]).unwrap() < 0.);
    assert!(next.value_at([0, -16, 0]).unwrap() > 0.);
}
