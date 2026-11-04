//! 水面几何按固定上传单元拆分，保留原法线、UV及共享互动足迹；块数不限制作者水体规模。
use super::*;
use std::collections::HashMap;

const TRIANGLES_PER_UPLOAD: usize = 16_384;

/// 必须在构建水面的同一个后台任务中调用，主帧只移动完成的单块。
pub fn chunk_water_surfaces(
    input: Vec<BuiltWaterSurface>,
) -> Result<Vec<BuiltWaterSurface>, String> {
    let mut output = Vec::new();
    for surface in input {
        let count = surface.mesh.positions.len();
        if surface.mesh.normals.len() != count
            || surface.mesh.uvs.len() != count
            || surface.mesh.colors.len() != count
            || surface
                .mesh
                .tangents
                .as_ref()
                .is_some_and(|v| v.len() != count)
            || surface
                .mesh
                .uvs_b
                .as_ref()
                .is_some_and(|v| v.len() != count)
            || !surface.mesh.indices.len().is_multiple_of(3)
        {
            return Err("水面分块属性或三角索引长度不一致".into());
        }
        if surface.mesh.triangle_count() <= TRIANGLES_PER_UPLOAD {
            output.push(surface);
            continue;
        }
        let data = surface.mesh;
        for triangles in data.indices.chunks(TRIANGLES_PER_UPLOAD * 3) {
            let mut remap = HashMap::new();
            let mut originals = Vec::new();
            let mut indices = Vec::with_capacity(triangles.len());
            for &original in triangles {
                if original as usize >= data.positions.len() {
                    return Err("水面分块索引越界".into());
                }
                let next = remap.len() as u32; // 单块最多49152个角点，索引天然可表示。
                indices.push(*remap.entry(original).or_insert_with(|| {
                    originals.push(original as usize);
                    next
                }));
            }
            let positions: Vec<_> = originals.iter().map(|&i| data.positions[i]).collect();
            let mesh = PaintedWaterMeshData {
                local_aabb: local_aabb_from_positions(&positions),
                positions,
                normals: originals.iter().map(|&i| data.normals[i]).collect(),
                tangents: data
                    .tangents
                    .as_ref()
                    .map(|v| originals.iter().map(|&i| v[i]).collect()),
                uvs: originals.iter().map(|&i| data.uvs[i]).collect(),
                uvs_b: data
                    .uvs_b
                    .as_ref()
                    .map(|v| originals.iter().map(|&i| v[i]).collect()),
                colors: originals.iter().map(|&i| data.colors[i]).collect(),
                indices,
                footprint: data.footprint.clone(),
                ribbon_footprint: data.ribbon_footprint.clone(),
            };
            // 切块只改变登记粒度，法线/累计米/水深不能逐块重新计算。
            output.push(BuiltWaterSurface {
                descriptor: surface.descriptor.clone(),
                mesh,
            });
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn water_upload_chunks_reject_incomplete_attributes() {
        let mut surface = super::super::super::ribbon::build_river_water_surface(
            &feathrise_map_content::MapWaterRiver {
                id: "invalid".into(),
                label: "损坏网格".into(),
                points: vec![
                    feathrise_map_content::MapWaterSplinePoint {
                        id: "a".into(),
                        position: [0., 5., 0.],
                        width: 4.,
                        depth: 2.,
                        flow_speed: 1.,
                        outgoing: default(),
                    },
                    feathrise_map_content::MapWaterSplinePoint {
                        id: "b".into(),
                        position: [8., 5., 0.],
                        width: 4.,
                        depth: 2.,
                        flow_speed: 1.,
                        outgoing: default(),
                    },
                ],
                source_layer_id: None,
                target_layer_id: None,
                base_color_srgba: [0.2, 0.3, 0.4, 0.7],
                perceptual_roughness: 0.06,
            },
        )
        .unwrap();
        surface.mesh.normals.pop();
        assert!(chunk_water_surfaces(vec![surface]).is_err());
    }
    #[test]
    fn long_river_chunks_preserve_every_face_normals_and_shared_interaction() {
        let river = feathrise_map_content::MapWaterRiver {
            id: "long".into(),
            label: "长河".into(),
            points: vec![
                feathrise_map_content::MapWaterSplinePoint {
                    id: "a".into(),
                    position: [0., 5., 0.],
                    width: 40.,
                    depth: 2.,
                    flow_speed: 1.,
                    outgoing: default(),
                },
                feathrise_map_content::MapWaterSplinePoint {
                    id: "b".into(),
                    position: [16_000., 5., 0.],
                    width: 40.,
                    depth: 2.,
                    flow_speed: 1.,
                    outgoing: default(),
                },
            ],
            source_layer_id: None,
            target_layer_id: None,
            base_color_srgba: [0.2, 0.3, 0.4, 0.7],
            perceptual_roughness: 0.06,
        };
        let source = super::super::super::ribbon::build_river_water_surface(&river).unwrap();
        let faces = source.mesh.triangle_count();
        assert!(faces > 131_072);
        let originals: std::collections::BTreeMap<_, _> = source
            .mesh
            .positions
            .iter()
            .zip(&source.mesh.normals)
            .map(|(p, n)| (p.map(f32::to_bits), *n))
            .collect();
        let pieces = chunk_water_surfaces(vec![source]).unwrap();
        assert_eq!(
            pieces
                .iter()
                .map(|p| p.mesh.triangle_count())
                .sum::<usize>(),
            faces
        );
        assert!(pieces.len() > 8);
        for piece in pieces {
            assert!(
                piece.mesh.estimated_gpu_bytes()
                    < super::super::super::gpu_upload::MAX_WATER_GPU_UPLOAD_BYTES_PER_FRAME
            );
            for (p, n) in piece.mesh.positions.iter().zip(&piece.mesh.normals) {
                assert_eq!(*n, originals[&p.map(f32::to_bits)]); // 比较原顶点法线，切块不能重算法线。
            }
            assert!(
                piece
                    .mesh
                    .ribbon_footprint
                    .as_ref()
                    .unwrap()
                    .sample(Vec2::new(15_000., 0.))
                    .is_some()
            );
        }
    }
}
