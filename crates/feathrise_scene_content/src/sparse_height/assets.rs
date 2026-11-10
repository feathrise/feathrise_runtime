//! 稀疏高度的 Bevy 资产入口：发布版只通过 AssetServer 读取正式资产源。
//!
//! 加载器在资产 IO 任务中限制字节数、验证路径身份和内容；主世界只接收完整资产。

use std::io::{Error, ErrorKind}; // 坏作者文件以 AssetLoadFailedEvent 返回明确原因。
use std::path::Path; // LoadContext 提供的是相对资产路径。
use std::sync::Arc; // 运行时碰撞和GPU页任务共享只读页，不在主线程深拷贝约264KiB。

use bevy::asset::{AssetLoader, LoadContext, io::Reader}; // 与现有场景资产保持同一异步加载机制。
use bevy::ecs::error::BevyError; // 统一承载 IO、路径和格式错误。
use bevy::prelude::*; // Asset 与 TypePath 派生。

use super::spec::{SPARSE_HEIGHT_MAX_MIP_LEVEL, SparseHeightTileKey}; // 文件名块坐标、层级上限与二进制头共享身份。
use super::tile::{
    SparseHeightManifest, SparseHeightMipTileRef, SparseHeightTile, SparseHeightTileRef,
    maximum_tile_file_bytes, valid_sha256,
}; // 清单和受检单块。
use crate::SceneId; // 场景目录标识不能包含路径语义。
use crate::authoring_io::bytes_hash; // 逐文件 SHA-256 与作者内容地址相同。
use crate::loaders::read_bounded; // 现有资产 IO 的有界分块读取。

const MAX_MANIFEST_BYTES: usize = 32 * 1024 * 1024; // 允许 65536 个非空块，但不能读取无界 JSON。

/// 已验证结构、场景 ID 和资产路径的稀疏清单资产。
#[derive(Asset, TypePath, Clone, Debug)]
pub struct SparseHeightManifestAsset {
    pub manifest: SparseHeightManifest, // 唯一持久作者索引，空白块缺席。
}

/// 已验证路径哈希、文件头和局部尺寸的二进制高度块资产。
#[derive(Asset, TypePath, Clone, Debug)]
pub struct SparseHeightTileAsset {
    pub scene_id: SceneId,               // 从受检资产路径取得的场景身份。
    pub terrain_id: Option<crate::TerrainId>, // 同图相同块号不能被另一块地形误用。
    pub mip_level: u8,               // 零是作者页，其余是清单声明的派生父页。
    pub sha256: String,              // 文件名与真实内容一致的摘要。
    pub tile: Arc<SparseHeightTile>, // 最多 257 × 257 个世界固定格点，多消费者只克隆 Arc。
}

impl SparseHeightTileAsset {
    /// 运行时统一页表可用同一入口核对基础作者页和任意派生父页。
    pub fn validate_page_against(
        &self,
        manifest: &SparseHeightManifest,
        level: u8,
        key: SparseHeightTileKey,
        sha256: &str,
    ) -> Result<(), String> {
        let indexed_hash = if level == 0 {
            manifest
                .tile_ref(key)
                .map(|reference| reference.sha256.as_str())
        } else {
            manifest
                .mip_tile_ref(level, key)
                .map(|reference| reference.sha256.as_str())
        };
        if self.scene_id != manifest.scene_id || self.terrain_id != manifest.terrain_id
            || self.mip_level != level
            || self.tile.key != key
            || self.sha256 != sha256
            || indexed_hash != Some(sha256)
            || manifest.spec.mip_tile_shape(level, key) != Some(self.tile.shape)
        {
            return Err("高度页与当前清单场景、层级、摘要或边缘形状不一致".into());
        }
        Ok(())
    }

    /// 清单和块可异步独立到达，正式采用前必须再次绑定同一版本与末块形状。
    pub fn validate_against(
        &self,
        manifest: &SparseHeightManifest, // 当前已采用清单，而非旧场景清单。
        reference: &SparseHeightTileRef, // 此次需求冻结的块引用。
    ) -> Result<(), String> {
        if self.scene_id != manifest.scene_id || self.terrain_id != manifest.terrain_id
            || self.mip_level != 0
            || self.tile.key != reference.key
            || self.sha256 != reference.sha256
            || manifest.tile_ref(reference.key) != Some(reference)
            || manifest.spec.tile_shape(reference.key) != Some(self.tile.shape)
        {
            return Err("高度块与当前清单场景、摘要或边缘形状不一致".into());
        }
        Ok(())
    }

    /// 派生父页采用前还要核对层级、摘要和当前层尾页形状。
    pub fn validate_mip_against(
        &self,
        manifest: &SparseHeightManifest,
        reference: &SparseHeightMipTileRef,
    ) -> Result<(), String> {
        if self.scene_id != manifest.scene_id || self.terrain_id != manifest.terrain_id
            || self.mip_level != reference.level
            || self.tile.key != reference.key
            || self.sha256 != reference.sha256
            || manifest.mip_tile_ref(reference.level, reference.key) != Some(reference)
            || manifest.spec.mip_tile_shape(reference.level, reference.key) != Some(self.tile.shape)
        {
            return Err("高度 Mip 页与当前清单场景、层级、摘要或边缘形状不一致".into());
        }
        Ok(())
    }
}

