//! 持久姿态与兼容默认值回归，使用实际非原点变换和非等比缩放。
use super::*;
use crate::{TerrainId, TerrainObjectTransform};
use bevy::prelude::*;
#[test]
fn binding_round_trip_and_world_pose_follow_translation_yaw_and_scale() {
    let terrain = TerrainObjectTransform {
        translation: [30., 4., -20.],
        yaw_degrees: 90.,
        scale: [2., 3., 4.],
        ..Default::default()
    };
    let world = TerrainObjectTransform {
        translation: [38., 10., -22.],
        yaw_degrees: 135.,
        scale: [4., 6., 8.],
        ..Default::default()
    };
    let binding =
        SceneTerrainBinding::capture(TerrainId::parse("island").unwrap(), terrain, world).unwrap();
    let json = serde_json::to_string(&binding).unwrap();
    let binding: SceneTerrainBinding = serde_json::from_str(&json).unwrap();
    let restored = binding.resolve(terrain).unwrap();
    assert!(
        Vec3::from_array(restored.translation)
            .abs_diff_eq(Vec3::from_array(world.translation), 0.0001)
    );
    assert_eq!(restored.yaw_degrees, 135.);
    assert_eq!(restored.scale, world.scale);
    let moved = TerrainObjectTransform {
        translation: [100., 20., 5.],
        yaw_degrees: 180.,
        scale: [4., 6., 8.],
        ..Default::default()
    };
    let resolved = binding.resolve(moved).unwrap();
    assert!(Vec3::from_array(resolved.translation).abs_diff_eq(Vec3::new(96., 32., -11.), 0.0001));
    assert_eq!(resolved.yaw_degrees, 225.);
    assert_eq!(resolved.scale, [8., 12., 16.]);
}
#[test]
fn absent_terrain_is_not_rebound_to_other_islands() {
    let binding =
        SceneTerrainBinding::capture(TerrainId::parse("removed").unwrap(), default(), default())
            .unwrap();
    assert!(TerrainBindingIndex::default().resolve(&binding).is_err());
}
#[test]
fn binding_xyz_legacy_json_defaults_to_zero_and_omits_new_angles() {
    let legacy = serde_json::json!({
        "terrain_id": "island", "local_pose": {
            "translation": [1.0, 2.0, 3.0], "yaw_degrees": 45.0, "scale": [1.0, 1.0, 1.0]
        }
    });
    let binding: SceneTerrainBinding = serde_json::from_value(legacy.clone()).unwrap();
    binding.validate().unwrap();
    assert_eq!((binding.local_pose.pitch_degrees, binding.local_pose.roll_degrees), (0., 0.));
    assert_eq!(serde_json::to_value(binding).unwrap(), legacy);
}

#[test]
fn tilted_binding_capture_reopen_and_resolve_use_complete_quaternion_composition() {
    let terrain = TerrainObjectTransform { translation: [30., 4., -20.], yaw_degrees: 37.,
        pitch_degrees: -28., roll_degrees: 19., scale: [2., 3., 0.5] };
    let world = TerrainObjectTransform { translation: [38., 10., -22.], yaw_degrees: -42.,
        pitch_degrees: 23., roll_degrees: -31., scale: [4., 6., 1.] };
    let binding = SceneTerrainBinding::capture(TerrainId::parse("island").unwrap(), terrain, world).unwrap();
    let saved = serde_json::to_value(&binding).unwrap();
    assert!(saved["local_pose"].get("pitch_degrees").is_some());
    assert!(saved["local_pose"].get("roll_degrees").is_some());
    let reopened: SceneTerrainBinding = serde_json::from_value(saved).unwrap();
    let expected_local_rotation = terrain.rotation().inverse() * world.rotation();
    assert!(reopened.local_pose.rotation().dot(expected_local_rotation).abs() > 0.99999);
    let restored = reopened.resolve(terrain).unwrap();
    assert!(Vec3::from_array(restored.translation).abs_diff_eq(Vec3::from_array(world.translation), 0.0001));
    assert!(restored.rotation().dot(world.rotation()).abs() > 0.99999);
    assert_eq!(restored.scale, world.scale);
    let moved = TerrainObjectTransform { translation: [100., 20., 5.], yaw_degrees: 63.,
        pitch_degrees: 34., roll_degrees: -17., scale: [4., 6., 2.] };
    let resolved = reopened.resolve(moved).unwrap();
    // 作者绑定按TRS分别组合，非等比缩放不能被错误当成欧拉角相加或矩阵剪切分解。
    let expected_position = moved.as_transform().transform_point(Vec3::from_array(reopened.local_pose.translation));
    let expected_rotation = moved.rotation() * expected_local_rotation;
    assert!(Vec3::from_array(resolved.translation).abs_diff_eq(expected_position, 0.0001));
    assert!(resolved.rotation().dot(expected_rotation).abs() > 0.99999);
    assert_eq!(resolved.scale, [8., 12., 4.]);
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut broken = reopened.clone(); broken.local_pose.pitch_degrees = invalid;
        assert!(broken.validate().is_err());
        assert!(broken.resolve(moved).is_err());
    }
}
