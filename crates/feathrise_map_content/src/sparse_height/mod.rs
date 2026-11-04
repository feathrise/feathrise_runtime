//! 新地图稀疏高度作者源：清单、唯一格点归属、按需块 IO 与原子保存。
//!
//! 这里不引用 F12、ECS 或渲染器；同步 IO 接口应由作者后台任务调用。

mod assets; // Bevy AssetServer 从正式资产源异步读取清单和内容寻址块。
mod author; // F12 与未来独立编辑器共用的有限笔刷、页缓存和撤销核心。
mod document; // 按需加载、纯内存采样和版本受检的脏块保存。
mod edit_types; // 局部后台候选和有限页原子采用/回滚，不复制整个作者会话。
mod live; // F12 未保存局部块通过共享 Arc 快照实时交给场景页与碰撞消费者。
mod session; // 技能造成的会话级浅坑与F12作者数据完全隔离。
mod spec; // 世界尺寸、作者精度、全局格点与块坐标的唯一规则。
mod tile; // 单块二进制格式和内容哈希身份。

pub use assets::{SparseHeightManifestAsset, SparseHeightTileAsset}; // 运行时只持有已校验的资产句柄。
pub(crate) use assets::{SparseHeightManifestLoader, SparseHeightTileLoader}; // 加载器由 MapContentPlugin 注册。
pub use author::{
    MAX_SPARSE_AUTHOR_CLEAN_TILES, MAX_SPARSE_BRUSH_CANDIDATES, MAX_SPARSE_PREPARED_EDIT_TILES,
    SparseTerrainAuthorSession, SparseTerrainBrushInput, SparseTerrainBrushKind,
    SparseTerrainBrushResult, SparseTerrainHeightChange,
}; // 纯作者核心不携带 F12、ECS 或渲染状态，可由独立编辑器直接复用。
pub use document::SparseHeightDocument; // 游戏内与独立编辑器共用的作者会话。
pub use edit_types::{
    SparseTerrainAppliedEdit, SparseTerrainEditRollback, SparseTerrainPreparedEdit,
};
pub use live::{SparseHeightLiveOverride, SparseHeightLiveSnapshot}; // 实时覆盖和后台持久根快照严格绑定地图与作者修订。
pub use session::{
    SparseCraterTiles, SparseHeightCraterRequest, SparseHeightSessionCrater, sample_crater_height,
}; // 技能与渲染/碰撞共享固定世界格点的局部会话覆盖。
pub use spec::{
    SPARSE_HEIGHT_MAX_MIP_LEVEL, SPARSE_HEIGHT_TILE_CELLS, SparseHeightSpec, SparseHeightTileKey,
}; // 格点、页层级与块布局合同。
pub use tile::{
    SparseHeightManifest, SparseHeightMipTileRef, SparseHeightTile, SparseHeightTileRef,
}; // 持久数据和按需解码。

use crate::MapId; // 安全地图目录身份由公共 MapId 验证。

/// 清单和二进制块的当前格式版本。
pub const SPARSE_HEIGHT_SCHEMA_VERSION: u32 = 3;

/// 新格式清单相对 assets 的固定路径，供地图总清单的 height_path 引用。
pub fn sparse_height_manifest_asset_path(map_id: &MapId) -> String {
    format!("maps/{map_id}/terrain/height_tiles.map_height_manifest.json") // 旧 GLB 作者路径不参与该清单。
}

/// 地图加载、编辑和文件交接共用同一闭合路径合同。
pub fn sparse_height_manifest_terrain_id(map_id: &MapId, path: &str) -> Result<Option<crate::TerrainId>, String> {
    if let Some(suffix) = path.strip_prefix(&format!("maps/{map_id}/terrain/")) {
        return valid_manifest_suffix(suffix).then_some(None).ok_or("主地形清单版本路径无效".into());
    }
    let prefix = format!("maps/{map_id}/terrains/");
    let (id, suffix) = path.strip_prefix(&prefix).and_then(|tail| tail.split_once('/'))
        .ok_or("高度作者路径不属于当前地图的主地形或独立对象")?;
    if !valid_manifest_suffix(suffix) { return Err("独立地形清单版本路径无效".into()); }
    Ok(Some(crate::TerrainId::parse(id)?))
}

/// 不可变作者版本只改变清单，所有版本继续共享同一对象的内容地址块。
pub(crate) fn valid_manifest_suffix(suffix: &str) -> bool {
    suffix == "height_tiles.map_height_manifest.json" || suffix.strip_prefix("versions/")
        .and_then(|file| file.strip_suffix(".map_height_manifest.json"))
        .is_some_and(|name| crate::TerrainId::parse(name).is_ok())
}

/// 内容寻址块的固定资产路径；文件名不会从外部 JSON 直接执行路径拼接。
pub fn sparse_height_tile_asset_path(
    map_id: &MapId,           // 块所属地图。
    key: SparseHeightTileKey, // 世界固定块坐标。
    sha256: &str,             // 已经验证为小写 64 位十六进制内容摘要。
) -> String {
    format!(
        "maps/{map_id}/terrain/height_tiles/{:04}_{:04}_{sha256}.r16tile",
        key.x, key.z
    ) // 哈希文件只增不改，旧清单读者仍可读取原版本。
}

/// 派生父页按层级分目录；文件仍是同一受检 R16 页格式且只增不改。
pub fn sparse_height_mip_tile_asset_path(
    map_id: &MapId,           // 页所属地图。
    level: u8,                // 一至当前规格最高 Mip。
    key: SparseHeightTileKey, // 当前层世界固定页坐标。
    sha256: &str,             // 当前派生页的内容摘要。
) -> String {
    format!(
        "maps/{map_id}/terrain/height_mips/{level:02}/{:04}_{:04}_{sha256}.r16tile",
        key.x, key.z
    ) // 层级进入受控路径，不能由清单提供自由字符串。
}

impl SparseHeightManifest {
    pub fn source_folder(&self) -> String {
        self.terrain_id.as_ref().map_or_else(|| format!("maps/{}/terrain", self.map_id),
            |id| format!("maps/{}/terrains/{}", self.map_id, id.as_str()))
    }
    pub fn asset_path(&self) -> String {
        format!("{}/height_tiles.map_height_manifest.json", self.source_folder())
    }
    pub fn tile_asset_path(&self, key: SparseHeightTileKey, sha256: &str) -> String {
        format!("{}/height_tiles/{:04}_{:04}_{sha256}.r16tile", self.source_folder(), key.x, key.z)
    }
    pub fn mip_tile_asset_path(&self, level: u8, key: SparseHeightTileKey, sha256: &str) -> String {
        format!("{}/height_mips/{level:02}/{:04}_{:04}_{sha256}.r16tile", self.source_folder(), key.x, key.z)
    }
}

#[cfg(test)] // 跨尺寸、边界归属、持久保存与版本冲突均使用同一公共 API。
mod tests;