/// 新场景专用 `.scene_height_manifest.json` 资产加载器。
#[derive(Default, TypePath)]
pub(crate) struct SparseHeightManifestLoader;

impl AssetLoader for SparseHeightManifestLoader {
    type Asset = SparseHeightManifestAsset;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,    // AssetServer 当前源的异步 Reader。
        _settings: &Self::Settings, // 版本和限额固定在作者格式中，不允许外部放宽。
        load_context: &mut LoadContext<'_>, // 路径用于验证场景目录身份。
    ) -> Result<Self::Asset, Self::Error> {
        let path_scene_id = parse_manifest_asset_path(load_context.path().path())?;
        let (_, terrain_id, _) = parse_source_asset_path(load_context.path().path())?;
        let bytes = read_bounded(reader, MAX_MANIFEST_BYTES, "稀疏高度清单").await?;
        let json = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes); // 与作者 IO 一样接受 UTF-8 BOM。
        let manifest: SparseHeightManifest = serde_json::from_slice(json)?;
        manifest.validate().map_err(invalid_data)?;
        if manifest.scene_id != path_scene_id || manifest.terrain_id != terrain_id {
            return Err(invalid_data("高度清单场景 ID 与资产目录不一致").into());
        }
        Ok(SparseHeightManifestAsset { manifest }) // 完整验证后一次性交给资产世界。
    }

    fn extensions(&self) -> &[&str] {
        &["scene_height_manifest.json"] // 与普通 scene_manifest.json 不争夺扩展名。
    }
}

/// 新场景专用 `.r16tile` 内容寻址二进制块加载器。
#[derive(Default, TypePath)]
pub(crate) struct SparseHeightTileLoader;

impl AssetLoader for SparseHeightTileLoader {
    type Asset = SparseHeightTileAsset;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,            // 单个二进制高度块的资产 Reader。
        _settings: &Self::Settings,         // 不允许调用者跳过摘要和边界校验。
        load_context: &mut LoadContext<'_>, // 文件名提供场景、块和内容哈希身份。
    ) -> Result<Self::Asset, Self::Error> {
        let (scene_id, mip_level, key, sha256) = parse_tile_asset_path(load_context.path().path())?;
        let (_, terrain_id, _) = parse_source_asset_path(load_context.path().path())?;
        let bytes = read_bounded(reader, maximum_tile_file_bytes(), "稀疏高度块").await?;
        if bytes_hash(&bytes) != sha256 {
            return Err(invalid_data("高度块文件名摘要与内容不一致").into());
        }
        let tile = SparseHeightTile::decode_asset_bytes(key, &bytes).map_err(invalid_data)?;
        Ok(SparseHeightTileAsset {
            scene_id,
            terrain_id,
            mip_level,
            sha256,
            tile: Arc::new(tile),
        }) // 清单绑定在使用该块时再核对。
    }

    fn extensions(&self) -> &[&str] {
        &["r16tile"] // 保留旧内容地址后缀；头部v2/v3决定u16或浮点，不能交给Image解码器。
    }
}

fn invalid_data(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidData, message.into()) // Bevy 将其转成资产加载失败事件。
}

/// 清单路径固定为 scenes/<安全 ID>/terrain/height_tiles.scene_height_manifest.json。
fn parse_manifest_asset_path(path: &Path) -> Result<SceneId, Error> {
    let (scene_id, _, suffix) = parse_source_asset_path(path)?;
    if !super::valid_manifest_suffix(&suffix) {
        return Err(invalid_data("稀疏高度清单资产路径无效"));
    }
    Ok(scene_id)
}

/// 只接受旧主地形或明确的独立对象目录；不允许自由相对路径绕过地形身份。
fn parse_source_asset_path(path: &Path) -> Result<(SceneId, Option<crate::TerrainId>, String), Error> {
    let portable = path.to_string_lossy().replace('\\', "/");
    let parts = portable.split('/').collect::<Vec<_>>();
    if parts.len() < 4 || parts[0] != "scenes" { return Err(invalid_data("高度源路径无效")); }
    let scene_id = SceneId::parse(parts[1]).map_err(invalid_data)?;
    let (terrain_id, offset) = match parts[2] {
        "terrain" => (None, 3),
        "terrains" if parts.len() >= 5 => (Some(crate::TerrainId::parse(parts[3]).map_err(invalid_data)?), 4),
        _ => return Err(invalid_data("高度源目录无效")),
    };
    Ok((scene_id, terrain_id, parts[offset..].join("/")))
}

