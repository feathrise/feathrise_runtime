//! 路径弧长、旋转截面、真实密度与旧根不变回归，不以预览存在冒充雕刻成功。
use super::*;
use crate::volume::sparse::{apply_sparse_brush_batch, new_sparse_volume};

fn bounds() -> Rect {
    Rect::from_corners(
        bevy::math::Vec2::splat(-2000.),
        bevy::math::Vec2::splat(2000.),
    )
}

#[test]
fn volume_path_global_stamps_are_unique_across_turns_and_long_batches() {
    let plan = Arc::new(
        VolumePathPlan::new(
            &[[0., 0., 0.], [90., 0., 0.], [90., 3., 90.]],
            VolumePathParameters::default(),
            0.25,
            bounds(),
        )
        .unwrap(),
    );
    let mut batches = plan
        .batches(VolumeBrushKind::Dig, 0.3, 0.25, Vec3::ZERO)
        .unwrap();
    let mut index = 0u64;
    let steps = batches.total_stamps() - 1;
    let mut previous = None;
    let mut batch_count = 0;
    while let Some(batch) = batches.next_batch().unwrap() {
        assert!(batch.last < 256);
        assert!(batch.block_count() <= 128);
        for brush in batch.brushes() {
            let expected = plan.brush_at(
                plan.length() * index as f64 / steps as f64,
                VolumeBrushKind::Dig,
                0.3,
                0.25,
            );
            assert_eq!(brush.from, expected.from);
            assert_ne!(
                previous,
                Some(brush.from),
                "节点或内部批次不能重复非幂等刷印"
            );
            previous = Some(brush.from);
            index += 1;
        }
        batch_count += 1;
    }
    assert!(batch_count > 1);
    assert_eq!(index, batches.total_stamps());
}

#[test]
fn volume_path_sections_turn_with_slopes_keep_the_floor_and_translate_exactly() {
    for section in [VolumePathSection::Rectangle, VolumePathSection::Round] {
        for to in [[5., 2., 5.], [0.01, 5., 0.01]] {
            let plan = VolumePathPlan::new(
                &[[0., 0., 0.], to],
                VolumePathParameters {
                    section,
                    width: 2.,
                    height: 4.,
                    ..Default::default()
                },
                0.25,
                bounds(),
            )
            .unwrap();
            let brush = plan.brush_at(plan.length() * 0.5, VolumeBrushKind::Dig, 1., 0.25);
            assert!(brush.shape_valid(0.25, 8.));
            let floor = Vec3::from_array(to) * 0.5;
            assert!(brush.flatten_distance(floor).abs() < 0.0001);
            let center = Vec3::from_array(brush.from);
            assert!(brush.signed_distance(floor - center).abs() < 0.0001);
            let shifted = brush.translated(Vec3::new(10., -5., 12.));
            assert!(
                shifted
                    .flatten_distance(floor + Vec3::new(10., -5., 12.))
                    .abs()
                    < 0.0001
            );
            let frame = brush.frame.unwrap();
            let q = Quat::from_array(frame.rotation);
            for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                let half = Vec3::from_array(frame.half_extents);
                let surface = q * (axis * half.dot(axis));
                assert!(brush.signed_distance(surface).abs() < 0.0001);
                assert!(surface.abs().cmple(brush.sample_half_extents(0.25)).all());
            }
        }
    }
}

#[test]
fn volume_path_curve_preview_and_sculpt_share_endpoints_and_rectangular_floor() {
    let nodes = [[0., -1., 0.], [2., -1., 1.], [4., -1., 0.]];
    let plan = Arc::new(
        VolumePathPlan::new(
            &nodes,
            VolumePathParameters {
                curved: true,
                width: 1.,
                height: 1.,
                ..Default::default()
            },
            0.25,
            bounds(),
        )
        .unwrap(),
    );
    let preview = plan.preview(49);
    assert_eq!(
        preview.first().unwrap().from,
        plan.brush_at(0., VolumeBrushKind::Dig, 1., 0.25).from
    );
    assert_eq!(
        preview.last().unwrap().from,
        plan.brush_at(plan.length(), VolumeBrushKind::Dig, 1., 0.25)
            .from
    );
    assert!(
        preview
            .iter()
            .all(|b| (b.frame.unwrap().plane_normal[1] - 1.).abs() < 0.0001)
    );
    let before = new_sparse_volume(0.25, Arc::default()).unwrap();
    let mut actual = before.clone();
    let mut batches = plan
        .batches(VolumeBrushKind::Dig, 1., 0.25, Vec3::ZERO)
        .unwrap();
    while let Some(batch) = batches.next_batch().unwrap() {
        actual = apply_sparse_brush_batch(&actual, batch, bounds(), |_| Ok(2.)).unwrap();
    }
    for node in nodes {
        let p = (Vec3::from_array(node) / 0.25)
            .round()
            .as_ivec3()
            .to_array();
        assert!(
            actual.value_at([p[0], p[1] + 2, p[2]]).unwrap() > 0.,
            "洞道节点上方应实际挖开"
        );
        assert!(
            actual.value_at([p[0], p[1] - 2, p[2]]).unwrap() < 0.,
            "矩形通道底面不能继续下挖成球坑"
        );
    }
    assert_eq!(before.block_count(), 0, "雕刻不能改写可撤销的原始根");
}

