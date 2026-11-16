//! 大小两种网格来源的拓扑、空间校验和历史序列化回归。

use super::*;
use super::triangulation::triangulate_face;

#[test]
fn plane_is_one_open_quad_with_upward_triangles_and_explicit_codec_identity() {
    let plane = EditableMeshGeometry::from_plane_size_mm([3000, 5000]).unwrap();
    assert!(plane.is_open());
    assert_eq!((plane.vertex_count(), plane.edges().len(), plane.face_count(), plane.triangles().len()), (4, 4, 1, 2));
    assert_eq!(plane.bounds_half_mm(), ([-3000, 0, -5000], [3000, 0, 5000]));
    for edge in 0..4 { assert_eq!(plane.adjacent_faces(edge).collect::<Vec<_>>(), [0]); }
    for triangle in plane.triangles() {
        let [a, b, c] = triangle.map(|i| DVec3::from_array(plane.vertices()[i].map(f64::from)));
        assert!((b - a).cross(c - a).y > 0.0);
    }
    let mut json = serde_json::to_value(&plane).unwrap();
    assert_eq!(json["open_surface"], true);
    assert_eq!(json["vertices_half_mm"].as_array().unwrap().len(), 4);
    assert_eq!(serde_json::from_value::<EditableMeshGeometry>(json.clone()).unwrap(), plane);
    json.as_object_mut().unwrap().remove("open_surface");
    assert!(serde_json::from_value::<EditableMeshGeometry>(json).is_err(), "缺省仍按闭壳校验，不能放行旧坏存档");
    assert!(EditableMeshGeometry::from_parts(plane.vertices(), plane.faces()).is_err());
    for size in [[0, 1000], [1000, 10_000_001]] { assert!(EditableMeshGeometry::from_plane_size_mm(size).is_err()); }
    EditableMeshGeometry::from_plane_size_mm([1, 1]).unwrap();
}

#[test]
fn plane_topology_and_deformation_keep_an_open_disk_and_reject_invalid_candidates() {
    let plane = EditableMeshGeometry::from_plane_size_mm([2000, 2000]).unwrap();
    let points = (-1..=1).flat_map(|z| (-1..=1).map(move |x| [x * 2000, 0, z * 2000])).collect::<Vec<_>>();
    let faces = [0, 1, 3, 4].map(|v| MeshFace::new(&[v, v + 3, v + 4, v + 1]).unwrap());
    let grid = plane.with_parts(points, faces.to_vec()).unwrap();
    assert_eq!((grid.vertex_count(), grid.edges().len(), grid.face_count()), (9, 12, 4));
    assert!(grid.is_open());
    let mut points = grid.vertices().to_vec();
    points[4][1] = 1000;
    let bent = grid.with_vertices(points).unwrap();
    assert!(bent.is_open());
    assert_eq!(serde_json::from_slice::<EditableMeshGeometry>(&serde_json::to_vec(&bent).unwrap()).unwrap(), bent);
    let mut points = grid.vertices().to_vec();
    points[4] = points[0];
    assert!(grid.with_vertices(points).is_err());
    let mut disconnected = plane.vertices().to_vec();
    disconnected.extend(plane.vertices().iter().map(|p| [p[0] + 10_000, p[1], p[2]]));
    assert!(plane.with_parts(disconnected, vec![plane.faces()[0], MeshFace::new(&[4, 5, 6, 7]).unwrap()]).is_err());
    let mut duplicate = grid.faces().to_vec();
    duplicate.push(duplicate[0]);
    assert!(grid.with_parts(grid.vertices().to_vec(), duplicate).is_err());
    let cube = ScenePlacementAssetTarget::DEFAULT_CUBOID.editable_geometry().unwrap();
    assert!(cube.with_parts(cube.vertices().to_vec(), cube.faces()[..5].to_vec()).is_err());
}