/// 块文件名只能由受检块坐标与 64 位小写哈希生成。
fn parse_tile_asset_path(path: &Path) -> Result<(SceneId, u8, SparseHeightTileKey, String), Error> {
    let (scene_id, _, suffix) = parse_source_asset_path(path)?;
    let parts = suffix.split('/').collect::<Vec<_>>();
    let (mip_level, file_name) = match parts.as_slice() {
        ["height_tiles", file_name] => (0_u8, *file_name),
        ["height_mips", level, file_name]
            if level.len() == 2
                && level.bytes().all(|byte| byte.is_ascii_digit())
                && level
                    .parse::<u8>()
                    .is_ok_and(|level| (1..=SPARSE_HEIGHT_MAX_MIP_LEVEL).contains(&level)) =>
        {
            (level.parse::<u8>().expect("守卫已验证层级"), *file_name)
        }
        _ => return Err(invalid_data("稀疏高度块资产路径无效")),
    };
    let stem = file_name
        .strip_suffix(".r16tile")
        .ok_or_else(|| invalid_data("高度块扩展名无效"))?;
    let fields = stem.split('_').collect::<Vec<_>>();
    if fields.len() != 3 || !valid_sha256(fields[2]) {
        return Err(invalid_data("高度块文件名缺少受检 SHA-256"));
    }
    let x = fields[0]
        .parse::<u32>()
        .map_err(|_| invalid_data("高度块 X 坐标无效"))?;
    let z = fields[1]
        .parse::<u32>()
        .map_err(|_| invalid_data("高度块 Z 坐标无效"))?;
    if x >= 256 || z >= 256 || fields[0] != format!("{x:04}") || fields[1] != format!("{z:04}") {
        return Err(invalid_data("高度块文件名坐标超出版本一场景预算"));
    }
    Ok((
        scene_id,
        mip_level,
        SparseHeightTileKey { x, z },
        fields[2].to_owned(),
    ))
}

#[cfg(test)] // 路径解析和清单绑定无需启动图形 App 即可验证。
mod tests {
    use super::*;
    use crate::SparseHeightSpec;

    #[test]
    fn loader_paths_require_safe_scene_identity_and_canonical_tile_name() {
        let map = parse_manifest_asset_path(Path::new(
            "scenes/MAP-safe/terrain/height_tiles.scene_height_manifest.json",
        ))
        .unwrap();
        assert_eq!(map.as_str(), "MAP-safe");
        assert!(
            parse_manifest_asset_path(Path::new(
                "scenes/../terrain/height_tiles.scene_height_manifest.json"
            ))
            .is_err()
        );
        let digest = "a".repeat(64);
        let path = format!("scenes/MAP-safe/terrain/height_tiles/0001_0255_{digest}.r16tile");
        let (map, level, key, hash) = parse_tile_asset_path(Path::new(&path)).unwrap();
        assert_eq!(map.as_str(), "MAP-safe");
        assert_eq!(level, 0);
        assert_eq!(key, SparseHeightTileKey { x: 1, z: 255 });
        assert_eq!(hash, digest);
        let mip_path = format!("scenes/MAP-safe/terrain/height_mips/08/0000_0000_{digest}.r16tile");
        let (_, level, key, _) = parse_tile_asset_path(Path::new(&mip_path)).unwrap();
        assert_eq!(level, 8);
        assert_eq!(key, SparseHeightTileKey { x: 0, z: 0 });
        assert!(parse_tile_asset_path(Path::new(&mip_path.replace("/08/", "/09/"))).is_err());
        assert!(parse_tile_asset_path(Path::new(&path.replace("0255", "0256"))).is_err());
        assert!(parse_tile_asset_path(Path::new(&path.replace(&digest, &"A".repeat(64)))).is_err());
    }

    #[test]
    fn tile_asset_must_match_current_manifest_and_edge_shape() {
        let scene_id = SceneId::parse("MAP-safe").unwrap();
        let spec =
            SparseHeightSpec::new([-32.0, -32.0], [32.0, 32.0], 0.125, -80.0, 160.0).unwrap();
        let key = SparseHeightTileKey { x: 1, z: 1 }; // 512 格场景的尾块应包含末端格点。
        let tile = SparseHeightTile::blank(&spec, key).unwrap();
        let reference = SparseHeightTileRef {
            key,
            sha256: tile.sha256(),
        };
        let manifest = SparseHeightManifest {
            terrain_id: None,
            schema_version: super::super::SPARSE_HEIGHT_SCHEMA_VERSION,
            scene_id: scene_id.clone(),
            spec,
            height_bounds: None,
            tiles: vec![reference.clone()],
            mip_tiles: Vec::new(),
        };
        let asset = SparseHeightTileAsset {
            terrain_id: None,
            scene_id,
            mip_level: 0,
            sha256: reference.sha256.clone(),
            tile: Arc::new(tile.clone()),
        };
        assert!(asset.validate_against(&manifest, &reference).is_ok());
        let mut wrong = asset.clone();
        Arc::make_mut(&mut wrong.tile).shape = [256, 256]; // 文件本身可能有界，但不能冒充当前清单的场景尾块。
        assert!(wrong.validate_against(&manifest, &reference).is_err());
        let mut other_reference = reference.clone();
        other_reference.sha256 = "b".repeat(64); // 同块旧修订不等于当前清单采用版本。
        assert!(asset.validate_against(&manifest, &other_reference).is_err());
    }
}
