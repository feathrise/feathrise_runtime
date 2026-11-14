//! 作者协议的行为回归：明确空图、无损旧图映射、稳定身份和完整变换。

use super::*;
use bevy::prelude::Vec3;

fn blank() -> SceneManifestAsset {
    serde_json::from_value(serde_json::json!({
        "schema_version": 2, "id": "MAP-terrain-test", "display_name": "空白测试",
        "runtime_scene": "data_driven", "world_min": [-64.0, -64.0],
        "world_max": [64.0, 64.0], "default_spawn": [0.0, 2.0, 0.0], "terrains": []
    })).unwrap()
}

fn glb() -> SceneTerrainObject {
    SceneTerrainObject {clipmap_stitches: Default::default(),
        mesh_lod: None,
        id: TerrainId::parse("terrain-a").unwrap(), display_name: "地形A".into(), enabled: true,
        transform: Default::default(), world_min: [-8.0, -8.0], world_max: [8.0, 8.0],
        terrain: SceneTerrainDescriptor {resolved_stitches: Default::default(), kind: SceneTerrainKind::Gltf,
            glb_path: "scenes/MAP-terrain-test/terrains/terrain-a/terrain.glb".into(),
            resolution: [17, 17], base_height: -8.0, height_scale: 16.0,
            ..SceneTerrainDescriptor::empty() },
        voxel: None, surface_path: None, vegetation_path: None, water_path: None, puddles: None,
    }
}

#[test]
fn empty_scene_round_trip_never_recreates_a_primary_surface() {
    let map = blank();
    validate_terrain_objects(&map).unwrap();
    assert_eq!(map.terrain_count(), 0);
    assert!(map.terrain_objects_for_edit().is_empty());
    let value = serde_json::to_value(&map).unwrap();
    assert!(value.get("terrain").is_none());
    assert_eq!(value["terrains"], serde_json::json!([]));
    assert_eq!(serde_json::from_value::<SceneManifestAsset>(value).unwrap().terrain_count(), 0);
}

#[test]
fn rectangular_clipmap_object_round_trip_keeps_minimum_requested_spacing_grid() {
    let mut map = blank();
    let mut object = glb();
    object.world_min = [-48.55, -47.7];
    object.world_max = [48.55, 47.7];
    let spec = crate::SparseHeightSpec::new(object.world_min, object.world_max, 0.125, -46.345, 92.69).unwrap();
    object.terrain = SceneTerrainDescriptor {
        kind: SceneTerrainKind::SparseClipmap,
        height_path: Some(format!("{}/height_tiles.scene_height_manifest.json", object.private_folder(&map.id))),
        resolution: spec.cells.map(|count| count as u32 + 1),
        base_height: spec.base_height,
        height_scale: spec.height_scale,
        collision_lods: crate::default_collision_lods_for_resolution(spec.cells.map(|count| count as u32 + 1)),
        display_lods: Some(crate::default_display_lods_for_spec(&spec).unwrap()),
        ..SceneTerrainDescriptor::empty()
    };
    map.terrains = Some(vec![object.clone()].into());
    validate_terrain_objects(&map).expect("非整除矩形的合法源不能在集合采用时被拒绝");
    let reopened: SceneManifestAsset = serde_json::from_slice(&serde_json::to_vec(&map).unwrap()).unwrap();
    validate_terrain_objects(&reopened).expect("保存重开必须使用同一两轴拓扑校验");
    assert_eq!(reopened.terrains.as_ref().unwrap()[0], object);
    let restored = crate::SparseHeightSpec::from_resolution(object.world_min, object.world_max,
        object.terrain.resolution, spec.base_height, spec.height_scale).unwrap();
    crate::compile_display_lod_plan(&restored, object.terrain.display_lods.as_ref().unwrap()).unwrap();
    crate::validate_collision_lods(&object.terrain.collision_lods, &restored).unwrap();
    assert_eq!(restored.actual_spacing_meters, spec.actual_spacing_meters);
}


