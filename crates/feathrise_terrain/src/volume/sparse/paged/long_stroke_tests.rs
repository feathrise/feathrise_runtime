//! 超出旧整段刷印/块预算的真实长路径：局部落盘、跨批次密度与持久旧根。
use super::*;
use crate::volume::{VolumeBrushBatches, VolumeBrushShape};

#[test]
fn volume_paged_curved_path_keeps_private_batches_reopens_and_preserves_the_old_root() {
    use crate::volume::{VolumePathParameters, VolumePathPlan};
    use bevy::math::Vec3;
    let temp = Temp::new();
    let original = PagedVolume::new(0.25).unwrap();
    let mut candidate = original.clone();
    let nodes = [[4., -1., 4.], [20., -1., 12.], [44., -1., 4.]];
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
    let mut batches = plan
        .batches(VolumeBrushKind::Dig, 1., 0.25, Vec3::ZERO)
        .unwrap();
    assert!(batches.total_stamps() > 256);
    let budget = Arc::new(VolumeMemoryBudget::default());
    let mut count = 0;
    while let Some(batch) = batches.next_batch().unwrap() {
        let before = load_edit_batch_window(&temp.0, &candidate, &batch, budget.clone()).unwrap();
        let after = apply_sparse_brush_batch(&before, batch, bounds(), |_| Ok(2.07)).unwrap();
        candidate = write_delta(
            &temp.0,
            &candidate,
            prepare_delta(Some(&before), &after).unwrap(),
        )
        .unwrap();
        drop(before);
        drop(after);
        assert_eq!(budget.used_bytes(), 0);
        count += 1;
    }
    assert!(count > 1);
    let reopened = decode_paged_volume(&encode_paged_volume(&candidate).unwrap())
        .unwrap()
        .unwrap();
    for node in nodes {
        let probe = VolumeBrush {
            from: [node[0], node[1] + 0.5, node[2]],
            to: [node[0], node[1] + 0.5, node[2]],
            radius: 0.5,
            ..Default::default()
        };
        let loaded = load_edit_window(&temp.0, &reopened, probe, budget.clone()).unwrap();
        let p = probe.from.map(|v| (v / 0.25).round() as i32);
        assert!(loaded.value_at(p).unwrap() > 0.);
        assert!(loaded.value_at([p[0], -6, p[2]]).unwrap() < 0.);
    }
    assert_eq!(
        original,
        PagedVolume::new(0.25).unwrap(),
        "临时批次不能改写撤销所引用的旧根"
    );
}

#[test]
fn volume_paged_long_diagonal_carves_every_batch_and_preserves_the_original_root() {
    let temp = Temp::new();
    let original = PagedVolume::new(0.25).unwrap();
    let mut candidate = original.clone();
    let brush = VolumeBrush {
        kind: VolumeBrushKind::Dig,
        from: [4., -0.25, 4.],
        to: [84., -0.25, 84.],
        radius: 0.5,
        half_height: 0.5,
        shape: VolumeBrushShape::Box,
        ..Default::default()
    };
    let mut plan = VolumeBrushBatches::new(brush, 0.25).unwrap();
    assert!(plan.total_stamps() > 256);
    assert!(
        load_edit_window(&temp.0, &original, brush, Arc::default()).is_err(),
        "整条斜线包围盒超过旧128块门"
    );
    let budget = Arc::new(VolumeMemoryBudget::default());
    let mut boundaries = Vec::new();
    let mut count = 0;
    while let Some(batch) = plan.next_batch().unwrap() {
        let scope = batch.scope();
        boundaries.extend([scope.from, scope.to]);
        let before = load_edit_window(&temp.0, &candidate, scope, budget.clone()).unwrap();
        let after = apply_sparse_brush_batch(&before, batch, bounds(), |_| Ok(0.07)).unwrap();
        candidate = write_delta(
            &temp.0,
            &candidate,
            prepare_delta(Some(&before), &after).unwrap(),
        )
        .unwrap();
        count += 1;
        drop(before);
        drop(after);
        assert_eq!(budget.used_bytes(), 0, "上一批密度和网格不能随路径累计驻留");
    }
    assert!(count > 1 && plan.finished());
    let reopened = decode_paged_volume(&encode_paged_volume(&candidate).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(candidate, reopened);
    for point in boundaries
        .into_iter()
        .chain([brush.from, [44., -0.25, 44.], brush.to])
    {
        let sample_brush = VolumeBrush {
            from: point,
            to: point,
            ..brush
        };
        let loaded = load_edit_window(&temp.0, &reopened, sample_brush, budget.clone()).unwrap();
        let sample = point.map(|v| (v / 0.25).round() as i32);
        assert!(
            loaded.value_at(sample).unwrap() > 0.,
            "端点、中部和批次交界都必须挖开"
        );
        let old = load_edit_window(&temp.0, &original, sample_brush, budget.clone()).unwrap();
        assert_eq!(old.column_count(), 0, "临时批次不能覆盖可撤销的原始根");
    }
    assert_eq!(original, PagedVolume::new(0.25).unwrap());
}
