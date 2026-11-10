//! 场景专用JSON与无Mip R8作者遮罩资产加载器。

use std::io::{Error, ErrorKind}; // 损坏作者资产使用标准IO错误进入Bevy资产失败事件。
use std::sync::Arc; // 解码结果直接转为后台消费者共享的不可变切片。

use bevy::asset::{AssetLoader, AsyncReadExt, LoadContext, io::Reader}; // 自定义复合扩展名与有界异步分块读取。
use bevy::ecs::error::BevyError; // 统一承载IO与Serde错误。
use bevy::prelude::*; // TypePath由Bevy预导出。

use crate::{
    BuildingPresetCatalogAsset, CourtyardPresetCatalogAsset, SceneBoundariesAsset, SceneCatalogAsset,
    SceneManifestAsset, ScenePlacementsAsset, ScenePortalsAsset, SceneSurfaceAsset, SceneVegetationAsset,
    SceneWaterAsset, SceneWaterMaskAsset, SurfacePresetCatalogAsset, VegetationPresetCatalogAsset,
    WaterPresetCatalogAsset, WorldEnvironmentAsset,
};

const MAX_SCENE_WATER_MASK_BYTES: usize = usize::MAX; // 兼容读取入口；水R8不再设置内容字节总门。
const MAX_SCENE_JSON_BYTES: usize = 8 * 1024 * 1024; // 其他场景JSON沿用8MiB结构门；水网和placement绑定正文按实际可分配性读取。
const ASSET_READ_CHUNK_BYTES: usize = 8 * 1024; // 固定栈缓冲逐块读取，不按损坏文件声明预分配。

fn append_bounded(
    bytes: &mut Vec<u8>,
    chunk: &[u8],
    max_bytes: usize,
    label: &str,
) -> Result<(), Error> {
    if chunk.len() > max_bytes.saturating_sub(bytes.len()) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("{label}超过{max_bytes}字节硬上限"),
        ));
    }
    bytes.try_reserve(chunk.len()).map_err(|e| Error::other(format!("{label}读取内存分配失败：{e}")))?;
    bytes.extend_from_slice(chunk);
    Ok(())
}

pub(crate) async fn read_bounded(
    reader: &mut dyn Reader,
    max_bytes: usize,
    label: &str,
) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; ASSET_READ_CHUNK_BYTES];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Ok(bytes);
        }
        append_bounded(&mut bytes, &chunk[..read], max_bytes, label)?; // 第max+1字节立即失败，Vec从不越过硬上限。
    }
}

/// 将IO任务拥有的R8读取缓冲冻结为共享切片，空文件不得进入运行时建网格。
fn freeze_water_mask_bytes(bytes: Vec<u8>) -> Result<Arc<[u8]>, Error> {
    if bytes.is_empty() {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "R8水域作者源不能为空",
        ));
    }
    Ok(bytes.into()) // Vec转Arc切片发生在资产IO任务内，主世界只克隆Arc身份。
}

/// 只接管场景私有`.r8`水域作者源；KTX2派生纹理继续由标准Image加载器处理。
#[derive(Default, TypePath)]
pub(crate) struct SceneWaterMaskLoader;

impl AssetLoader for SceneWaterMaskLoader {
    type Asset = SceneWaterMaskAsset;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let bytes = read_bounded(reader, MAX_SCENE_WATER_MASK_BYTES, "R8水域作者源").await?;
        Ok(SceneWaterMaskAsset::from_shared(freeze_water_mask_bytes(
            bytes,
        )?))
    }

    fn extensions(&self) -> &[&str] {
        &["r8"] // 水域JSON的mask_source_path是CPU权威输入，KTX2仅保留派生用途。
    }
}

macro_rules! json_asset_loader {
    ($loader:ident, $asset:ty, $extension:literal) => {
        #[derive(Default, TypePath)]
        pub(crate) struct $loader;

        impl AssetLoader for $loader {
            type Asset = $asset;
            type Settings = ();
            type Error = BevyError;

            async fn load(
                &self,
                reader: &mut dyn Reader,
                _settings: &Self::Settings,
                _load_context: &mut LoadContext<'_>,
            ) -> Result<Self::Asset, Self::Error> {
                // 水网和GLB水绑定没有数量总门，JSON不能再以旧容量间接截断它们。
                let max_bytes = match $extension {
                    "scene_water.json" | "scene_placements.json" => usize::MAX,
                    _ => MAX_SCENE_JSON_BYTES,
                };
                let bytes = read_bounded(reader, max_bytes, "场景JSON").await?;
                Ok(serde_json::from_slice::<$asset>(&bytes)?)
            }

            fn extensions(&self) -> &[&str] {
                &[$extension]
            }
        }
    };
}

