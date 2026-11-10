//! 三维网格吸附的真实BVH适配回归，覆盖坡面、分离双层、边界与完整快照等待。
use super::*;
use crate::scenes::dynamic_scene::terrain_topology::TerrainMeshQuery;
use avian3d::prelude::Collider;

// 【回归设计】生产包装在碰撞域中保持私有；直接编译同一源文件，测试真实Custom接口而非仿制包装。
#[path = "../../../shared/placement_data/terrain_collision/static_shape.rs"]
mod production_static_shape;

fn slope_surface(slope: f32, upper_layer: bool, matrix: Mat4) -> ClipmapGroundSnapshot {
    let mut snapshot = super::tests::snapshot();
    let mut vertices = Vec::new();
    let mut triangles = Vec::new();
    for offset in if upper_layer {
        vec![0.0, 0.25]
    } else {
        vec![0.0]
    } {
        let base = vertices.len() as u32;
        vertices.extend([
            Vec3::new(0.0, offset, 0.0),
            Vec3::new(0.0, offset, 2.0),
            Vec3::new(2.0, 2.0 * slope + offset, 2.0),
            Vec3::new(2.0, 2.0 * slope + offset, 0.0),
        ]);
        triangles.extend([[base, base + 1, base + 2], [base, base + 2, base + 3]]);
    }
    snapshot.terrain_meshes =
        Arc::from([TerrainMeshQuery::new(Collider::trimesh(vertices, triangles), matrix).unwrap()]);
    snapshot
}

fn assert_hit(actual: ClipmapGroundHit, expected: Vec3) {
    let ClipmapGroundHit::Hit(point) = actual else {
        panic!("连续承托应命中 {expected:?}，实际为 {actual:?}");
    };
    assert!(
        point.distance(expected) < 0.0001,
        "实际 {point:?}，预期 {expected:?}"
    );
}

#[test]
fn continuous_surface_snap_tracks_uphill_and_downhill_beyond_half_a_meter() {
    let translation = Vec3::new(22.0, 0.0, 12.0);
    let snapshot = slope_surface(1.0, false, Mat4::from_translation(translation));
    for (start_x, target_x) in [(0.2, 1.0), (1.8, 1.0)] {
        let start = translation + Vec3::new(start_x, start_x, 1.0);
        let expected = translation + Vec3::new(target_x, target_x, 1.0);
        assert_hit(
            snapshot.sample_support_along_surface(start, expected.xz()),
            expected,
        );
    } // 两向高度差均0.8米，旧固定抬高0.5米射线无法覆盖上坡。
}

#[test]
fn continuous_surface_snap_preserves_the_selected_layer_between_close_parallel_floors() {
    let translation = Vec3::new(22.0, 0.0, 12.0);
    let snapshot = slope_surface(1.0, true, Mat4::from_translation(translation));
    for offset in [0.0, 0.25] {
        let start = translation + Vec3::new(0.2, 0.2 + offset, 1.0);
        let expected = translation + Vec3::new(1.5, 1.5 + offset, 1.0);
        assert_hit(
            snapshot.sample_support_along_surface(start, expected.xz()),
            expected,
        );
    } // 上层仅相隔0.25米，按最近Y或抬高射线都会有改选另一层的可能。
}

#[test]
fn continuous_surface_snap_rejects_an_unsupported_edge_without_exterior_fallback() {
    let translation = Vec3::new(22.0, 0.0, 12.0);
    let snapshot = slope_surface(0.0, false, Mat4::from_translation(translation));
    assert_eq!(
        snapshot.sample_support_along_surface(
            translation + Vec3::new(1.8, 0.0, 1.0),
            Vec2::new(25.0, 13.0)
        ),
        ClipmapGroundHit::Miss,
    ); // 目标仍在完整高度图内，但当前真实承托面到边界即终止，不能落到室外包络。
}

#[test]
fn continuous_surface_snap_respects_world_rotation_scale_and_translation() {
    let matrix = Mat4::from_scale_rotation_translation(
        Vec3::new(2.0, 3.0, 0.5),
        Quat::from_rotation_y(0.6),
        Vec3::new(22.0, -4.0, 12.0),
    );
    let snapshot = slope_surface(1.0, false, matrix);
    let start = matrix.transform_point3(Vec3::new(0.2, 0.2, 1.0));
    let expected = matrix.transform_point3(Vec3::new(1.5, 1.5, 1.0));
    assert_hit(
        snapshot.sample_support_along_surface(start, expected.xz()),
        expected,
    );
}

#[test]
fn continuous_surface_snap_waits_for_the_complete_snapshot_and_rejects_invalid_origins() {
    let mut snapshot = slope_surface(0.0, false, Mat4::IDENTITY);
    snapshot.ready = false;
    assert_eq!(
        snapshot.sample_support_along_surface(Vec3::ONE, Vec2::ONE),
        ClipmapGroundHit::WaitingForHeight
    );
    snapshot.ready = true;
    assert_eq!(
        snapshot.sample_support_along_surface(Vec3::ONE, Vec2::ONE),
        ClipmapGroundHit::Miss
    );
    assert_eq!(
        snapshot.sample_support_along_surface(Vec3::splat(f32::NAN), Vec2::ONE),
        ClipmapGroundHit::Failed
    );
}

