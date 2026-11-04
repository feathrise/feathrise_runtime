//! 密度、真实多层拓扑、块接缝和源/网格同文件往返回归。
use super::*;
use bevy::math::Vec3;
fn flat(cells: u32) -> VolumeData {
    volume_from_heights(
        cells,
        0.5,
        vec![cells as f32 * 0.25; (cells as usize + 1).pow(2)],
    )
    .unwrap()
}

fn assert_protected_samples_unchanged(before: &VolumeData, after: &VolumeData) {
    let cells = before.cells as usize;
    for z in 0..before.side() {
        for y in 0..before.side() {
            for x in 0..before.side() {
                if [x, y, z].iter().any(|&i| i <= 2 || i >= cells - 2) {
                    let at = before.index(x, y, z);
                    assert_eq!(
                        before.density[at], after.density[at],
                        "保护带样本改变：{x}/{y}/{z}"
                    );
                }
            }
        }
    }
}

#[test]
fn dig_fill_and_reopen_preserve_three_dimensional_source() {
    let source = flat(16);
    let brush = VolumeBrush {
        kind: VolumeBrushKind::Dig,
        from: [4., 4., 4.],
        to: [4., 3., 4.],
        radius: 1.,
        strength: 1.,
        ..Default::default()
    };
    let dug = apply_volume_brush(&source, brush).unwrap();
    assert!(dug.value(8, 6, 8) > 0.0);
    assert!(source.value(8, 6, 8) < 0.0);
    assert!(dug.value(8, 3, 8) < 0.0); // 洞底以下仍是岩土。
    let mesh = build_volume_mesh(&dug).unwrap();
    let bytes = encode_volume_glb(&dug, &mesh).unwrap();
    assert_eq!(decode_volume_glb(&bytes).unwrap(), dug);
    assert_eq!(crate::parse_dynamic_terrain_glb(&bytes).unwrap(), mesh);
    let filled = apply_volume_brush(
        &dug,
        VolumeBrush {
            kind: VolumeBrushKind::Fill,
            ..brush
        },
    )
    .unwrap();
    assert!(filled.value(8, 6, 8) < 0.0);
}

#[test]
fn shared_block_plane_has_welded_indices_and_upward_original_ground() {
    let source = flat(16);
    let mesh = build_volume_mesh(&source).unwrap();
    let mut points = std::collections::HashSet::new();
    for p in &mesh.positions {
        assert!(points.insert(p.map(f32::to_bits)));
        assert_eq!(p[1], 4.0);
    }
    let normals = crate::derive_vertex_normals(&mesh).unwrap();
    assert!(
        normals
            .iter()
            .all(|p| Vec3::from_array(*p).dot(Vec3::Y) > 0.999)
    );
    // 三角面积完整覆盖64m²，内部八块既不能重叠也不能漏格。
    let area: f32 = mesh
        .indices
        .chunks_exact(3)
        .map(|i| {
            let p = i
                .iter()
                .map(|i| Vec3::from_array(mesh.positions[*i as usize]))
                .collect::<Vec<_>>();
            (p[1] - p[0]).cross(p[2] - p[0]).length() / 2.0
        })
        .sum();
    assert!((area - 64.0).abs() < 0.0001);
}

#[test]
fn outside_region_rejection_is_atomic_and_smoothing_keeps_outer_height() {
    let source = flat(16);
    assert!(
        apply_volume_brush(
            &source,
            VolumeBrush {
                kind: VolumeBrushKind::Dig,
                from: [-0.5, 4., 4.],
                to: [4., 4., 4.],
                radius: 1.,
                strength: 1.,
                ..Default::default()
            }
        )
        .is_err()
    );
    let dug = apply_volume_brush(
        &source,
        VolumeBrush {
            kind: VolumeBrushKind::Dig,
            from: [4., 4., 4.],
            to: [4., 4., 4.],
            radius: 1.,
            strength: 1.,
            ..Default::default()
        },
    )
    .unwrap();
    let smooth = apply_volume_brush(
        &dug,
        VolumeBrush {
            kind: VolumeBrushKind::Smooth,
            from: [4., 4., 4.],
            to: [4., 4., 4.],
            radius: 1.5,
            strength: 1.,
            ..Default::default()
        },
    )
    .unwrap();
    assert_ne!(smooth.density, dug.density);
    assert_protected_samples_unchanged(&source, &smooth);
}