#[test]
fn mesh_lod_override_round_trips_without_migrating_stable_terrain_identity_or_source_paths() {
    let mut object = glb();
    let old_json = serde_json::to_value(&object).unwrap();
    assert!(old_json.get("mesh_lod").is_none());
    let old: SceneTerrainObject = serde_json::from_value(old_json).unwrap();
    assert!(old.mesh_lod.is_none());
    let folder = object.private_folder(&crate::SceneId::parse("MAP-terrain-test").unwrap());
    object.mesh_lod = Some(crate::MeshLodPolicy {
        display: crate::MeshLodMode::Override(crate::MeshDisplayLodSettings { max_error_pixels: 0.5, ..Default::default() }),
        collision: crate::MeshLodMode::Disabled,
    });
    let restored: SceneTerrainObject = serde_json::from_value(serde_json::to_value(&object).unwrap()).unwrap();
    assert_eq!(restored, object);
    assert_eq!(restored.id, old.id);
    assert_eq!(restored.terrain, old.terrain);
    assert_eq!(restored.private_folder(&crate::SceneId::parse("MAP-terrain-test").unwrap()), folder);
    let mut map = blank();
    map.terrains = Some(vec![restored].into());
    validate_terrain_objects(&map).unwrap();
    let history = SceneTerrainCollection::capture(&map);
    map.terrains = Some(Vec::new().into());
    history.apply(&mut map);
    assert_eq!(map.terrains.as_ref().unwrap()[0].mesh_lod, object.mesh_lod);
}

#[test]
fn malformed_per_mesh_lod_is_rejected_by_the_formal_terrain_validation() {
    let mut object = glb();
    object.mesh_lod = Some(crate::MeshLodPolicy {
        collision: crate::MeshLodMode::Override(crate::MeshCollisionLodSettings { safe_near_meters: 0.0, ..Default::default() }),
        ..Default::default()
    });
    let mut map = blank();
    map.terrains = Some(vec![object].into());
    assert!(validate_terrain_objects(&map).unwrap_err().contains("安全近区"));
}

#[test]
fn legacy_mapping_retains_source_and_does_not_migrate_unrelated_saves() {
    let mut map = blank();
    map.terrains = None;
    map.terrain = glb().terrain;
    let before = serde_json::to_value(&map).unwrap();
    let objects = map.terrain_objects_for_edit();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].id, TerrainId::legacy());
    assert_eq!(objects[0].terrain, map.terrain);
    assert_eq!(serde_json::to_value(&map).unwrap(), before);
    assert!(before.get("terrains").is_none());
}

#[test]
fn duplicate_ids_and_ambiguous_primary_are_rejected() {
    let mut map = blank();
    map.terrains = Some(vec![glb(), glb()].into());
    assert!(validate_terrain_objects(&map).unwrap_err().contains("重复"));
    map.terrains = Some(vec![glb()].into());
    validate_terrain_objects(&map).unwrap();
    map.terrain = glb().terrain;
    assert!(validate_terrain_objects(&map).is_err());
}

#[test]
fn source_projection_preserves_owner_and_isolates_other_terrain_objects() {
    let mut map = blank();
    map.terrains = Some(vec![glb()].into());
    let projection = glb().source_manifest(&map);
    assert_eq!(projection.id, map.id);
    assert_eq!(projection.world_min, [-8.0; 2]);
    assert!(projection.terrains.is_none());
    assert_eq!(map.terrain_count(), 1);
    assert!(map.terrain.is_empty());
}

#[test]
fn translation_yaw_and_nonuniform_scale_share_an_invertible_frame() {
    let pose = TerrainObjectTransform { translation: [30.0, 7.0, -12.0],
        yaw_degrees: 90.0, scale: [2.0, 3.0, 4.0],  ..Default::default() };
    pose.validate().unwrap();
    let matrix = pose.as_transform().to_matrix();
    let point = Vec3::new(2.0, -1.0, 3.0);
    assert!((matrix.inverse().transform_point3(matrix.transform_point3(point)) - point).length() < 0.0001);
    for scale in [[0.0, 1.0, 1.0], [-1.0, 1.0, 1.0], [f32::NAN, 1.0, 1.0]] {
        assert!(TerrainObjectTransform { scale, ..pose }.validate().is_err());
    }
}

#[test]
fn terrain_and_scene_instance_ids_have_distinct_paths_and_correct_scaled_bounds() {
    use crate::{SceneId, SceneInstanceId, SceneInstancePlacement, SceneInstanceSource, expand_scene_instances};
    use std::{collections::BTreeMap, sync::Arc};
    let map = blank(); let mut object = glb();
    object.transform = TerrainObjectTransform { translation: [40., 7., 10.], yaw_degrees: 90., scale: [2.,3.,4.],  ..Default::default() };
    let child = SceneId::parse("MAP-child").unwrap();
    let reference = SceneInstancePlacement { id: SceneInstanceId::parse(object.id.as_str()).unwrap(), source_scene_id: child.clone(),
        label: "完整子场景".into(), transform: Default::default() };
    let sources = BTreeMap::from([(map.id.clone(), Arc::new(SceneInstanceSource { scene_id: map.id.clone(), revision: 1,
        world_bounds: map.world_bounds().unwrap(), instances: Arc::from([reference]), terrains: Some(Arc::from([object])) })),
        (child.clone(), Arc::new(SceneInstanceSource { scene_id: child, revision: 1, world_bounds: map.world_bounds().unwrap(),
            instances: Arc::from([]), terrains: None }))]);
    let nodes = expand_scene_instances(&map.id, &sources).unwrap();
    assert_eq!(nodes.len(), 3);
    assert_ne!(nodes[1].path, nodes[2].path);
    assert!(nodes[1].terrain.is_some()); assert!(nodes[2].terrain.is_none());
    assert!((nodes[1].world_bounds.size() - bevy::prelude::Vec2::new(64.,32.)).length() < 0.001);
    assert!((nodes[1].world_pose().transform_point(Vec3::Y).y - 10.).abs() < 0.001);
}

