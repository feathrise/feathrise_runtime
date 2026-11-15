//! 单面挤出纯运算：冻结完整记录，世界法线定距，局部半毫米量化后原子返回完整候选。

use crate::{
    EditableMeshGeometry, MAX_EDITABLE_MESH_BYTES, MeshFace, ScenePlacementRecord,
    editable_mesh_required_bytes,
};
use bevy::{
    math::{DQuat, DVec3},
    prelude::*,
};

fn face_normal(
    before: &ScenePlacementRecord,
    geometry: &EditableMeshGeometry,
    face: usize,
) -> Result<Vec3, String> {
    let scale = Vec3::from_array(before.scale);
    let origin = Vec3::from_array(before.translation);
    if !scale.is_finite()
        || scale.min_element() <= 0.0
        || !origin.is_finite()
        || !before.yaw_degrees.is_finite()
    {
        return Err("实例变换无效，无法挤出".into());
    }
    let rotation = DQuat::from_rotation_y(f64::from(before.yaw_degrees).to_radians());
    let points = geometry.vertices();
    let scale = scale.as_dvec3() * 0.0005;
    geometry.face_triangles(face).ok_or("所选面已失效")?
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.map(|index| DVec3::from_array(points[index].map(f64::from)));
            // 法线只需当前面的有序三角，使用差向量可避免大世界原点的精度抵消。
            (rotation * ((b - a) * scale)).cross(rotation * ((c - a) * scale))
        })
        .sum::<DVec3>()
        .try_normalize()
        .map(|normal| normal.as_vec3())
        .ok_or_else(|| "所选面法线退化，无法挤出".into())
}

pub fn extrusion_normal(before: &ScenePlacementRecord, face: usize) -> Result<Vec3, String> {
    let geometry = before
        .asset_target
        .editable_geometry()
        .ok_or("当前对象不支持面挤出")?;
    before.asset_target.validate()?;
    face_normal(before, &geometry, face)
}

