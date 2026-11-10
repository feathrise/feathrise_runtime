//! 场景天气身份的读取兼容回归；使用既有空场景形状，不读写原游戏素材。

use crate::{SceneManifestAsset, SceneWeatherClimate, SceneWeatherRegion, validate_terrain_objects};
use serde_json::{Value, json};

/// 沿用terrain_objects测试的中性空容器，不添加地面、角色或外部资产依赖。
fn manifest_source() -> Value {
    json!({
        "schema_version": 2, "id": "MAP-weather-test", "display_name": "天气兼容测试",
        "notes": "作者备注\n保持原值", "runtime_scene": "data_driven",
        "world_min": [-64.0, -64.0], "world_max": [64.0, 64.0],
        "default_spawn": [0.0, 2.0, 0.0], "terrains": []
    })
}

fn validate_neutral(manifest: &SceneManifestAsset) {
    manifest
        .environment
        .validate()
        .expect("规范化天气必须通过正式环境校验");
    validate_terrain_objects(manifest).expect("天气兼容不得破坏已有空场景容器校验");
    assert_eq!(
        manifest.environment.weather.region,
        SceneWeatherRegion::Global
    );
    assert_eq!(
        manifest.environment.weather.climate,
        SceneWeatherClimate::Uniform
    );
}

fn canonical_weather(manifest: &SceneManifestAsset) -> Value {
    let saved = serde_json::to_value(manifest).expect("完整清单必须可以正常保存");
    assert_eq!(
        saved["environment"]["weather"],
        json!({"region":"global", "climate":"uniform"})
    );
    saved
}

#[test]
fn missing_scene_weather_fields_default_to_global_uniform() {
    for environment in [
        None,
        Some(json!({})),
        Some(json!({"weather":{}})),
        Some(json!({"weather":{"region":"global"}})),
        Some(json!({"weather":{"climate":"uniform"}})),
        Some(json!({"weather":{"region":"kaifeng"}})),
        Some(json!({"weather":{"climate":"yellow_river_floodplain"}})),
    ] {
        let mut source = manifest_source();
        if let Some(environment) = environment {
            source["environment"] = environment;
        }
        let manifest: SceneManifestAsset = serde_json::from_value(source).unwrap();
        validate_neutral(&manifest);
        canonical_weather(&manifest);
    }
}

#[test]
fn legacy_scene_weather_profiles_validate_and_serialize_only_neutral_names() {
    for climate in ["kaifeng_plain", "yellow_river_floodplain"] {
        let mut source = manifest_source();
        source["environment"] = json!({"weather":{"region":"kaifeng", "climate":climate}});
        let manifest: SceneManifestAsset = serde_json::from_value(source).unwrap();
        validate_neutral(&manifest);
        let saved = canonical_weather(&manifest);
        assert_eq!(saved["id"], "MAP-weather-test");
        assert_eq!(saved["notes"], "作者备注\n保持原值");
        assert_eq!(saved["terrains"], json!([]));
        let reopened: SceneManifestAsset = serde_json::from_value(saved).unwrap();
        validate_neutral(&reopened);
        assert_eq!(reopened.environment.weather, manifest.environment.weather);
        assert_eq!(reopened.terrain_count(), 0);
    }
}

#[test]
fn unknown_scene_weather_identities_are_rejected_in_full_manifests() {
    // 仅列出的退役拼写是兼容入口；未知地域、大小写或拼错的新身份不能被当作缺省。
    for (field, invalid) in [
        ("region", "unknown_region"),
        ("region", "Global"),
        ("region", "gloabl"),
        ("climate", "unknown_climate"),
        ("climate", "unifrom"),
        ("climate", "kaifeng_plains"),
        ("climate", "yellow_river_floodplain_typo"),
    ] {
        let mut source = manifest_source();
        source["environment"] = json!({"weather":{"region":"global", "climate":"uniform"}});
        source["environment"]["weather"][field] = json!(invalid);
        let error = serde_json::from_value::<SceneManifestAsset>(source).unwrap_err();
        assert!(
            error.to_string().contains("unknown variant"),
            "{field}={invalid}: {error}"
        );
    }
}
