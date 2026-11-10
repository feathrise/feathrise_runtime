//! 静态LOD真实减面回归：约束几何、纹理与自定义属性对应关系，以及坏输入回退。
use super::*;
use bevy::{asset::RenderAssetUsages, mesh::{Indices, MeshVertexAttribute, PrimitiveTopology, VertexAttributeValues, VertexFormat}, prelude::*};

const CUSTOM_ID: MeshVertexAttribute = MeshVertexAttribute::new("author_vertex", 9001, VertexFormat::Uint32);

fn grid() -> Mesh {
    let mut positions = Vec::new();
    let mut uv = Vec::new();
    let mut indices = Vec::new();
    for z in 0..17 { for x in 0..17 {
        positions.push([x as f32, 0.0, z as f32]);
        uv.push([x as f32 / 16.0, z as f32 / 16.0]);
    } }
    for z in 0..16 { for x in 0..16 {
        let i = z * 17 + x;
        indices.extend_from_slice(&[i, i + 17, i + 1, i + 1, i + 17, i + 18]);
    } }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 289])
        .with_inserted_attribute(Mesh::ATTRIBUTE_TANGENT, vec![[1.0, 0.0, 0.0, 1.0]; 289])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv)
        .with_inserted_attribute(CUSTOM_ID, (0..289u32).collect::<Vec<_>>())
        .with_inserted_indices(Indices::U32(indices))
}

#[test]
fn static_lods_reduce_triangles_and_vertices_without_losing_attributes_or_winding() {
    let lods = build_static_mesh_lods(grid()).unwrap();
    assert!(!lods.is_empty());
    let mut triangles = 512;
    for lod in lods {
        let mesh = lod.mesh;
        assert_eq!(mesh.asset_usage, RenderAssetUsages::default());
        assert!(lod.max_error.is_finite() && lod.max_error >= 0.0);
        let VertexAttributeValues::Float32x3(positions) = mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() else { panic!() };
        let VertexAttributeValues::Float32x2(uv) = mesh.attribute(Mesh::ATTRIBUTE_UV_0).unwrap() else { panic!() };
        let VertexAttributeValues::Uint32(ids) = mesh.attribute(CUSTOM_ID).unwrap() else { panic!() };
        assert!(positions.len() < 289);
        for (i, position) in positions.iter().enumerate() {
            assert_eq!(ids[i], position[2] as u32 * 17 + position[0] as u32);
            assert_eq!(uv[i], [position[0] / 16.0, position[2] / 16.0]);
        }
        assert_eq!(mesh.attribute(Mesh::ATTRIBUTE_TANGENT).unwrap().len(), positions.len());
        // 原始所有边界点都保留，分块/材质primitive独立降级也不能把公共边界撕开。
        for z in 0..17 { for x in 0..17 {
            if x == 0 || z == 0 || x == 16 || z == 16 {
                assert!(positions.contains(&[x as f32, 0.0, z as f32]));
            }
        } }
        let index: Vec<usize> = mesh.indices().unwrap().iter().collect();
        assert!(index.len() / 3 < triangles);
        triangles = index.len() / 3;
        for face in index.chunks_exact(3) {
            let a = Vec3::from(positions[face[0]]);
            let b = Vec3::from(positions[face[1]]);
            let c = Vec3::from(positions[face[2]]);
            assert!((b - a).cross(c - a).y > 0.0);
        }
    }
}

#[test]
fn skin_morph_and_nontriangle_geometry_keep_their_original_backend() {
    let mut skin = grid();
    skin.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, vec![[1.0, 0.0, 0.0, 0.0]; 289]);
    assert!(build_static_mesh_lods(skin).is_err());
    let mut morph = grid();
    morph.set_morph_targets(Vec::new());
    assert!(build_static_mesh_lods(morph).is_err());
    assert!(build_static_mesh_lods(Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::default())).is_err());
}

#[test]
fn invalid_indices_attributes_and_nonfinite_positions_fail_before_meshopt() {
    let mut mesh = grid();
    mesh.insert_indices(Indices::U32(vec![0, 1, 999]));
    assert!(build_static_mesh_lods(mesh).is_err());
    let mut mesh = grid();
    mesh.insert_attribute(CUSTOM_ID, vec![1u32]);
    assert!(build_static_mesh_lods(mesh).is_err());
    let mut mesh = grid();
    let VertexAttributeValues::Float32x3(positions) = mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION).unwrap() else { panic!() };
    positions[0][1] = f32::NAN;
    assert!(build_static_mesh_lods(mesh).is_err());
}

#[test]
fn bounded_attribute_capture_preserves_exact_format_and_rejects_invalid_range_atomically() {
    let source = VertexAttributeValues::Sint16x4(vec![[1, 2, 3, 4], [-9, 8, 7, 6]]);
    let mut target = empty_static_mesh_attribute(&source);
    copy_static_mesh_attribute_range(&source, &mut target, 0..1).unwrap();
    let before = target.clone();
    assert!(copy_static_mesh_attribute_range(&source, &mut target, 1..3).is_err());
    assert_eq!(target, before);
    copy_static_mesh_attribute_range(&source, &mut target, 1..2).unwrap();
    assert_eq!(target, source);
    assert!(copy_static_mesh_attribute_range(&source, &mut VertexAttributeValues::Uint32(Vec::new()), 0..1).is_err());
}

#[test]
fn shared_fragment_capture_preserves_all_attributes_indices_and_actual_lod_results() {
    let source = grid();
    let mut capture = StaticMeshCapture::new(&source).unwrap();
    let mut frames = 0;
    while !capture.step(&source, 256).unwrap() { frames += 1; assert!(frames < 1000); }
    assert!(frames > 1);
    let captured = capture.finish().unwrap();
    for (attribute, values) in source.attributes() {
        assert_eq!(captured.attribute(*attribute), Some(values));
    }
    assert_eq!(captured.indices(), source.indices());
    let direct = build_static_mesh_lods(source).unwrap();
    let shared = build_static_mesh_lods(captured).unwrap();
    assert_eq!(direct.len(), shared.len());
    for (direct, shared) in direct.into_iter().zip(shared) {
        assert_eq!(direct.max_error, shared.max_error);
        assert_eq!(direct.mesh.indices(), shared.mesh.indices());
    }
}
