//! 规则刷与固定平面的真实密度/网格回归；检查平底、直墙、跨块和独立半高。
use super::{sparse::*, *};
use bevy::math::{Rect, Vec2, Vec3};
use std::sync::Arc;

fn bounds() -> Rect {
    Rect::from_corners(Vec2::splat(-64.), Vec2::splat(64.))
}
fn brush(shape: VolumeBrushShape) -> VolumeBrush {
    VolumeBrush {
        shape,
        from: [8., 8., 8.],
        to: [8., 8., 8.],
        radius: 2.,
        half_height: 1.25,
        ..Default::default()
    }
}

#[test]
fn transformed_sculpt_preserves_world_sphere_and_world_aligned_box() {
    let pose = bevy::math::Mat4::from_scale_rotation_translation(Vec3::new(4.,2.,0.5),
        bevy::math::Quat::from_rotation_y(0.7),Vec3::new(30.,12.,-10.));
    let center=pose.transform_point3(Vec3::new(8.,8.,8.));
    for shape in [VolumeBrushShape::Sphere,VolumeBrushShape::Box,VolumeBrushShape::Cylinder] {
        let world = VolumeBrush {shape,from:center.to_array(),to:center.to_array(),radius:1.,half_height:0.5,..Default::default()};
        let local = world.in_source_frame(pose); let inverse=pose.inverse();
        for offset in [Vec3::new(0.8,0.,0.),Vec3::new(1.2,0.,0.),Vec3::new(0.,0.4,0.),Vec3::new(0.,0.6,0.)] {
            let source_delta=inverse.transform_vector3(offset);
            assert_eq!(local.signed_distance(source_delta).is_sign_negative(),world.signed_distance(offset).is_sign_negative());
            let extent=local.sample_half_extents(0.25);
            if world.signed_distance(offset)<0. { assert!(source_delta.abs().cmple(extent).all()); }
        }
        assert!((Vec3::from_array(local.to)-Vec3::splat(8.)).length()<1e-4);
        assert!((local.translated(-Vec3::Y).plane_height-(local.plane_height-1.)).abs()<1e-6);
    }
}

/// 从真实三角面求竖直最上层交点，不能用预期SDF替代网格验收。
fn top(mesh: &crate::TerrainMeshInput, p: Vec2) -> f32 {
    let mut result = f32::NEG_INFINITY;
    for t in mesh.indices.chunks_exact(3) {
        let a = Vec3::from_array(mesh.positions[t[0] as usize]);
        let b = Vec3::from_array(mesh.positions[t[1] as usize]);
        let c = Vec3::from_array(mesh.positions[t[2] as usize]);
        let (u, v, q) = (
            Vec2::new(b.x - a.x, b.z - a.z),
            Vec2::new(c.x - a.x, c.z - a.z),
            p - Vec2::new(a.x, a.z),
        );
        let d = u.perp_dot(v);
        if d.abs() < 1e-8 {
            continue;
        }
        let (ub, uc) = (q.perp_dot(v) / d, u.perp_dot(q) / d);
        if ub >= -1e-5 && uc >= -1e-5 && ub + uc <= 1.00001 {
            result = result.max(a.y + ub * (b.y - a.y) + uc * (c.y - a.y));
        }
    }
    assert!(result.is_finite(), "真实网格缺面：{p:?}");
    result
}

#[test]
fn box_and_cylinder_make_flat_floors_and_box_keeps_square_corners() {
    for shape in [VolumeBrushShape::Box, VolumeBrushShape::Cylinder] {
        let before = new_sparse_volume(0.25, Arc::default()).unwrap();
        let b = brush(shape);
        let after = apply_sparse_brush(&before, b, bounds(), |_| Ok(8.)).unwrap();
        let mesh = build_author_volume_mesh(&VolumeSource::Sparse(after.clone())).unwrap();
        for x in [6.75, 7.5, 8., 8.5, 9.25] {
            for z in [7.5, 8., 8.5] {
                assert!(
                    (top(&mesh, Vec2::new(x, z)) - 6.75).abs() < 0.005,
                    "{shape:?}: 平底不平"
                );
            }
        }
        assert!(
            after.value_at([32, 26, 32]).unwrap() < 0.,
            "半高以下不能继续挖空"
        );
        assert!(after.value_at([32, 27, 32]).unwrap() >= 0.);
        // 方盒角落必须实际挖开，不能把新形状只画在光标上。
        let corner = after.value_at([38, 30, 38]).unwrap();
        if shape == VolumeBrushShape::Box {
            assert!(corner > 0.);
        } else {
            assert!(corner < 0.);
        }
        // 多个Y层同一X位置越过直墙，符号边界都位于同一世界平面。
        if shape == VolumeBrushShape::Box {
            for y in [29, 30, 31] {
                assert!(after.value_at([25, y, 32]).unwrap() > 0.);
                assert!(after.value_at([23, y, 32]).unwrap() < 0.);
            }
        }
    }
}