#[test]
fn volume_dig_keeps_following_real_floor_hits_when_the_brush_reaches_boundary() {
    // 精细作者源的8m区域配2m笔刷第二次落笔就会触及旧整球门；32m区域也必须能挖到真实底带。
    for step in [0.125, 0.5] {
        let extent = 64.0 * step;
        let original = volume_from_heights(64, step, vec![extent / 2.0; 65 * 65]).unwrap();
        let mut source = original.clone();
        let center = extent / 2.0;
        let mut previous_height = center;
        let mut clipped_changes = 0;
        for _ in 0..20 {
            let mesh = build_volume_mesh(&source).unwrap();
            let index = crate::build_terrain_editing_index(&mesh).unwrap();
            let floor = index
                .ray_intersection(
                    &mesh.positions,
                    &mesh.indices,
                    [center, extent, center],
                    [0., -1., 0.],
                )
                .unwrap();
            assert!(floor.position[1] <= previous_height + 0.001);
            previous_height = floor.position[1];
            let brush = VolumeBrush {
                kind: VolumeBrushKind::Dig,
                from: floor.position,
                to: floor.position,
                radius: 2.,
                strength: 1.,
                ..Default::default()
            };
            let next = apply_volume_brush(&source, brush).unwrap();
            if next == source {
                assert!(brush.is_clipped_by_boundary(&source));
                break;
            }
            clipped_changes += usize::from(brush.is_clipped_by_boundary(&source));
            source = next;
        }
        assert!(clipped_changes > 0, "触边后必须仍有成功的连续挖掘");
        assert!(
            previous_height < 3.0 * step,
            "仍在真实底部保护带之前停住：{previous_height}"
        );
        assert_protected_samples_unchanged(&original, &source);
        let mesh = build_volume_mesh(&source).unwrap();
        let bytes = encode_volume_glb(&source, &mesh).unwrap();
        assert_eq!(decode_volume_glb(&bytes).unwrap(), source);
        assert_eq!(crate::parse_dynamic_terrain_glb(&bytes).unwrap(), mesh);
        assert_eq!(source.memory_bytes(), original.memory_bytes()); // 裁剪不扩大密度、网格或历史预算。
    }
}

#[test]
fn volume_clipped_dig_fill_and_smooth_preserve_all_six_protected_bands() {
    let original = flat(16);
    let mut source = original.clone();
    for axis in 0..3 {
        for edge in [0.0, source.extent()] {
            let mut point = [4.; 3];
            point[axis] = edge;
            for kind in [
                VolumeBrushKind::Dig,
                VolumeBrushKind::Fill,
                VolumeBrushKind::Smooth,
            ] {
                let brush = VolumeBrush {
                    kind,
                    from: point,
                    to: point,
                    radius: 2.,
                    strength: 1.,
                    ..Default::default()
                };
                assert!(brush.is_clipped_by_boundary(&source));
                source = apply_volume_brush(&source, brush).unwrap();
                assert_protected_samples_unchanged(&original, &source);
            }
        }
    }
    assert_ne!(source.density, original.density);
    let mesh = build_volume_mesh(&source).unwrap();
    assert!(crate::derive_vertex_normals(&mesh).is_ok());
    assert_eq!(
        decode_volume_glb(&encode_volume_glb(&source, &mesh).unwrap()).unwrap(),
        source
    );
}

#[test]
fn corrupt_source_cannot_overallocate_and_truncated_glb_is_rejected() {
    let mut source = flat(8);
    source.cells = u32::MAX;
    assert!(source.validate().is_err());
    let source = flat(8);
    let mesh = build_volume_mesh(&source).unwrap();
    let mut bytes = encode_volume_glb(&source, &mesh).unwrap();
    bytes.pop();
    assert!(decode_volume_glb(&bytes).is_err());
}

