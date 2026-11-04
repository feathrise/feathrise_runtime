//! 单面挤出纯运算：冻结完整记录，世界法线定距，局部半毫米量化后原子返回完整候选。

use crate::{
    MAX_EDITABLE_MESH_FACES, MAX_EDITABLE_MESH_VERTICES, MapPlacementAssetTarget,
    MapPlacementRecord, MeshFace,
};
use bevy::{
    math::{DQuat, DVec3},
    prelude::*,
};

fn world_points(before: &MapPlacementRecord) -> Result<Vec<DVec3>, String> {
    before.asset_target.validate()?;
    let geometry = before
        .asset_target
        .editable_geometry()
        .ok_or("当前对象不支持面挤出")?;
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
    Ok(geometry
        .vertices()
        .iter()
        .map(|point| {
            origin.as_dvec3()
                + rotation
                    * (DVec3::from_array(point.map(|value| f64::from(value) * 0.0005))
                        * scale.as_dvec3())
        })
        .collect())
}

pub fn extrusion_normal(before: &MapPlacementRecord, face: usize) -> Result<Vec3, String> {
    let geometry = before
        .asset_target
        .editable_geometry()
        .ok_or("当前对象不支持面挤出")?;
    geometry.faces().get(face).ok_or("所选面已失效")?;
    let points = world_points(before)?;
    geometry
        .triangles_with_faces()
        .into_iter()
        .filter(|(index, _)| *index == face)
        .map(|(_, triangle)| {
            let [a, b, c] = triangle.map(|index| points[index]);
            (b - a).cross(c - a)
        })
        .sum::<DVec3>()
        .try_normalize()
        .map(|normal| normal.as_vec3())
        .ok_or_else(|| "所选面法线退化，无法挤出".into())
}

