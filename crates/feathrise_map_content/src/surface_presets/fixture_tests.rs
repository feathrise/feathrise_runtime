//! 独立SDK地表目录回归：合成输入覆盖空目录、任意容量和稳定引用，不读取游戏素材。

use super::*;

fn fixture_catalog() -> SurfacePresetCatalogAsset {
    serde_json::from_str(include_str!("../../tests/fixtures/surface_presets.json"))
        .expect("SDK中性地表夹具必须可读取")
}

#[test]
fn empty_project_surface_catalog_is_valid_without_implicit_materials() {
    let mut asset = fixture_catalog();
    asset.presets.clear();
    let resolved =
        ResolvedSurfacePresetCatalog::build(&asset).expect("新建空工程不应被要求附送任何地表预设");
    assert!(resolved.presets().is_empty());
    assert!(resolved.resolve("fixture-surface").is_none());
    assert_eq!(resolved.texture_resolution(), [1, 1]);
}

#[test]
fn catalog_protocol_has_no_artificial_preset_count_limit() {
    let mut asset = fixture_catalog();
    let template = asset.presets[0].clone();
    asset.presets = (0..1_000)
        .map(|index| {
            let mut preset = template.clone();
            preset.id = format!("surface-{index}");
            preset.label = format!("测试地表{index}");
            preset
        })
        .collect();
    let resolved =
        ResolvedSurfacePresetCatalog::build(&asset).expect("一千个唯一预设不应被固定Rust容量拒绝");
    assert_eq!(resolved.presets().len(), 1_000);
    assert_eq!(resolved.texture_slice("surface-999"), Some(999));

    // 地图持久化稳定ID；重排目录只改变当代数组下标，不能改变材质身份。
    let layer = resolved.resolve("surface-999").unwrap().create_map_layer();
    asset.presets.reverse();
    let reordered = ResolvedSurfacePresetCatalog::build(&asset).unwrap();
    assert_eq!(layer.preset_id, "surface-999");
    assert_eq!(reordered.texture_slice(&layer.preset_id), Some(0));
}