#[test]
fn volume_tunnel_crosses_internal_blocks_with_roof_floor_and_original_surface() {
    let started = std::time::Instant::now();
    let source = flat(64);
    let brush = VolumeBrush {
        kind: VolumeBrushKind::Dig,
        from: [12., 16., 16.],
        to: [12., 12., 16.],
        radius: 1.25,
        strength: 1.,
        ..Default::default()
    };
    let shaft = apply_volume_brush(&source, brush).unwrap();
    let tunnel = apply_volume_brush(
        &shaft,
        VolumeBrush {
            from: [12., 12., 16.],
            to: [20., 12., 16.],
            ..brush
        },
    )
    .unwrap();
    let mesh = build_volume_mesh(&tunnel).unwrap();
    let index = crate::build_terrain_editing_index(&mesh).unwrap();
    let ray = |from, direction| {
        index
            .ray_intersection(&mesh.positions, &mesh.indices, from, direction)
            .unwrap()
    };
    let roof = ray([16., 12., 16.], [0., 1., 0.]);
    let floor = ray([16., 12., 16.], [0., -1., 0.]);
    let ground = ray([16., 20., 16.], [0., -1., 0.]);
    assert!((roof.position[1] - 13.25).abs() < 0.1);
    assert!((floor.position[1] - 10.75).abs() < 0.1);
    assert_eq!(ground.position[1], 16.0); // 同一XZ有洞顶、洞底和室外地表，不能被高度图表示。
    let mut edges = std::collections::HashMap::new();
    for triangle in mesh.indices.chunks_exact(3) {
        for (a, b) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            *edges.entry((a.min(b), a.max(b))).or_insert(0usize) += 1;
        }
    }
    for ((a, b), count) in edges {
        if count == 1 {
            let a = mesh.positions[a as usize];
            let b = mesh.positions[b as usize];
            assert!(
                [0, 2]
                    .iter()
                    .any(|&axis| (a[axis] == 0. && b[axis] == 0.)
                        || (a[axis] == 32. && b[axis] == 32.)),
                "内部块或洞壁出现未焊接边"
            );
        } else {
            assert_eq!(count, 2, "内部三角边必须是流形公共边");
        }
    }
    let bytes = encode_volume_glb(&tunnel, &mesh).unwrap();
    assert_eq!(decode_volume_glb(&bytes).unwrap(), tunnel);
    assert!(bytes.len() < MAX_VOLUME_GLB_BYTES);
    eprintln!(
        "64³洞穴计算、BVH与编码诊断：{:?}，{}顶点/{}三角/{}字节",
        started.elapsed(),
        mesh.positions.len(),
        mesh.indices.len() / 3,
        bytes.len()
    );
}

#[test]
fn volume_fill_can_close_a_dug_tunnel_without_changing_boundary_samples() {
    let source = flat(32);
    let brush = VolumeBrush {
        kind: VolumeBrushKind::Dig,
        from: [5., 8., 8.],
        to: [11., 6., 8.],
        radius: 1.5,
        strength: 1.,
        ..Default::default()
    };
    let dug = apply_volume_brush(&source, brush).unwrap();
    let filled = apply_volume_brush(
        &dug,
        VolumeBrush {
            kind: VolumeBrushKind::Fill,
            ..brush
        },
    )
    .unwrap();
    assert!(filled.value(16, 12, 16) < 0.);
    for z in 0..source.side() {
        for y in 0..source.side() {
            for x in [0, 1, 31, 32] {
                assert_eq!(source.value(x, y, z), filled.value(x, y, z));
            }
        }
    }
}

#[test]
fn volume_small_valid_faces_and_isolated_zero_samples_remain_saveable() {
    let tiny = crate::TerrainMeshInput {
        positions: vec![[0., 0., 0.], [0., 0., 0.002], [0.002, 0., 0.]],
        indices: vec![0, 1, 2],
    };
    assert!(crate::derive_vertex_normals(&tiny).is_ok()); // 合法毫米级面不能被高度投影阈值误杀。
    let mut source = flat(16);
    let i = source.index(8, 2, 8);
    source.density[i] = 0;
    let mesh = build_volume_mesh(&source).unwrap();
    assert!(crate::derive_vertex_normals(&mesh).is_ok()); // 孤立零点没有曲面，不留下未引用顶点。
    assert!(encode_volume_glb(&source, &mesh).is_ok());
}