pub fn extruded_placement_record(
    before: &MapPlacementRecord,
    face: usize,
    world_distance: f32,
) -> Result<MapPlacementRecord, String> {
    if !world_distance.is_finite() {
        return Err("挤出距离不是有限数值".into());
    }
    let mut geometry = before
        .asset_target
        .editable_geometry()
        .ok_or("当前对象不支持面挤出")?;
    let old_face = *geometry.faces().get(face).ok_or("所选面已失效")?;
    before.asset_target.validate()?;
    if world_distance == 0.0 {
        return Ok(before.clone());
    }
    let vertices = usize::from(geometry.vertex_count);
    let faces = usize::from(geometry.face_count);
    let corners = old_face.indices().len();
    if vertices + corners > MAX_EDITABLE_MESH_VERTICES || faces + corners > MAX_EDITABLE_MESH_FACES
    {
        return Err("面挤出最多支持64个共享顶点、64个面，请撤销或拆分对象后继续".into());
    }
    let normal = extrusion_normal(before, face)?;
    let local_delta = (DQuat::from_rotation_y(-f64::from(before.yaw_degrees).to_radians())
        * normal.as_dvec3()
        * f64::from(world_distance))
        / Vec3::from_array(before.scale).as_dvec3()
        * 2_000.0;
    let mut cap = Vec::with_capacity(corners);
    for (corner, vertex) in old_face.indices().iter().copied().enumerate() {
        let source =
            DVec3::from_array(geometry.vertices_half_mm[usize::from(vertex)].map(f64::from));
        let result = (source + local_delta).round();
        if !result.is_finite() || result.abs().max_element() > 20_000_000.0 {
            return Err("挤出后的顶点超过本地±10000米范围".into());
        }
        geometry.vertices_half_mm[vertices + corner] = result.to_array().map(|value| value as i32);
        cap.push((vertices + corner) as u8);
    }
    if (0..corners).all(|corner| {
        geometry.vertices_half_mm[vertices + corner]
            == geometry.vertices_half_mm[usize::from(old_face.indices()[corner])]
    }) {
        return Ok(before.clone()); // 小于量化精度的鼠标位移不能生成零厚度侧面或空历史。
    }
    geometry.faces[face] = MeshFace::new(&cap)?; // 顶帽保持原face索引，确认后仍可直接继续挤出该面。
    for side in 0..corners {
        let next = (side + 1) % corners;
        geometry.faces[faces + side] = MeshFace::new(&[
            old_face.indices()[side],
            old_face.indices()[next],
            cap[next],
            cap[side],
        ])?;
    }
    geometry.vertex_count += corners as u8;
    geometry.face_count += corners as u8;
    let (color_srgb, roughness_milli, collision) = match before.asset_target {
        MapPlacementAssetTarget::Cuboid {
            color_srgb,
            roughness_milli,
            collision,
            ..
        }
        | MapPlacementAssetTarget::EditableCuboid {
            color_srgb,
            roughness_milli,
            collision,
            ..
        }
        | MapPlacementAssetTarget::EditableMesh {
            color_srgb,
            roughness_milli,
            collision,
            ..
        } => (color_srgb, roughness_milli, collision),
        _ => return Err("当前对象不支持面挤出".into()),
    };
    let target = MapPlacementAssetTarget::EditableMesh {
        geometry,
        color_srgb,
        roughness_milli,
        collision,
    };
    target.validate()?; // 有效负距可以采用；穿入旧表面、重叠侧壁或翻面时保留原记录并返回原因。
    let mut after = before.clone();
    after.asset_target = target;
    Ok(after)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> MapPlacementRecord {
        serde_json::from_value(serde_json::json!({
            "id":"mesh-extrusion", "asset_path":"", "asset_target":MapPlacementAssetTarget::DEFAULT_CUBOID,
            "translation":[3,4,5], "yaw_degrees":37, "scale":[2,3,0.5],
            "height_mode":"follow_terrain", "surface_offset":0.271
        })).unwrap()
    }

    #[test]
    fn extrusion_preserves_full_record_and_measures_along_world_normal_under_object_transform() {
        let before = record();
        let face = 1;
        let normal = extrusion_normal(&before, face).unwrap();
        let old_points = world_points(&before).unwrap();
        let old_geometry = before.asset_target.editable_geometry().unwrap();
        let after = extruded_placement_record(&before, face, 0.7).unwrap();
        let geometry = after.asset_target.editable_geometry().unwrap();
        assert_eq!((geometry.vertex_count, geometry.face_count), (12, 10));
        assert_eq!(geometry.faces()[face].indices(), [8, 9, 10, 11]);
        let new_points = world_points(&after).unwrap();
        for (corner, index) in old_geometry.faces()[face].indices().iter().enumerate() {
            let delta = new_points[8 + corner] - old_points[usize::from(*index)];
            assert!((delta - normal.as_dvec3() * 0.7).length() < 0.0015);
        }
        let mut expected = before.clone();
        expected.asset_target = after.asset_target;
        assert_eq!(after, expected);
        let target = serde_json::to_value(after.asset_target).unwrap();
        assert_eq!(target["color_srgb"], serde_json::json!([160, 160, 160]));
        assert_eq!(target["roughness_milli"], 700);
        assert_eq!(target["collision"], true);
        let decoded: MapPlacementRecord =
            serde_json::from_str(&serde_json::to_string(&after).unwrap()).unwrap();
        assert_eq!(decoded, after);
        let again = extruded_placement_record(&after, face, 0.2).unwrap();
        assert_eq!(
            again.asset_target.editable_geometry().unwrap().vertex_count,
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
    fn extrusion_valid_inward_cap_forms_concavity_and_capacity_failure_is_explicit() {
        let mut before = record();
        before.scale = [1.0; 3];
        let mut vertices = before.asset_target.editable_vertices_half_mm().unwrap();
        for index in [2, 3, 6, 7] {
            vertices[index][0] /= 2;
            vertices[index][2] /= 2;
        }
        before.asset_target = MapPlacementAssetTarget::EditableCuboid {
            vertices_half_mm: vertices,
            color_srgb: [3, 4, 5],
            roughness_milli: 500,
            collision: false,
        };
        before.asset_target.validate().unwrap();
        let after = extruded_placement_record(&before, 3, -0.25).unwrap();
        let geometry = after.asset_target.editable_geometry().unwrap();
        assert_eq!(geometry.vertex_count, 12);
        assert!(
            geometry.vertices()[8..12]
                .iter()
                .all(|point| point[1] == 1_500)
        );
        assert!(matches!(
            after.asset_target,
            MapPlacementAssetTarget::EditableMesh {
                collision: false,
                ..
            }
        ));
        let mut current = record();
        for _ in 0..14 {
            current = extruded_placement_record(&current, 3, 0.1).unwrap();
        }
        assert_eq!(
            current
                .asset_target
                .editable_geometry()
                .unwrap()
                .vertex_count,
            64
        );
        let frozen = current.clone();
        assert!(
            extruded_placement_record(&current, 3, 0.1)
                .unwrap_err()
                .contains("64")
        );
        assert_eq!(current, frozen);
    }
}