json_asset_loader!(SceneCatalogLoader, SceneCatalogAsset, "scene_catalog.json");
json_asset_loader!(
    BuildingPresetCatalogLoader,
    BuildingPresetCatalogAsset,
    "building_presets.json"
);
json_asset_loader!(
    CourtyardPresetCatalogLoader,
    CourtyardPresetCatalogAsset,
    "courtyard_presets.json"
); // 院落目录沿用有界异步 JSON 读取，不在主线程触发文件 I/O。
json_asset_loader!(
    VegetationPresetCatalogLoader,
    VegetationPresetCatalogAsset,
    "vegetation_presets.json"
);
json_asset_loader!(
    SurfacePresetCatalogLoader,
    SurfacePresetCatalogAsset,
    "surface_presets.json"
);
json_asset_loader!(
    WaterPresetCatalogLoader,
    WaterPresetCatalogAsset,
    "water_presets.json"
);
json_asset_loader!(
    WorldEnvironmentLoader,
    WorldEnvironmentAsset,
    "world_environment.json"
);
/// 引用图的局部结构在 IO 线程拒绝；间接循环由完整后台实例图继续核验。
#[derive(Default, TypePath)]
pub(crate) struct SceneManifestLoader;

/// 版本先于结构校验，避免旧文件中省略的子场景字段被误读为空列表。
fn parse_scene_manifest(bytes: &[u8]) -> Result<SceneManifestAsset, BevyError> {
    let manifest: SceneManifestAsset = serde_json::from_slice(bytes)?;
    if !manifest.is_data_driven() {
        return Err(Error::new(ErrorKind::InvalidData,
            format!("场景{}使用不支持的运行类型：{}", manifest.id, manifest.runtime_scene)).into());
    }
    crate::validate_terrain_objects(&manifest)
        .map_err(|message| Error::new(ErrorKind::InvalidData, message))?;
    crate::validate_scene_instances(&manifest.id, &manifest.subscenes)
        .map_err(|message| Error::new(ErrorKind::InvalidData, message))?;
    Ok(manifest)
}