fn prism(boundary: &[[i32; 2]]) -> EditableMeshGeometry {
    let count = boundary.len();
    let mut vertices = Vec::new();
    for y in [-2_000, 2_000] {
        vertices.extend(boundary.iter().map(|[x, z]| [*x, y, *z]));
    }
    let mut faces = vec![
        MeshFace::new(&(0..count as u32).rev().collect::<Vec<_>>()).unwrap(),
        MeshFace::new(&(count as u32..(count * 2) as u32).collect::<Vec<_>>()).unwrap(),
    ];
    for side in 0..count {
        let next = (side + 1) % count;
        faces.push(
            MeshFace::new(&[
                side as u32,
                next as u32,
                (next + count) as u32,
                (side + count) as u32,
            ])
            .unwrap(),
        );
    }
    EditableMeshGeometry::from_parts(&vertices, &faces).unwrap()
}

#[test]
fn mesh_polygon_ear_clipping_preserves_concavity_and_collinear_author_edges() {
    let concave = prism(&[
        [-2_000, -2_000],
        [-2_000, 2_000],
        [2_000, 2_000],
        [2_000, 0],
        [0, 0],
        [0, -2_000],
    ]);
    assert_eq!(concave.triangles().len(), 20);
    assert_eq!(concave.edges().len(), 18);
    let collinear = prism(&[
        [-2_000, -2_000],
        [-2_000, 0],
        [-2_000, 2_000],
        [2_000, 2_000],
        [2_000, -2_000],
    ]);
    assert_eq!(collinear.triangles().len(), 16);
    assert_eq!(collinear.edges().len(), 15);
    let from_pairs = concave
        .triangles_with_faces()
        .into_iter()
        .map(|&(_, triangle)| triangle)
        .collect::<Vec<_>>();
    assert_eq!(concave.triangles(), from_pairs);
    let old = ScenePlacementAssetTarget::DEFAULT_CUBOID
        .editable_geometry()
        .unwrap();
    assert_eq!(old.edges(), EDITABLE_CUBOID_EDGES);
    assert_eq!(old.triangles(), crate::EDITABLE_CUBOID_TRIANGLES);
}

