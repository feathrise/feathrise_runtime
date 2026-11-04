//! 密集平面上的真实Tnua感测扫掠；逐组阶段输出用于外部超时定位，不把胶囊替代零高度圆柱。
use super::StaticTerrainShape;
use avian3d::{
    parry::{
        math::Pose,
        query::{ShapeCastHit, ShapeCastOptions, cast_shapes},
        shape::{Shape, TriMesh, TriMeshFlags},
    },
    prelude::{Collider, ShapeCastConfig},
};
use bevy::prelude::*;
// 独立扫掠夹具保留既有几何尺寸，不读取任何应用角色配置。
const SENSOR_RADIUS: f32 = 0.5;
const CAPSULE_LENGTH: f32 = 1.0;
const FLOAT_HEIGHT: f32 = 1.01;
const CLING_DISTANCE: f32 = 0.5;
use std::time::Instant;

fn dense_flat_plane() -> TriMesh {
    // 16米块、128×128单元与0.125米源格距一致，原点是四格共用顶点。
    // 只构造这一块32768个三角形，构建时间和查询时间分别记录，不读取任何作者文件。
    const CELLS: usize = 128;
    const SPACING: f32 = 0.125;
    let started = Instant::now();
    eprintln!("dense_shape_cast BEGIN build_raw cells={CELLS} spacing={SPACING}");
    let side = CELLS + 1;
    let mut vertices = Vec::with_capacity(side * side);
    for z in 0..=CELLS {
        for x in 0..=CELLS {
            vertices.push(Vec3::new(
                x as f32 * SPACING - 8.0,
                0.0,
                z as f32 * SPACING - 8.0,
            ));
        }
    }
    let mut triangles = Vec::with_capacity(CELLS * CELLS * 2);
    for z in 0..CELLS {
        for x in 0..CELLS {
            let a = (z * side + x) as u32;
            let b = a + side as u32;
            triangles.push([a, b, b + 1]);
            triangles.push([a, b + 1, a + 1]); // 与正式水平地面相同的朝上绕序。
        }
    }
    let mesh = TriMesh::with_flags(vertices, triangles, TriMeshFlags::FIX_INTERNAL_EDGES)
        .expect("有限平面与索引应满足正式Trimesh构造约束");
    eprintln!(
        "dense_shape_cast END build_raw elapsed_us={} vertices={} triangles={}",
        started.elapsed().as_micros(),
        mesh.vertices().len(),
        mesh.indices().len()
    );
    mesh
}

fn cast_case(
    terrain: &dyn Shape,
    terrain_name: &str,
    sensor: &Collider,
    sensor_name: &str,
    sample_name: &str,
    origin: Vec3,
    config: &ShapeCastConfig,
) -> Option<ShapeCastHit> {
    // 与bevy-tnua-avian3d的shape_hits_callback→Parry参数完全同义：
    // 地形先、感测形状后，向下单位速度，保留接触几何，忽略向外的初始重叠。
    let options = ShapeCastOptions {
        max_time_of_impact: config.max_distance,
        target_distance: config.target_distance,
        stop_at_penetration: !config.ignore_origin_penetration,
        compute_impact_geometry_on_penetration: config.compute_contact_on_penetration,
    };
    eprintln!(
        "dense_shape_cast BEGIN query terrain={terrain_name} sensor={sensor_name} sample={sample_name} origin={origin:?} config={config:?}"
    );
    let started = Instant::now();
    let result = cast_shapes(
        &Pose::IDENTITY,
        Vec3::ZERO,
        terrain,
        &Pose::translation(origin.x, origin.y, origin.z),
        Vec3::NEG_Y,
        sensor.shape_scaled().as_ref(),
        options,
    );
    eprintln!(
        "dense_shape_cast END query terrain={terrain_name} sensor={sensor_name} sample={sample_name} origin={origin:?} elapsed_us={} result={result:?}",
        started.elapsed().as_micros()
    );
    let hit = result.expect("正式包装必须继续支持与raw Trimesh相同的扫掠类型");
    if let Some(hit) = hit {
        assert!(hit.time_of_impact.is_finite());
        assert!((0.0..=config.max_distance + 0.0001).contains(&hit.time_of_impact));
        assert!(hit.witness1.is_finite() && hit.witness2.is_finite());
        assert!(hit.normal1.is_finite() && hit.normal2.is_finite());
    }
    hit
}

#[test]
fn dense_plane_tnua_sensor_shape_cast_finishes_at_grid_corners_edges_and_penetrations() {
    let raw = dense_flat_plane();
    eprintln!("dense_shape_cast BEGIN build_wrapper");
    let started = Instant::now();
    let wrapped = StaticTerrainShape::new(raw.clone());
    eprintln!(
        "dense_shape_cast END build_wrapper elapsed_us={}",
        started.elapsed().as_micros()
    );
    let config = ShapeCastConfig {
        // TnuaBuiltinWalk的实际地面范围为float_height+cling_distance，即1.51米。
        max_distance: FLOAT_HEIGHT + CLING_DISTANCE,
        ignore_origin_penetration: true,
        ..default()
    };
    let sensors = [
        (
            "tnua_zero_height_cylinder",
            Collider::cylinder(SENSOR_RADIUS - 0.01, 0.0),
        ),
        (
            "physical_capsule_control",
            Collider::capsule(SENSOR_RADIUS, CAPSULE_LENGTH),
        ),
    ];
    // 顶点、水平格边、单元内共用对角边与三角内部都保持真实密网格关系。
    let samples = [
        ("shared_corner", Vec2::ZERO),
        ("shared_grid_edge", Vec2::new(0.0625, 0.0)),
        ("shared_diagonal_edge", Vec2::splat(0.0625)),
        ("triangle_interior", Vec2::new(0.03125, 0.0625)),
    ];
    for (sensor_name, sensor) in sensors {
        for (sample_name, xz) in samples {
            for y in [1.0, 0.0, 0.01] {
                let origin = Vec3::new(xz.x, y, xz.y);
                let raw_hit = cast_case(
                    &raw,
                    "raw_trimesh",
                    &sensor,
                    sensor_name,
                    sample_name,
                    origin,
                    &config,
                );
                let wrapped_hit = cast_case(
                    &wrapped,
                    "static_terrain_shape",
                    &sensor,
                    sensor_name,
                    sample_name,
                    origin,
                    &config,
                );
                assert_eq!(
                    raw_hit.is_some(),
                    wrapped_hit.is_some(),
                    "同一几何包装不应改变命中资格：sensor={sensor_name} sample={sample_name} y={y}"
                );
                if let (Some(raw_hit), Some(wrapped_hit)) = (raw_hit, wrapped_hit) {
                    assert!(
                        (raw_hit.time_of_impact - wrapped_hit.time_of_impact).abs() < 0.0001,
                        "同一几何包装不应改变命中距离：sensor={sensor_name} sample={sample_name} y={y}"
                    );
                }
                if sensor_name == "tnua_zero_height_cylinder" && y == 1.0 {
                    assert!(
                        raw_hit.is_some_and(|hit| (hit.time_of_impact - 1.0).abs() < 0.001),
                        "离地一米的真实感测圆柱须命中平面"
                    );
                }
            }
        }
    }
    eprintln!("dense_shape_cast END all_cases");
}
