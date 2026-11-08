//! 完整体积面先统一派生法线，再按固定三角数分成上传单元；块数不限制地形规模。
use super::display::VolumeSurfaceFields;
use bevy::prelude::*;
use feathrise_map_terrain::{TerrainMeshInput, derive_vertex_normals};
use std::collections::HashMap;

const TRIANGLES_PER_CHUNK: usize = 16_384;

pub(in crate::maps::dynamic_map) fn build_meshes(
    input: TerrainMeshInput,
    fields: &VolumeSurfaceFields,
) -> Result<Vec<Mesh>, String> {
    // 分块前在完整焊接网格计算一次法线；切块边缘不能各自平均而产生接缝。
    let normals = derive_vertex_normals(&input)?;
    let mut meshes = Vec::new();
    for triangles in input.indices.chunks(TRIANGLES_PER_CHUNK * 3) {
        let mut remap = HashMap::new();
        let mut positions = Vec::new();
        let mut local_normals = Vec::new();
        let mut indices = Vec::with_capacity(triangles.len());
        for &original in triangles {
            let next = remap.len() as u32; // 单个上传单元至多49152个三角角点。
            let index = *remap.entry(original).or_insert_with(|| {
                positions.push(input.positions[original as usize]);
                local_normals.push(normals[original as usize]);
                next
            });
            indices.push(index);
        }
        meshes.push(feathrise_environment::terrain_surface::build_volume_surface_mesh(
            &positions,
            &indices,
            &local_normals,
            |_, _| None,
            |p| fields.0.exposed_depth(p),
        )?);
    }
    Ok(meshes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn volume_field_limit_uses_real_device_capacity_instead_of_a_fixed_cpu_budget() {
        let buffer = bevy::render::storage::ShaderBuffer::from(vec![UVec4::ZERO; 16]);
        let bytes = buffer.cast_slice::<u8>().unwrap().len() as u64;
        let fields = VolumeSurfaceFields(
            default(),
            std::sync::Arc::new(std::sync::Mutex::new(Some(buffer))),
        );
        assert!(fields.validate_storage_limit(Some(bytes)).is_ok());
        assert!(fields.validate_storage_limit(None).is_ok());
        assert!(
            fields
                .validate_storage_limit(Some(bytes - 1))
                .unwrap_err()
                .contains("设备单绑定上限")
        );
    }
    #[test]
    fn large_volume_is_split_without_dropping_faces_or_changing_seam_normals() {
        let width = 370usize;
        let positions = (0..=width)
            .flat_map(|z| (0..=width).map(move |x| [x as f32, (x % 17) as f32 * 0.1, z as f32]))
            .collect();
        let mut indices = Vec::new();
        for z in 0..width {
            for x in 0..width {
                let a = (z * (width + 1) + x) as u32;
                let b = a + (width + 1) as u32;
                indices.extend([a, b, a + 1, a + 1, b, b + 1]);
            }
        }
        assert!(indices.len() / 3 > 262_144);
        let total = indices.len();
        let input = TerrainMeshInput { positions, indices };
        let normal = derive_vertex_normals(&input).unwrap();
        let meshes = build_meshes(input, &default()).unwrap();
        assert!(meshes.len() > 16);
        assert_eq!(
            meshes
                .iter()
                .map(|m| m.indices().unwrap().len())
                .sum::<usize>(),
            total
        );
        for mesh in &meshes {
            let bevy::mesh::VertexAttributeValues::Float32x3(points) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap()
            else {
                panic!()
            };
            let bevy::mesh::VertexAttributeValues::Float32x3(actual) =
                mesh.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap()
            else {
                panic!()
            };
            for (p, n) in points.iter().zip(actual) {
                assert_eq!(*n, normal[p[2] as usize * (width + 1) + p[0] as usize]);
            }
            assert!(mesh.indices().unwrap().len() <= TRIANGLES_PER_CHUNK * 3);
        }
    }
}