#[test]
fn terrain_xyz_protocol_keeps_legacy_defaults_and_serializes_new_angles() {
    let legacy = serde_json::json!({
        "translation": [3.0, -4.0, 5.0], "yaw_degrees": 73.0, "scale": [2.0, 3.0, 0.5]
    });
    let pose: TerrainObjectTransform = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!((pose.pitch_degrees, pose.roll_degrees), (0.0, 0.0));
    assert_eq!(serde_json::to_value(pose).unwrap(), legacy);
    let tilted = TerrainObjectTransform { pitch_degrees: -23., roll_degrees: 61., ..pose };
    let json = serde_json::to_value(tilted).unwrap();
    assert_eq!(json["pitch_degrees"], -23.);
    assert_eq!(json["roll_degrees"], 61.);
    assert_eq!(serde_json::from_value::<TerrainObjectTransform>(json).unwrap(), tilted);
}

#[test]
fn terrain_xyz_uses_yxz_rotation_and_rejects_every_nonfinite_angle() {
    use bevy::prelude::{Quat, Transform};
    let pose = TerrainObjectTransform { translation: [30., 7., -12.], yaw_degrees: 37.,
        pitch_degrees: -23., roll_degrees: 61., scale: [2., 3., 0.5] };
    pose.validate().unwrap();
    // 用三个独立轴的四元数乘积核验次序，避免只把生产转换再调用一次当作期望。
    let rotation = Quat::from_rotation_y(37_f32.to_radians())
        * Quat::from_rotation_x((-23_f32).to_radians())
        * Quat::from_rotation_z(61_f32.to_radians());
    let expected = Transform::from_translation(Vec3::from_array(pose.translation))
        .with_rotation(rotation).with_scale(Vec3::from_array(pose.scale));
    for point in [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(2., -3., 5.)] {
        let world = pose.as_transform().transform_point(point);
        assert!(world.abs_diff_eq(expected.transform_point(point), 0.0001));
        assert!(pose.as_transform().to_matrix().inverse().transform_point3(world).abs_diff_eq(point, 0.0001));
    }
    let decomposed = pose.with_rotation(rotation);
    assert!(decomposed.rotation().dot(rotation).abs() > 0.99999);
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for axis in 0..3 {
            let mut broken = pose;
            match axis { 0 => broken.pitch_degrees = invalid, 1 => broken.yaw_degrees = invalid,
                _ => broken.roll_degrees = invalid }
            assert!(broken.validate().is_err(), "非有限旋转轴{axis}必须在矩阵采用前拒绝");
        }
    }
}

#[test]
fn vertical_flat_terrain_expands_with_finite_projected_bounds_and_full_pose() {
    use crate::{SceneInstanceSource, expand_scene_instances};
    use std::{collections::BTreeMap, sync::Arc};
    let map = blank();
    let mut object = glb();
    object.terrain.base_height = 0.;
    object.terrain.height_scale = 0.;
    object.transform.translation = [10., 4., 2.];
    object.transform.pitch_degrees = 90.;
    let sources = BTreeMap::from([(map.id.clone(), Arc::new(SceneInstanceSource {
        scene_id: map.id.clone(), revision: 1, world_bounds: map.world_bounds().unwrap(),
        instances: Arc::from([]), terrains: Some(Arc::from([object.clone()])),
    }))]);
    let nodes = expand_scene_instances(&map.id, &sources).expect("合法竖直平面不能因XZ投影退化被当作空对象拒绝");
    let terrain = nodes.iter().find(|node| node.terrain.is_some()).unwrap();
    assert!(terrain.world_bounds.min.is_finite() && terrain.world_bounds.max.is_finite());
    assert!(terrain.world_bounds.min.cmplt(terrain.world_bounds.max).all());
    for point in [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z] {
        assert!(terrain.world_pose().transform_point(point)
            .abs_diff_eq(object.transform.as_transform().transform_point(point), 0.0001));
    }
}