pub fn extruded_placement_record(
    before: &ScenePlacementRecord,
    face: usize,
    world_distance: f32,
) -> Result<ScenePlacementRecord, String> {
    if !world_distance.is_finite() {
        return Err("挤出距离不是有限数值".into());
    }
    let geometry = before
        .asset_target
        .editable_geometry()
        .ok_or("当前对象不支持面挤出")?;
    let old_face = *geometry.faces().get(face).ok_or("所选面已失效")?;
    before.asset_target.validate()?;
    if world_distance == 0.0 {
        return Ok(before.clone());
    }
    let vertices = geometry.vertex_count();
    let faces = geometry.face_count();
    let corners = old_face.indices().len();
    if editable_mesh_required_bytes(vertices + corners, faces + corners)
        .is_none_or(|bytes| bytes > MAX_EDITABLE_MESH_BYTES) {
        return Err("挤出会超过网格安全内存预算，请减少操作范围或拆分对象".into());
    }
    let normal = face_normal(before, &geometry, face)?;
    let local_delta = (DQuat::from_rotation_y(-f64::from(before.yaw_degrees).to_radians())
        * normal.as_dvec3()
        * f64::from(world_distance))
        / Vec3::from_array(before.scale).as_dvec3()
        * 2_000.0;
    let mut cap = Vec::with_capacity(corners);
    let mut points = Vec::with_capacity(vertices + corners);
    points.extend_from_slice(geometry.vertices());
    for (corner, vertex) in old_face.indices().iter().copied().enumerate() {
        let source =
            DVec3::from_array(points[vertex as usize].map(f64::from));
        let result = (source + local_delta).round();
        if !result.is_finite() || result.abs().max_element() > 20_000_000.0 {
            return Err("挤出后的顶点超过本地±10000米范围".into());
        }
        points.push(result.to_array().map(|value| value as i32));
        cap.push((vertices + corner) as u32);
    }
    if (0..corners).all(|corner| {
        points[vertices + corner] == points[old_face.indices()[corner] as usize]
    }) {
        return Ok(before.clone()); // 小于量化精度的鼠标位移不能生成零厚度侧面或空历史。
    }
    let mut boundaries = Vec::with_capacity(faces + corners);
    boundaries.extend_from_slice(geometry.faces());
    boundaries[face] = MeshFace::new(&cap)?; // 顶帽保持原face索引，确认后仍可直接继续挤出该面。
    for side in 0..corners {
        let next = (side + 1) % corners;
        boundaries.push(MeshFace::new(&[
            old_face.indices()[side],
            old_face.indices()[next],
            cap[next],
            cap[side],
        ])?);
    }
    // 有效负距可以采用；穿入旧表面、重叠侧壁或翻面时完整拒绝候选。
    let target = before.asset_target.with_editable_geometry(
        EditableMeshGeometry::from_owned_parts(points, boundaries)?,
    )?;
    let mut after = before.clone();
    after.asset_target = target;
    Ok(after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScenePlacementAssetTarget;

    fn record() -> ScenePlacementRecord {
        serde_json::from_value(serde_json::json!({
            "id":"mesh-extrusion", "asset_path":"", "asset_target":ScenePlacementAssetTarget::DEFAULT_CUBOID,
            "translation":[3,4,5], "yaw_degrees":37, "scale":[2,3,0.5],
            "height_mode":"follow_terrain", "surface_offset":0.271
        })).unwrap()
    }

    fn world_points(record: &ScenePlacementRecord) -> Vec<DVec3> {
        let geometry = record.asset_target.editable_geometry().unwrap();
        let origin = DVec3::from_array(record.translation.map(f64::from));
        let scale = DVec3::from_array(record.scale.map(f64::from)) * 0.0005;
        let rotation = DQuat::from_rotation_y(f64::from(record.yaw_degrees).to_radians());
        geometry.vertices().iter().map(|point| {
            origin + rotation * (DVec3::from_array(point.map(f64::from)) * scale)
        }).collect()
    }

    #[test]
    fn extrusion_preserves_full_record_and_measures_along_world_normal_under_object_transform() {
        let before = record();
        let face = 1;
        let normal = extrusion_normal(&before, face).unwrap();
        let old_points = world_points(&before);
        let old_geometry = before.asset_target.editable_geometry().unwrap();
        let after = extruded_placement_record(&before, face, 0.7).unwrap();
        let geometry = after.asset_target.editable_geometry().unwrap();
        assert_eq!((geometry.vertex_count(), geometry.face_count()), (12, 10));
        assert_eq!(geometry.faces()[face].indices(), [8, 9, 10, 11]);
        let new_points = world_points(&after);
        for (corner, index) in old_geometry.faces()[face].indices().iter().enumerate() {
            let delta = new_points[8 + corner] - old_points[*index as usize];
            assert!((delta - normal.as_dvec3() * 0.7).length() < 0.0015);
        }
        let mut expected = before.clone();
        expected.asset_target = after.asset_target.clone();
        assert_eq!(after, expected);
        let target = serde_json::to_value(&after.asset_target).unwrap();
        assert_eq!(target["color_srgb"], serde_json::json!([160, 160, 160]));
        assert_eq!(target["roughness_milli"], 700);
        assert_eq!(target["collision"], true);
        let decoded: ScenePlacementRecord =
            serde_json::from_str(&serde_json::to_string(&after).unwrap()).unwrap();
        assert_eq!(decoded, after);
        let again = extruded_placement_record(&after, face, 0.2).unwrap();
        assert_eq!(
            again.asset_target.editable_geometry().unwrap().vertex_count(),
            16
        );
    }

    #[test]
    fn extrusion_zero_subquantum_invalid_distance_and_penetrating_inward_motion_do_not_apply() {
        let before = record();
        assert_eq!(extruded_placement_record(&before, 3, 0.0).unwrap(), before);
        assert_eq!(
            extruded_placement_record(&before, 3, 0.000001).unwrap(),
            before
        );
        assert!(extruded_placement_record(&before, 3, f32::NAN).is_err());
        assert!(extruded_placement_record(&before, 60, 0.2).is_err());
        assert!(
            extruded_placement_record(&before, 3, -0.3).is_err(),
            "直立盒壁上向内折返会与旧侧壁重叠，应拒绝"
        );
    }

    #[test]
    fn extrusion_normal_does_not_collapse_when_object_origin_is_far_from_world_zero() {
        let before = record();
        let expected = extrusion_normal(&before, 1).unwrap();
        let mut far = before.clone();
        far.translation = [20_000_000.0; 3];
        assert!(extrusion_normal(&far, 1).unwrap().distance(expected) < 1e-6);
        assert!(extruded_placement_record(&far, 1, 0.25).is_ok());
    }

    #[test]
    fn extrusion_valid_inward_cap_forms_concavity_and_can_grow_past_legacy_limit() {
        let mut before = record();
        before.scale = [1.0; 3];
        let mut vertices = before.asset_target.editable_vertices_half_mm().unwrap();
        for index in [2, 3, 6, 7] {
            vertices[index][0] /= 2;
            vertices[index][2] /= 2;
        }
        before.asset_target = ScenePlacementAssetTarget::EditableCuboid {
            vertices_half_mm: vertices,
            color_srgb: [3, 4, 5],
            roughness_milli: 500,
            collision: false,
        };
        before.asset_target.validate().unwrap();
        let after = extruded_placement_record(&before, 3, -0.25).unwrap();
        let geometry = after.asset_target.editable_geometry().unwrap();
        assert_eq!(geometry.vertex_count(), 12);
        assert!(
            geometry.vertices()[8..12]
                .iter()
                .all(|point| point[1] == 1_500)
        );
        assert!(matches!(
            after.asset_target,
            ScenePlacementAssetTarget::EditableMesh {
                collision: false,
                ..
            }
        ));
        let mut current = record();
        for _ in 0..15 {
            current = extruded_placement_record(&current, 3, 0.1).unwrap();
        }
        assert_eq!(
            current
                .asset_target
                .editable_geometry()
                .unwrap()
                .vertex_count(),
            68
        );
    }
}