#[test]
fn mesh_roundtrip_preserves_legacy_padding_and_rejects_truncation_or_hidden_data() {
    let geometry = ScenePlacementAssetTarget::DEFAULT_CUBOID
        .editable_geometry()
        .unwrap();
    let target = ScenePlacementAssetTarget::EditableMesh {
        geometry: geometry.clone(),
        color_srgb: [8, 45, 129],
        roughness_milli: 321,
        collision: false,
    };
    target.validate().unwrap();
    let mut json = serde_json::to_value(&target).unwrap();
    assert_eq!(json["geometry"]["vertices_half_mm"].as_array().unwrap().len(), 64);
    assert_eq!(json["geometry"]["faces"].as_array().unwrap().len(), 64);
    assert_eq!(
        serde_json::from_value::<ScenePlacementAssetTarget>(json.clone()).unwrap(),
        target
    );
    json["geometry"]["vertices_half_mm"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(serde_json::from_value::<ScenePlacementAssetTarget>(json).is_err());
    let mut dirty = serde_json::to_value(&target).unwrap();
    dirty["geometry"]["faces"][6]["vertices"][5] = serde_json::json!(1);
    assert!(serde_json::from_value::<ScenePlacementAssetTarget>(dirty).unwrap_err().to_string().contains("清零"));
    let mut duplicate = geometry.faces().to_vec();
    duplicate[1] = duplicate[0];
    assert!(EditableMeshGeometry::from_parts(geometry.vertices(), &duplicate).unwrap_err().contains("重复面"));
    let mut broken = geometry.faces().to_vec();
    broken[0].vertices[0] = 63;
    assert!(EditableMeshGeometry::from_parts(geometry.vertices(), &broken).unwrap_err().contains("索引"));
    assert!(MeshFace::new(&[0, 1, 2, 0]).is_err());
    assert!(MeshFace::new(&(0..17).collect::<Vec<_>>()).is_err());
    assert!(
        EditableMeshGeometry::check_budget(MAX_EDITABLE_MESH_VERTICES, MAX_EDITABLE_MESH_FACES, false)
            .unwrap_err()
            .contains("96 MiB")
    );
}

#[test]
fn mesh_polygon_rejects_crossed_and_nonplanar_boundaries_without_partial_triangles() {
    let points = [[0, 0, 0], [4, 0, 0], [4, 0, 4], [2, 0, 2], [0, 0, 4]]
        .map(|point| DVec3::from_array(point.map(f64::from)));
    assert!(triangulate_face(&MeshFace::new(&[0, 2, 1, 4, 3]).unwrap(), &points).is_err());
    let mut bent = points;
    bent[3].y = 10.0;
    assert!(
        triangulate_face(&MeshFace::new(&[0, 1, 2, 3, 4]).unwrap(), &bent)
            .unwrap_err()
            .contains("共面")
    );
    // 历史四边面的折面仍使用0→2对角线，不能受新多边面平面规则影响。
    assert_eq!(
        triangulate_face(&MeshFace::new(&[0, 1, 3, 4]).unwrap(), &bent).unwrap(),
        vec![[0, 1, 3], [0, 3, 4]]
    );
}

#[test]
fn mesh_rejects_two_closed_shells_glued_at_two_vertices_even_when_euler_is_two() {
    let vertices = [
        [0, 0, 2_000],
        [0, 0, -2_000],
        [2_000, -1_000, 0],
        [4_000, 0, 0],
        [2_000, 1_000, 0],
        [1_000, 0, 0],
        [-2_000, -1_000, 0],
        [-1_000, 0, 0],
        [-2_000, 1_000, 0],
        [-4_000, 0, 0],
    ];
    let mut faces = Vec::new();
    for base in [2u32, 6u32] {
        for side in 0..4 {
            let a = base + side;
            let b = base + (side + 1) % 4;
            faces.push(MeshFace::new(&[0, a, b]).unwrap());
            faces.push(MeshFace::new(&[1, b, a]).unwrap());
        }
    }
    let error = EditableMeshGeometry::from_parts(&vertices, &faces).unwrap_err();
    assert!(
        error.contains("一环面"),
        "应明确识别共享顶点处的非流形，而不是接受Euler碰巧为2的双壳：{error}"
    );
}

#[test]
fn mesh_legacy_64_point_topology_has_124_triangles_and_126_edges() {
    let mut vertices = Vec::new();
    for y in [-3_000, -1_000, 1_000, 3_000] {
        for side in 0..16 {
            let angle = side as f64 * std::f64::consts::TAU / 16.0;
            vertices.push([
                (angle.cos() * 4_000.0).round() as i32,
                y,
                (-angle.sin() * 4_000.0).round() as i32,
            ]);
        }
    }
    let mut faces = vec![
        MeshFace::new(&(0..16).rev().collect::<Vec<_>>()).unwrap(),
        MeshFace::new(&(48..64).collect::<Vec<_>>()).unwrap(),
    ];
    for ring in 0..3 {
        for side in 0..16 {
            let next = (side + 1) % 16;
            let [a, b, c, d] = [
                (ring * 16 + side) as u32,
                (ring * 16 + next) as u32,
                ((ring + 1) * 16 + next) as u32,
                ((ring + 1) * 16 + side) as u32,
            ];
            if ring == 0 && side < 14 {
                faces.push(MeshFace::new(&[a, b, c]).unwrap());
                faces.push(MeshFace::new(&[a, c, d]).unwrap());
            } else {
                faces.push(MeshFace::new(&[a, b, c, d]).unwrap());
            }
        }
    }
    let geometry = EditableMeshGeometry::from_parts(&vertices, &faces).unwrap();
    assert_eq!((geometry.vertex_count(), geometry.face_count()), (64, 64));
    assert_eq!(geometry.triangles().len(), 124);
    assert_eq!(geometry.edges().len(), 126);
    assert!(geometry.same_storage(&geometry.clone()));
    assert_eq!(geometry.edge_faces().len(), geometry.edges().len());
}

#[test]
fn mesh_coordinate_edits_keep_legacy_storage_and_existing_mesh_identity() {
    let source = ScenePlacementAssetTarget::DEFAULT_CUBOID;
    let geometry = source.editable_geometry().unwrap();
    let mut points = geometry.vertices().to_vec();
    points[7][1] += 100;
    let geometry = geometry.with_vertices(points).unwrap();
    let editable = source.with_editable_geometry(geometry.clone()).unwrap();
    assert!(matches!(
        editable,
        ScenePlacementAssetTarget::EditableCuboid { .. }
    ));
    let target = ScenePlacementAssetTarget::EditableMesh {
        geometry: geometry.clone(),
        color_srgb: [1, 2, 3],
        roughness_milli: 221,
        collision: false,
    };
    assert_eq!(target.with_editable_geometry(geometry).unwrap(), target);
}

/// 按原八点面绕序生成闭合立方体格，边上的格点按位置共享。
fn gridded_cube(segments: u32) -> EditableMeshGeometry {
    use std::collections::HashMap;
    let mut indices = HashMap::new();
    let mut vertices = Vec::new();
    let mut faces = Vec::new();
    let corners: [[i32; 3]; 8] = std::array::from_fn(|corner| {
        std::array::from_fn(|axis| if corner & (1 << axis) == 0 { -10_100 } else { 10_100 })
    });
    for [a, b, _, d] in EDITABLE_CUBOID_FACES {
        let mut grid = Vec::new();
        let side = segments as usize + 1;
        for y in 0..=segments as i32 {
            for x in 0..=segments as i32 {
                let point = std::array::from_fn(|axis| {
                    corners[a][axis] + (corners[b][axis] - corners[a][axis]) * x / segments as i32
                        + (corners[d][axis] - corners[a][axis]) * y / segments as i32
                });
                grid.push(*indices.entry(point).or_insert_with(|| {
                    let index = vertices.len() as u32;
                    vertices.push(point);
                    index
                }));
            }
        }
        for y in 0..segments as usize {
            for x in 0..segments as usize {
                let i = y * side + x;
                faces.push(MeshFace::new(&[grid[i], grid[i + 1], grid[i + 1 + side], grid[i + side]]).unwrap());
            }
        }
    }
    EditableMeshGeometry::from_owned_parts(vertices, faces).unwrap()
}

#[test]
fn dense_mesh_100_cuts_builds_one_shared_source_and_uses_a_local_triangle_index() {
    let geometry = gridded_cube(101);
    assert_eq!((geometry.vertex_count(), geometry.face_count()), (61_208, 61_206));
    assert_eq!(geometry.edges().len(), 122_412);
    assert_eq!(geometry.triangles().len(), 122_412);
    assert_eq!(geometry.bounds_half_mm(), ([-10_100; 3], [10_100; 3]));
    assert!(geometry.same_storage(&geometry.clone()));
    assert!(geometry.allocation_bytes() <= editable_mesh_required_bytes(61_208, 61_206).unwrap());
    assert!(geometry.triangles_with_faces().windows(2).all(|pair| pair[0].0 <= pair[1].0));
    let origin = DVec3::new(0.1, 20_100.0, 0.1);
    let direction = DVec3::NEG_Y * 2_000.0;
    assert!(geometry.nearest_triangle(origin, direction, 4.99).is_none());
    let (hit, t) = geometry.nearest_triangle(origin, direction, 5.01).unwrap();
    assert_eq!(t, 5.0);
    let (face, triangle) = geometry.triangles_with_faces()[hit];
    assert!(geometry.face_triangles(face).unwrap().contains(&triangle));
    assert!(triangle.iter().all(|&vertex| geometry.vertices()[vertex][1] == 10_100));
}

#[test]
fn compact_geometry_roundtrips_high_indices_and_rejects_inconsistent_counts() {
    let source = gridded_cube(10);
    assert_eq!((source.vertex_count(), source.face_count()), (602, 600));
    let mut value = serde_json::to_value(&source).unwrap();
    assert_eq!(value["vertices_half_mm"].as_array().unwrap().len(), source.vertex_count());
    assert_eq!(value["faces"].as_array().unwrap().len(), source.face_count());
    assert!(source.faces().iter().flat_map(MeshFace::indices).any(|&vertex| vertex > 255));
    assert_eq!(serde_json::from_value::<EditableMeshGeometry>(value.clone()).unwrap(), source);
    value["face_count"] = serde_json::json!(599);
    assert!(serde_json::from_value::<EditableMeshGeometry>(value).is_err());
}