#[test]
fn volume_path_rejects_invalid_nodes_and_outside_rotated_sections() {
    assert!(
        VolumePathPlan::new(
            &[[0.; 3], [0.; 3]],
            VolumePathParameters::default(),
            0.25,
            bounds()
        )
        .is_err()
    );
    assert!(
        VolumePathPlan::new(
            &[[f32::NAN, 0., 0.], [2., 0., 2.]],
            VolumePathParameters::default(),
            0.25,
            bounds()
        )
        .is_err()
    );
    let narrow = Rect::from_corners(bevy::math::Vec2::splat(-1.), bevy::math::Vec2::splat(1.));
    assert!(
        VolumePathPlan::new(
            &[[0.; 3], [0.8, 0., 0.8]],
            VolumePathParameters::default(),
            0.25,
            narrow
        )
        .is_err()
    );
}

#[test]
fn volume_path_selected_node_preview_uses_its_own_tangent() {
    let plan = VolumePathPlan::new(
        &[[0.; 3], [4., 0., 0.], [4., 0., 4.]],
        VolumePathParameters::default(),
        0.25,
        bounds(),
    )
    .unwrap();
    let first = plan.node_brush(0).unwrap().frame.unwrap();
    let last = plan.node_brush(2).unwrap().frame.unwrap();
    assert!((Quat::from_array(first.rotation) * Vec3::Z).distance(Vec3::X) < 0.0001);
    assert!((Quat::from_array(last.rotation) * Vec3::Z).distance(Vec3::Z) < 0.0001);
}


#[test]
fn volume_path_object_bounds_and_stamps_share_world_geometry_after_transform() {
    let bounds = Rect::from_corners(bevy::math::Vec2::splat(-32.), bevy::math::Vec2::splat(32.));
    for scale in [Vec3::new(0.5, 2., 1.5), Vec3::splat(2.)] {
        let transform = Mat4::from_scale_rotation_translation(
            scale, Quat::from_rotation_y(0.7), Vec3::new(100., 5., -50.),
        );
        let nodes = [Vec3::ZERO, Vec3::new(4., 1., 3.)]
            .map(|p| transform.transform_point3(p).to_array());
        for section in [VolumePathSection::Rectangle, VolumePathSection::Round] {
            let parameters = VolumePathParameters { section, ..Default::default() };
            assert!(VolumePathPlan::new(&nodes, parameters, 0.25, bounds).is_err(),
                "世界节点在源边界外，不能直接用世界坐标检查源范围");
            let plan = Arc::new(VolumePathPlan::new_in_source_frame(
                &nodes, parameters, 0.25, bounds, transform,
            ).unwrap());
            let offset = Vec3::new(-8., 3., -16.);
            let mut batches = plan.batches(VolumeBrushKind::Flatten, 1., 0.25, offset).unwrap();
            let steps = batches.total_stamps() - 1;
            let world_step = 0.25 * scale.min_element();
            assert!(plan.length() / steps as f64 <= f64::from(world_step * 0.5));
            let mut index = 0;
            while let Some(batch) = batches.next_batch().unwrap() {
                for brush in batch.brushes() {
                    let world = plan.brush_at(plan.length() * index as f64 / steps as f64,
                        VolumeBrushKind::Flatten, 1., world_step);
                    let center = Vec3::from_array(brush.from);
                    assert!(transform.transform_point3(center - offset)
                        .distance(Vec3::from_array(world.from)) < 0.0001);
                    let up = Vec3::from_array(world.frame.unwrap().plane_normal);
                    let floor = Vec3::from_array(world.from) - up * parameters.height * 0.5;
                    let local_floor = transform.inverse().transform_point3(floor) + offset;
                    assert!(brush.flatten_distance(local_floor).abs() < 0.0001);
                    assert!(brush.signed_distance(local_floor - center).abs() < 0.0001);
                    assert!(brush.shape_valid(0.25, 8.));
                    index += 1;
                }
            }
            assert_eq!(index, batches.total_stamps());
            assert!(plan.matches_source(bounds, transform));
            assert!(!plan.matches_source(bounds, Mat4::IDENTITY));
            let outside = [Vec3::new(80., 0., 0.), Vec3::new(84., 0., 0.)]
                .map(|p| transform.transform_point3(p).to_array());
            assert!(VolumePathPlan::new_in_source_frame(
                &outside, parameters, 0.25, bounds, transform,
            ).is_err(), "变换后仍必须拒绝越出源边界的完整截面");
        }
    }
}

#[test]
fn volume_path_transformed_batches_really_dig_the_selected_source() {
    let transform = Mat4::from_scale_rotation_translation(
        Vec3::new(0.5, 1., 2.), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
        Vec3::new(100., 5., -50.),
    );
    let nodes = [Vec3::new(0., -1., 0.), Vec3::new(4., -1., 0.)]
        .map(|p| transform.transform_point3(p).to_array());
    let plan = Arc::new(VolumePathPlan::new_in_source_frame(
        &nodes, VolumePathParameters { width: 1., height: 1., ..Default::default() },
        0.25, bounds(), transform,
    ).unwrap());
    let offset = Vec3::splat(-8.);
    let before = new_sparse_volume(0.25, Arc::default()).unwrap();
    let mut actual = before.clone();
    let mut batches = plan.batches(VolumeBrushKind::Dig, 1., 0.25, offset).unwrap();
    while let Some(batch) = batches.next_batch().unwrap() {
        actual = apply_sparse_brush_batch(&actual, batch, bounds(), |_| Ok(2. + offset.y)).unwrap();
    }
    let sample = |p: Vec3| actual.value_at(((p + offset) / 0.25).round().as_ivec3().to_array()).unwrap();
    assert!(sample(Vec3::new(2., -0.5, 0.)) > 0., "通道内部必须在对象源坐标中实际挖空");
    assert!(sample(Vec3::new(2., -1.5, 0.)) < 0., "世界底面映射后的下方保持实心");
    assert_eq!(before.block_count(), 0);
}
