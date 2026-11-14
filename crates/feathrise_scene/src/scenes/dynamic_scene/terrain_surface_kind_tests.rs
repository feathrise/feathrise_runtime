//! 作者载体分流验证同次真实最近面；高度面不能因公共法线查询或射线后方洞面而转入三维字段。

use super::*;
use super::super::{clipmap::test_ground_snapshot, terrain_topology::TerrainMeshQuery};
use std::sync::Arc;

fn plane(height: f32) -> Collider {
    Collider::trimesh(vec![Vec3::new(20., height, 10.), Vec3::new(24., height, 10.),
        Vec3::new(24., height, 14.), Vec3::new(20., height, 14.)], vec![[0, 2, 1], [0, 3, 2]])
}

fn ray_hit(query: &TerrainObjectRaySnapshot, origin: Vec3) -> Vec3 {
    let point = query.ray(origin, Vec3::NEG_Y).expect("真实最近表面必须可命中");
    let (surface, normal) = query.ray_surface(origin, Vec3::NEG_Y, 1_000_000.).expect("公共表面必须包含同次真实点与法线");
    assert!(surface.distance(point) < 0.0001);
    assert!(normal.y > 0.0);
    point
}

#[test]
fn ordinary_height_surface_uses_two_dimensional_authoring() {
    let frame = SceneSourceFrame::from_transform(Transform::from_xyz(48., 3., -48.)
        .with_scale(Vec3::new(2., 3., 0.5)));
    let query = TerrainObjectRaySnapshot::Clipmap(test_ground_snapshot(), frame);
    let origin = frame.to_world(Vec3::new(22., 50., 12.));
    let point = ray_hit(&query, origin);
    assert!(!query.ray_hit_is_three_dimensional(origin, Vec3::NEG_Y, point),
        "普通高度面即使由公共三角查询提供法线，也必须留在二维作者页");
}

#[test]
fn a_farther_cave_cannot_change_the_nearest_height_surface_carrier() {
    let origin = Vec3::new(22., 50., 12.);
    for separation in [0.01, 0.0005] {
        let mut ground = test_ground_snapshot();
        let height = ground.ray_surface(origin, Vec3::NEG_Y, 100.).unwrap().0.y;
        ground.terrain_meshes = Arc::from([TerrainMeshQuery::new(plane(height - separation), Mat4::IDENTITY).unwrap()]);
        let query = TerrainObjectRaySnapshot::Clipmap(ground, SceneSourceFrame::from_transform(Transform::IDENTITY));
        let point = ray_hit(&query, origin);
        assert!((point.y - height).abs() < 0.0001);
        assert!(!query.ray_hit_is_three_dimensional(origin, Vec3::NEG_Y, point),
            "后方{separation}米洞面不能把当前高度面判作三维字段");
    }
}

#[test]
fn an_actual_closer_cave_keeps_three_dimensional_authoring() {
    let mut ground = test_ground_snapshot();
    ground.terrain_meshes = Arc::from([TerrainMeshQuery::new(plane(40.), Mat4::IDENTITY).unwrap()]);
    let query = TerrainObjectRaySnapshot::Clipmap(ground, SceneSourceFrame::from_transform(Transform::IDENTITY));
    let origin = Vec3::new(22., 50., 12.);
    let point = ray_hit(&query, origin);
    assert_eq!(point, Vec3::new(22., 40., 12.));
    assert!(query.ray_hit_is_three_dimensional(origin, Vec3::NEG_Y, point));
}

#[test]
fn a_native_mesh_source_keeps_three_dimensional_authoring() {
    let frame = SceneSourceFrame::from_transform(Transform::from_xyz(48., 3., -48.)
        .with_scale(Vec3::new(2., 3., 0.5)));
    let query = TerrainObjectRaySnapshot::Mesh(plane(0.), frame);
    let origin = frame.to_world(Vec3::new(22., 50., 12.));
    let point = ray_hit(&query, origin);
    assert!(query.ray_hit_is_three_dimensional(origin, Vec3::NEG_Y, point));
}
