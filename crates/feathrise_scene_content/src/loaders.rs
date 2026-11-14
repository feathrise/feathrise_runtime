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
/// GLB 的摆放信息通过类型读取，标准 GltfLoader 仍负责同一文件的网格与材质子资产。
#[derive(Default, TypePath)]
pub(crate) struct ScenePlacementsLoader;
impl AssetLoader for ScenePlacementsLoader {
    type Asset = ScenePlacementsAsset;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self, reader: &mut dyn Reader, _: &Self::Settings, context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let bytes = read_bounded(reader, usize::MAX, "场景摆放").await?;
        // Windows 文件监听传入原生分隔符，内嵌网格仍须使用目录登记的统一资产路径。
        let path = context.path().path().to_str()
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "场景摆放路径不是 UTF-8"))?
            .replace('\\', "/");
        Ok(crate::read_scene_placements(&bytes, &path).map_err(|error| Error::new(ErrorKind::InvalidData, error))?)
    }

    fn extensions(&self) -> &[&str] { &["scene_placements.json"] }
}
json_asset_loader!(SceneSurfaceLoader, SceneSurfaceAsset, "scene_surface.json");
/// 模型密度块是植被文档的IO依赖；用类型加载器保持有界读取和原生热更新追踪。
#[derive(Default, TypePath)]
pub(crate) struct ModelVegetationBlockLoader;
impl AssetLoader for ModelVegetationBlockLoader {
    type Asset = crate::model_vegetation_io::Block;
    type Settings = ();
    type Error = BevyError;
    async fn load(&self, reader: &mut dyn Reader, _: &(), _: &mut LoadContext<'_>) -> Result<Self::Asset, Self::Error> {
        let bytes = read_bounded(reader, crate::model_vegetation_io::MAX_BYTES, "模型植被块").await?;
        let mut block: Self::Asset = serde_json::from_slice(&bytes)?;
        if block.samples.len() > crate::model_vegetation_io::BLOCK_SAMPLES {
            return Err(Error::new(ErrorKind::InvalidData, "模型植被块超过4096样本预算").into());
        }
        block.source_bytes = bytes.len();
        Ok(block)
    }
    fn extensions(&self) -> &[&str] { &[] } // 仅由父文档按明确类型加载，普通JSON不会误选该加载器。
}