impl AssetLoader for SceneManifestLoader {
    type Asset = SceneManifestAsset;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let bytes = read_bounded(reader, MAX_SCENE_JSON_BYTES, "场景清单").await?;
        parse_scene_manifest(&bytes)
    }

    fn extensions(&self) -> &[&str] {
        &["scene_manifest.json"]
    }
}
json_asset_loader!(ScenePortalsLoader, ScenePortalsAsset, "scene_portals.json");
/// 密度块索引完整校验只发生在资产IO任务，运行帧不逐帧扫描整张作者表。
#[derive(Default, TypePath)]
pub(crate) struct SceneVoxelTerrainSourceLoader;
impl AssetLoader for SceneVoxelTerrainSourceLoader {
    type Asset = crate::SceneVoxelTerrainSource;
    type Settings = ();
    type Error = BevyError;
    async fn load(&self, reader: &mut dyn Reader, _: &Self::Settings, _: &mut LoadContext<'_>) -> Result<Self::Asset, Self::Error> {
        let bytes = read_bounded(reader, MAX_SCENE_JSON_BYTES, "体素作者清单").await?;
        let json = bytes.strip_prefix(&[0xef,0xbb,0xbf]).unwrap_or(&bytes);
        let source: Self::Asset = serde_json::from_slice(json)?;
        source.validate().map_err(|message| Error::new(ErrorKind::InvalidData, message))?;
        Ok(source)
    }
    fn extensions(&self) -> &[&str] { &["scene_voxel_source.json"] }
}
json_asset_loader!(
    ScenePlacementsLoader,
    ScenePlacementsAsset,
    "scene_placements.json"
);
json_asset_loader!(SceneSurfaceLoader, SceneSurfaceAsset, "scene_surface.json");
json_asset_loader!(
    SceneVegetationLoader,
    SceneVegetationAsset,
    "scene_vegetation.json"
);
json_asset_loader!(SceneWaterLoader, SceneWaterAsset, "scene_water.json");
json_asset_loader!(
    SceneBoundariesLoader,
    SceneBoundariesAsset,
    "scene_boundaries.json"
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SCENE_SCHEMA_VERSION, SCENE_SURFACE_SCHEMA_VERSION};
    use serde::{Serialize, de::DeserializeOwned};
    use serde_json::{Value, json};

    #[test]
    fn manifest_loader_accepts_only_the_scene_runtime_name() {
        for (runtime, accepted) in [("data_driven", true), ("DynamicMapScene", false),
            ("KaifengScene", false), ("FutureRuntime", false), ("", false)] {
            let bytes = serde_json::to_vec(&json!({
                "schema_version": SCENE_SCHEMA_VERSION, "id": "SCENE-loader", "display_name": "类型验收",
                "runtime_scene": runtime, "world_min": [0, 0], "world_max": [1, 1],
                "default_spawn": [0, 0, 0], "terrains": []
            })).unwrap();
            let result = parse_scene_manifest(&bytes);
            assert_eq!(result.is_ok(), accepted, "{runtime}");
            if let Ok(manifest) = result {
                assert_eq!(manifest.runtime_scene, runtime); // 加载不是格式迁移操作。
            }
        }
    }

    #[test]
    fn previous_asset_versions_are_rejected_even_with_current_field_names() {
        fn verify<T: DeserializeOwned>(mut value: Value, version: u32) {
            assert!(
                serde_json::from_value::<T>(value.clone()).is_err(),
                "磁盘文件必须显式声明版本"
            );
            value["schema_version"] = version.into();
            assert!(serde_json::from_value::<T>(value.clone()).is_ok());
            value["schema_version"] = (version - 1).into();
            let error = serde_json::from_value::<T>(value).err().unwrap();
            assert!(error.to_string().contains(&format!("需要版本 {version}")));
        }
        verify::<SceneCatalogAsset>(json!({"scenes":[]}), SCENE_SCHEMA_VERSION);
        verify::<SceneManifestAsset>(json!({
            "id":"SCENE-test", "display_name":"版本校验", "runtime_scene":"data_driven",
            "world_min":[0,0], "world_max":[1,1], "default_spawn":[0,0,0], "terrains":[]
        }), SCENE_SCHEMA_VERSION);
        verify::<ScenePortalsAsset>(json!({"scene_id":"SCENE-test","portals":[]}), SCENE_SCHEMA_VERSION);
        verify::<ScenePlacementsAsset>(json!({"scene_id":"SCENE-test","placements":[]}), SCENE_SCHEMA_VERSION);
        verify::<SceneVegetationAsset>(json!({"scene_id":"SCENE-test","layers":[]}), SCENE_SCHEMA_VERSION);
        verify::<SceneWaterAsset>(json!({"scene_id":"SCENE-test","layers":[]}), SCENE_SCHEMA_VERSION);
        verify::<SceneBoundariesAsset>(json!({"scene_id":"SCENE-test","paths":[]}), SCENE_SCHEMA_VERSION);
        verify::<SceneSurfaceAsset>(json!({
            "scene_id":"SCENE-test", "layers":[], "resolution":[1,1],
            "weight_source_path":"weights.rgba8", "weight_texture_path":"weights.ktx2",
            "index_source_path":"indices.rgba32", "index_texture_path":"indices.ktx2"
        }), SCENE_SURFACE_SCHEMA_VERSION);
    }

    #[test]
    fn default_scene_assets_serialize_with_the_current_schema() {
        fn verify<T: Default + Serialize + DeserializeOwned>() {
            let mut json = serde_json::to_value(T::default()).unwrap();
            assert_eq!(json["schema_version"], SCENE_SCHEMA_VERSION);
            if let Some(scene_id) = json.get_mut("scene_id") {
                *scene_id = json!("SCENE-test"); // 私有资产仍由调用方指定所属场景。
            }
            assert!(serde_json::from_value::<T>(json).is_ok());
        }
        verify::<SceneCatalogAsset>();
        verify::<ScenePortalsAsset>();
        verify::<ScenePlacementsAsset>();
        verify::<SceneVegetationAsset>();
        verify::<SceneWaterAsset>();
    }

    #[test]
    fn r8_freezer_preserves_bytes_and_rejects_empty_author_source() {
        let frozen = freeze_water_mask_bytes(vec![0, 17, 255]).expect("非空R8作者源必须可冻结");
        assert_eq!(frozen.as_ref(), &[0, 17, 255]); // 像素字节不得经过颜色或纹理解码。
        let asset = SceneWaterMaskAsset::from_shared(Arc::clone(&frozen)); // 模拟加载器登记后的资产值。
        assert!(Arc::ptr_eq(&frozen, &asset.shared_pixels())); // 主世界冻结必须只克隆Arc而非整图。
        assert!(freeze_water_mask_bytes(Vec::new()).is_err()); // 空画布文件必须发布资产失败。
    }

    #[test]
    fn bounded_reader_append_rejects_the_first_byte_over_budget() {
        let mut bytes = Vec::new();
        append_bounded(&mut bytes, &[1, 2, 3, 4], 4, "测试资产").expect("正好达到上限合法");
        assert_eq!(bytes, [1, 2, 3, 4]);
        assert!(append_bounded(&mut bytes, &[5], 4, "测试资产").is_err());
        assert_eq!(bytes, [1, 2, 3, 4]); // 拒绝前不追加任何字节，内存永远不越过硬上限。
    }
}