#[test]
fn continuous_surface_snap_uses_the_real_custom_static_terrain_wrapper() {
    use avian3d::parry::shape::{SharedShape, TriMesh};

    let vertices = vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 2.0),
        Vec3::new(2.0, 2.0, 2.0),
        Vec3::new(2.0, 2.0, 0.0),
        Vec3::new(0.0, 0.25, 0.0),
        Vec3::new(0.0, 0.25, 2.0),
        Vec3::new(2.0, 2.25, 2.0),
        Vec3::new(2.0, 2.25, 0.0),
    ];
    let mesh = TriMesh::new(vertices, vec![[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]]).unwrap();
    let collider = Collider::from(SharedShape::new(
        production_static_shape::StaticTerrainShape::new(mesh),
    ));
    assert!(collider.shape().as_trimesh().is_none()); // 真正生产包装不会暴露as_trimesh。
    assert!(collider.shape().as_composite_shape().is_some()); // 查询必须通过这里得到真实三角BVH。

    let translation = Vec3::new(22.0, -3.0, 12.0);
    let mut snapshot = super::tests::snapshot();
    snapshot.terrain_meshes =
        Arc::from([TerrainMeshQuery::new(collider, Mat4::from_translation(translation)).unwrap()]);
    let start = translation + Vec3::new(0.2, 0.2, 1.0);
    let expected = translation + Vec3::new(1.5, 1.5, 1.0);
    assert_hit(
        snapshot.sample_support_along_surface(start, expected.xz()),
        expected,
    );
}

#[test]
fn surface_ray_preserves_winding_for_wrapped_meshes_from_both_sides() {
    use avian3d::parry::shape::{SharedShape, TriMesh};

    let matrix = Mat4::from_scale_rotation_translation(
        Vec3::new(2.0, 3.0, 0.5),
        Quat::from_rotation_y(0.6),
        Vec3::new(22.0, -4.0, 12.0),
    );
    for upward in [true, false] {
        for wrapped in [false, true] {
            let vertices = vec![
                Vec3::new(-2.0, 0.0, -2.0),
                Vec3::new(-2.0, 0.0, 2.0),
                Vec3::new(2.0, 0.0, 2.0),
                Vec3::new(2.0, 0.0, -2.0),
            ];
            let indices = if upward {
                vec![[0, 1, 2], [0, 2, 3]]
            } else {
                vec![[0, 2, 1], [0, 3, 2]]
            };
            let collider = if wrapped {
                Collider::from(SharedShape::new(production_static_shape::StaticTerrainShape::new(
                    TriMesh::new(vertices, indices).unwrap(),
                )))
            } else {
                Collider::trimesh(vertices, indices)
            };
            let query = TerrainMeshQuery::new(collider, matrix).unwrap();
            let expected = if upward { Vec3::Y } else { Vec3::NEG_Y };
            for side in [-1.0, 1.0] {
                let origin = matrix.transform_point3(Vec3::new(0.25, side, 0.0));
                let direction = Vec3::new(0.0, -side, 0.0);
                let (distance, normal) = query.ray_normal(origin, direction, 10.0).unwrap();
                assert!((distance - 3.0).abs() < 0.0001);
                assert!(normal.dot(expected) > 0.9999,
                    "真实绕序不能因射线反向改变：upward={upward} wrapped={wrapped} side={side} normal={normal:?}");
            }
        }
    }
}

#[test]
fn wrapped_ceiling_backface_cannot_authorize_ground_support() {
    use avian3d::parry::shape::{SharedShape, TriMesh};

    // 【回归设计】使用正式Custom碰撞包装；单纯raw Trimesh会绕过原先遗漏的分支。
    let mesh = TriMesh::new(
        vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 2.0),
            Vec3::new(2.0, 0.0, 2.0),
            Vec3::new(2.0, 0.0, 0.0),
        ],
        vec![[0, 2, 1], [0, 3, 2]],
    ).unwrap();
    let collider = Collider::from(SharedShape::new(production_static_shape::StaticTerrainShape::new(mesh)));
    let matrix = Mat4::from_translation(Vec3::new(22.0, -4.0, 12.0));
    let mut snapshot = super::tests::snapshot();
    snapshot.terrain_meshes = Arc::from([TerrainMeshQuery::new(collider, matrix).unwrap()]);
    let origin = matrix.transform_point3(Vec3::new(0.5, 1.0, 0.5));
    assert_eq!(snapshot.sample_support_below(origin, 3.0), ClipmapGroundHit::Miss);
    // 表面拾取继续命中背面；只有承托授权拒绝它，不能用删掉几何隐藏错误。
    assert!(snapshot.three_dimensional_hit(origin, Vec3::NEG_Y, 3.0).is_some());
}