#[test]
fn dense_and_sparse_shapes_share_density_and_legacy_sphere_semantics() {
    let dense = volume_from_heights(64, 0.25, vec![8.; 65 * 65]).unwrap();
    for shape in [
        VolumeBrushShape::Sphere,
        VolumeBrushShape::Box,
        VolumeBrushShape::Cylinder,
    ] {
        let b = brush(shape);
        let a = apply_volume_brush(&dense, b).unwrap();
        let sparse = apply_sparse_brush(
            &new_sparse_volume(0.25, Arc::default()).unwrap(),
            b,
            bounds(),
            |_| Ok(8.),
        )
        .unwrap();
        for z in 22..=42 {
            for y in 22..=38 {
                for x in 22..=42 {
                    assert!(
                        (a.value(x, y, z)
                            - sparse.value_at([x as i32, y as i32, z as i32]).unwrap())
                        .abs()
                            <= 0.0011,
                        "{shape:?} {x}/{y}/{z}"
                    );
                }
            }
        }
        if shape == VolumeBrushShape::Sphere {
            // 球形不消费独立半高；原场景笔刷行为保持原半径。
            let ignored_height = apply_volume_brush(
                &dense,
                VolumeBrush {
                    half_height: f32::NAN,
                    ..b
                },
            )
            .unwrap();
            assert_eq!(a, ignored_height);
        }
    }
}

#[test]
fn flatten_cuts_highs_fills_lows_and_matches_one_plane_across_chunks() {
    let height = |p: Vec2| Ok(0.45 * (p.x * 2.).sin() + 0.3 * (p.y * 3.).cos());
    let b = VolumeBrush {
        kind: VolumeBrushKind::Flatten,
        shape: VolumeBrushShape::Box,
        from: [6.5, 0.07, 0.],
        to: [9.5, 0.07, 0.],
        radius: 2.,
        half_height: 1.25,
        plane_height: 0.07,
        ..Default::default()
    };
    let before = new_sparse_volume(0.25, Arc::default()).unwrap();
    let after = apply_sparse_brush(&before, b, bounds(), height).unwrap();
    let mesh = build_author_volume_mesh(&VolumeSource::Sparse(after.clone())).unwrap();
    for x in [5.5, 6.5, 7.5, 8., 8.5, 9.5, 10.5] {
        for z in [-1., 0., 1.] {
            assert!((top(&mesh, Vec2::new(x, z)) - b.plane_height).abs() < 0.003);
            assert!(
                (after
                    .value_at([(x * 4.) as i32, 0, (z * 4.) as i32])
                    .unwrap()
                    + 0.07)
                    .abs()
                    < 0.001
            );
        }
    }
    // 笔刷之外与更深地层保持原值，局部整平不能变成整列填充或封闭另一层洞穴。
    let p = [32, -8, 0];
    assert!(
        (after.value_at(p).unwrap() - (-2. - height(Vec2::new(8., 0.)).unwrap())).abs() < 0.001
    );
    assert_eq!(before.block_count(), 0, "输入快照不可原地写入");
    let bytes = encode_author_volume_glb(&VolumeSource::Sparse(after.clone()), &mesh).unwrap();
    assert_eq!(
        decode_author_volume_glb(&bytes, Arc::default()).unwrap(),
        VolumeSource::Sparse(after)
    );
}

#[test]
fn invalid_shape_parameters_fail_before_editing_and_plane_translates_with_source() {
    let source = new_sparse_volume(0.25, Arc::default()).unwrap();
    let b = brush(VolumeBrushShape::Box);
    for half_height in [f32::NAN, 0., 8.1] {
        assert!(
            apply_sparse_brush(&source, VolumeBrush { half_height, ..b }, bounds(), |_| Ok(
                8.
            ))
            .is_err()
        );
    }
    assert!(
        apply_sparse_brush(
            &source,
            VolumeBrush {
                kind: VolumeBrushKind::Flatten,
                plane_height: f32::NAN,
                ..b
            },
            bounds(),
            |_| Ok(8.)
        )
        .is_err()
    );
    let shifted = VolumeBrush {
        plane_height: 8.07,
        ..b
    }
    .translated(Vec3::new(-8., -8., -8.));
    assert_eq!(shifted.from, [0.; 3]);
    assert!((shifted.plane_height - 0.07).abs() < 1e-6);
    assert_eq!(source.block_count(), 0);
}