#[derive(Default, TypePath)]
pub(crate) struct SceneVegetationLoader;
impl AssetLoader for SceneVegetationLoader {
    type Asset = SceneVegetationAsset;
    type Settings = ();
    type Error = BevyError;
    async fn load(&self, reader: &mut dyn Reader, _: &(), context: &mut LoadContext<'_>) -> Result<Self::Asset, Self::Error> {
        let bytes = read_bounded(reader, MAX_SCENE_JSON_BYTES, "场景植被文档").await?;
        let mut asset: Self::Asset = serde_json::from_slice(&bytes)?;
        let folder = context.path().path().parent().and_then(|p|p.to_str())
            .ok_or_else(|| Error::new(ErrorKind::InvalidData,"模型植被文档没有场景私有目录"))?
            .replace('\\',"/");
        let mut total = 0usize;
        let mut targets = std::collections::BTreeSet::new();
        for shared in std::sync::Arc::make_mut(&mut asset.model_surfaces) {
            let surface = std::sync::Arc::make_mut(shared);
            surface.validate().map_err(Error::other)?;
            if !targets.insert(serde_json::to_string(&surface.target)?) {
                return Err(Error::other("模型植被目标重复").into());
            }
            if surface.sample_blocks.is_empty() { continue; }
            if !surface.samples.is_empty() { return Err(Error::other("模型植被不能同时声明内联样本和分块源").into()); }
            let mut samples = Vec::new();
            for (ordinal, relative) in surface.sample_blocks.iter().enumerate() {
                crate::model_vegetation_io::validate_block_path(&folder,relative).map_err(Error::other)?;
                let block = context.load_builder().load_value::<crate::model_vegetation_io::Block>(relative.clone()).await?.take();
                total = total.checked_add(block.source_bytes).filter(|n|*n <= crate::model_vegetation_io::MAX_BYTES)
                    .ok_or_else(|| Error::other("模型植被超过64MiB读取预算"))?;
                if block.format != 1 || block.scene_id != asset.scene_id || block.target != surface.target
                    || block.topology != surface.topology || block.ordinal != ordinal {
                    return Err(Error::other("模型植被块身份或顺序不一致").into());
                }
                samples.extend(block.samples);
            }
            surface.samples = samples;
            surface.sample_blocks.clear();
            surface.validate().map_err(Error::other)?;
        }
        Ok(asset)
    }
    fn extensions(&self) -> &[&str] { &["scene_vegetation.json"] }
}
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

    #[cfg(windows)]
    #[test]
    fn private_placement_glb_reloads_without_native_path_separators_in_records() {
        use bevy::asset::{AssetApp, AssetPlugin, io::{AssetSourceBuilder, AssetSourceId,
            memory::{Dir, MemoryAssetReader}}};
        use std::{path::{Path, PathBuf}, time::{Duration, Instant}};

        const PATH: &str = "scenes/SCENE-private-reload/scene.glb";
        let native_path = ["scenes", "SCENE-private-reload", "scene.glb"].into_iter().collect::<PathBuf>();
        assert_ne!(native_path.as_os_str(), Path::new(PATH).as_os_str()); // 原生监听器按组件重组路径。
        let mut records: ScenePlacementsAsset = serde_json::from_value(json!({
            "schema_version":SCENE_SCHEMA_VERSION,"scene_id":"SCENE-private-reload","placements":[{
                "id":"bridge","translation":[0.,1.,0.],"yaw_degrees":0.,"scale":[1.,1.,1.],
                "asset_target":{"kind":"cuboid","size_mm":[1000,100,1000],
                    "color_srgb":[80,100,120],"roughness_milli":800,"collision":false}
            },{"id":"shared","translation":[0.,1.,0.],"yaw_degrees":0.,"scale":[1.,1.,1.],
                "asset_path":"scene/shared.glb"}]
        })).unwrap();
        let initial = crate::write_scene_placements_glb(&records, PATH, None).unwrap();
        // 移除作者配方可信标记，使该内嵌网格按真实 Primitive 解析，和外部模型共用一个摆放文档。
        let mut initial = initial;
        let key = b"feathrise_geometry_hash";
        let offset = initial.windows(key.len()).position(|window| window == key).unwrap();
        initial[offset] = b'x';
        records = crate::read_scene_placements(&initial, PATH).unwrap();
        assert!(matches!(records.placements[0].asset_target, crate::ScenePlacementAssetTarget::Primitive { .. }));

        let assets = Dir::default();
        assets.insert_asset(Path::new(PATH), initial.clone());
        let source = assets.clone();
        let mut app = App::new();
        app.register_asset_source(AssetSourceId::Default, AssetSourceBuilder::new(move || {
            Box::new(MemoryAssetReader { root: source.clone() })
        }))
        .add_plugins((TaskPoolPlugin::default(), AssetPlugin {
            watch_for_changes_override: Some(false), ..Default::default()
        }))
        .init_asset::<ScenePlacementsAsset>()
        .init_asset_loader::<ScenePlacementsLoader>();

        let server = app.world().resource::<AssetServer>().clone();
        let handle = server.load::<ScenePlacementsAsset>(PATH);
        fn read_after(app: &mut App, handle: &Handle<ScenePlacementsAsset>, x: f32) -> ScenePlacementsAsset {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                app.update();
                if let Some(record) = app.world().resource::<Assets<ScenePlacementsAsset>>().get(handle)
                    && record.placements[0].translation[0] == x {
                    return record.clone();
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            panic!("等待原摆放加载器采用 {x} 超时");
        }
        let loaded = read_after(&mut app, &handle, 0.);
        assert_eq!(loaded.placements, records.placements);

        records.placements[0].translation[0] = 2.;
        let changed = crate::write_scene_placements_glb(&records, PATH, Some(&initial)).unwrap();
        assets.insert_asset(Path::new(PATH), changed);
        // 向真实 AssetServer 传入与 Windows 监听事件相同的 PathBuf，必须采用新值且保留规范资产身份。
        server.reload(native_path);
        let reloaded = read_after(&mut app, &handle, 2.);
        assert_eq!(reloaded.placements[0].id, "bridge");
        assert_eq!(reloaded.placements[0].asset_path, PATH);
        assert_eq!(reloaded.placements, records.placements);
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
