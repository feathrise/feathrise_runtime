//! 内容寻址的局部米制高度页；v3保存f32位模式，旧v2的u16页保持按源清单兼容读取。

use serde::{Deserialize, Serialize}; // 清单可由游戏内和独立作者工具共同解析。

use super::SPARSE_HEIGHT_SCHEMA_VERSION; // 清单与块文件同步升级格式。
use super::spec::{SparseHeightSpec, SparseHeightTileKey}; // 二进制尺寸必须由同一世界格点规则给出。
use crate::MapId; // 防止把别的地图清单误装入当前地图。
use crate::authoring_io::bytes_hash; // 块文件内容身份与项目既有 SHA-256 规则一致。

const TILE_MAGIC: [u8; 8] = *b"B5HTILE\0"; // 防止任意二进制被误读为高度块。
const TILE_HEADER_BYTES: usize = 24; // 魔数、版本、两轴块号和两轴拥有的格点数。
const LEGACY_TILE_VERSION: u32 = 2;
const MAX_TILE_BYTES: usize = TILE_HEADER_BYTES + 257 * 257 * 4; // v3每页至多约264KiB，IO仍先验证固定页预算。

/// 一条清单索引只指向不可变内容哈希，不直接信任外部路径字符串。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SparseHeightTileRef {
    pub key: SparseHeightTileKey, // 世界固定块身份。
    pub sha256: String,           // 完整块文件的 64 位小写十六进制 SHA-256。
}

/// 一个可重建父页的内容地址；层级零仍只由 `tiles` 保存，避免两套基础作者索引。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SparseHeightMipTileRef {
    pub level: u8,                // 一表示二倍格距，最高由地图规格确定。
    pub key: SparseHeightTileKey, // 当前层世界固定页坐标。
    pub sha256: String,           // 当前层不可变二进制页的完整内容摘要。
}

/// 稀疏作者高度的唯一版本化权威清单。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SparseHeightManifest {
    pub schema_version: u32,                    // 新写v3；未触及的v2内容地址页仍可在新清单中复用。
    pub map_id: MapId, // 防止用户把另一地图文件复制到当前目录后被静默采用。
    /// 同地图的高度块按地形隔离；缺省保留旧单地形路径和文件内容。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terrain_id: Option<crate::TerrainId>,
    pub spec: SparseHeightSpec, // 地图范围、作者格距和高度量程。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_bounds: Option<[f32; 2]>, // 当前已保存实际高度的保守范围；旧清单缺省沿用原编码域。
    pub tiles: Vec<SparseHeightTileRef>, // 按 key 严格升序的非平地作者块。
    pub mip_tiles: Vec<SparseHeightMipTileRef>, // 按层级和 key 严格升序的非平地派生父页。
}

impl SparseHeightManifest {
    /// 空白新图只需一个小清单，零高度块不落盘。
    pub fn empty(map_id: MapId, spec: SparseHeightSpec) -> Result<Self, String> {
        crate::MapId::parse(map_id.as_str())?; // 反序列化的 MapId 同样要接受安全校验。
        spec.validate()?; // 格距与世界范围属于权威文件，不可相信调用者直接构造字段。
        Ok(Self {
            schema_version: SPARSE_HEIGHT_SCHEMA_VERSION, // 持久格式身份。
            map_id,                                       // 和地图总清单使用同一个目录 ID。
            terrain_id: None,
            spec,                                         // 不复制任何格点或块数组。
            height_bounds: Some([0.0; 2]),                 // 空白源精确为零，不把旧兼容量程当作雕刻边界。
            tiles: Vec::new(),                            // 全图平地由缺席块隐式表达。
            mip_tiles: Vec::new(),                        // 空白图的所有父页也隐式为世界 Y=0。
        })
    }

    /// 加载和保存前严格验证引用顺序、范围及内容摘要语法。
    pub fn validate(&self) -> Result<(), String> {
        if ![LEGACY_TILE_VERSION, SPARSE_HEIGHT_SCHEMA_VERSION].contains(&self.schema_version) {
            return Err(format!("不支持稀疏高度清单版本 {}", self.schema_version));
        }
        crate::MapId::parse(self.map_id.as_str())?;
        if let Some(id) = &self.terrain_id { crate::TerrainId::parse(id.as_str())?; }
        self.spec.validate()?;
        if self.height_bounds.is_some_and(|bounds| bounds.iter().any(|height| !height.is_finite())
            || bounds[0] > 0.0 || bounds[1] < 0.0 || bounds[0] > bounds[1]) {
            return Err("稀疏高度清单的实际高度范围必须有限、有序且包含隐式零平地".into());
        }
        let mut previous = None; // 严格升序同时阻止相同块出现两次。
        for tile in &self.tiles {
            if self.spec.tile_shape(tile.key).is_none()
                || previous.is_some_and(|key| key >= tile.key)
                || !valid_sha256(&tile.sha256)
            {
                return Err(format!(
                    "稀疏高度块索引无效：({}, {})",
                    tile.key.x, tile.key.z
                ));
            }
            previous = Some(tile.key);
        }
        let mut previous_mip = None; // `(level, key)` 严格升序同时阻止父页重复。
        for tile in &self.mip_tiles {
            let identity = (tile.level, tile.key);
            if tile.level == 0
                || tile.level > self.spec.max_mip_level()
                || self.spec.mip_tile_shape(tile.level, tile.key).is_none()
                || previous_mip.is_some_and(|previous| previous >= identity)
                || !valid_sha256(&tile.sha256)
            {
                return Err(format!(
                    "稀疏高度 Mip 页索引无效：L{} ({}, {})",
                    tile.level, tile.key.x, tile.key.z
                ));
            }
            previous_mip = Some(identity);
        }
        Ok(())
    }

    /// 二分索引使空白大图查询不扫描或构造完整块目录。
    pub fn tile_ref(&self, key: SparseHeightTileKey) -> Option<&SparseHeightTileRef> {
        self.tiles
            .binary_search_by_key(&key, |entry| entry.key)
            .ok()
            .map(|index| &self.tiles[index])
    }

    /// 小型范围元数据可在主帧读取；旧v2页无需为求包围盒扫描或迁移整图。
    pub fn height_bounds(&self) -> [f32; 2] {
        self.height_bounds.unwrap_or([self.spec.base_height.min(0.0),
            (self.spec.base_height + self.spec.height_scale).max(0.0)])
    }

    /// 二分取得一个正式派生父页；缺席仍表示这一整页是世界 Y=0。
    pub fn mip_tile_ref(
        &self,
        level: u8,
        key: SparseHeightTileKey,
    ) -> Option<&SparseHeightMipTileRef> {
        self.mip_tiles
            .binary_search_by_key(&(level, key), |entry| (entry.level, entry.key))
            .ok()
            .map(|index| &self.mip_tiles[index])
    }
}

/// 一个已载入或正在编辑的局部作者块，边界格点只在拥有者块内保存。
#[derive(Clone, Debug)]
pub struct SparseHeightTile {
    pub key: SparseHeightTileKey, // 全局作者块身份。
    pub shape: [u16; 2],          // 当前块实际拥有的格点数，尾块可能较小。
    heights: Vec<u32>,           // v3为f32米制位模式；v2资产暂存原u16码，统一读取时绑定真实源spec。
    legacy_u16: bool,            // AssetLoader没有源量程，不能在加载帧猜默认域转换旧页。
    bounds: [u32; 2],            // IO/作者后台维护的极值；旧页缓存原码，新页缓存米制bits，查询O(1)。
}

impl PartialEq for SparseHeightTile {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.shape == other.shape && self.legacy_u16 == other.legacy_u16
            && self.heights == other.heights // 保守范围缓存不是作者内容身份，重载后缩界不改变页值。
    }
}
impl Eq for SparseHeightTile {}

impl SparseHeightTile {
    /// 按合法规格只创建一个块；隐式平地不调用这个分配路径。
    pub fn blank(spec: &SparseHeightSpec, key: SparseHeightTileKey) -> Result<Self, String> {
        Self::blank_at_level(spec, 0, key)
    }

    /// 创建一个基础作者页或派生父页；所有层都保持最多 257×257 个格点。
    pub fn blank_at_level(
        spec: &SparseHeightSpec,
        level: u8,
        key: SparseHeightTileKey,
    ) -> Result<Self, String> {
        let shape = spec
            .mip_tile_shape(level, key)
            .ok_or("高度页层级或坐标超出地图范围")?;
        let count = usize::from(shape[0]) * usize::from(shape[1]);
        Ok(Self {
            key,                                           // 与清单索引及二进制头必须完全一致。
            shape,                                         // 共享边界只在唯一拥有者块出现。
            heights: vec![spec.flat_height_code(); count], // 一块至多约264KiB，不物化整图。
            legacy_u16: false,
            bounds: [0; 2],
        })
    }

    /// 只读已绑定米制的作者页；旧v2资产尚未知原量程，运行时应改用get_height。
    pub fn get(&self, local: [u16; 2]) -> Option<u32> {
        (!self.legacy_u16).then(|| self.index(local).map(|index| self.heights[index])).flatten()
    }

    /// 所有运行时消费者使用本入口；旧页在原来源量程下逐点解码，绝不主帧转换完整页。
    pub fn get_height(&self, spec: &SparseHeightSpec, local: [u16; 2]) -> Option<u32> {
        self.index(local).map(|index| self.height_code(spec, self.heights[index]))
    }

    /// 完整页遍历只允许原后台worker使用；迭代不分配另一份整页数组。
    pub fn height_codes<'a>(&'a self, spec: &'a SparseHeightSpec) -> impl ExactSizeIterator<Item = u32> + 'a {
        self.heights.iter().map(move |code| self.height_code(spec, *code))
    }

    pub fn memory_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.heights.capacity() * std::mem::size_of::<u32>()
    }

    pub fn height_bounds(&self, spec: &SparseHeightSpec) -> [f32; 2] {
        self.bounds.map(|code| spec.decode_height(self.height_code(spec, code)))
    }

    /// 已受检米制高度的缓存边界；旧U16文件尚未绑定清单范围时返回None，实时发布不能猜测。
    pub fn meter_bounds(&self) -> Option<[f32; 2]> {
        (!self.legacy_u16).then(|| self.bounds.map(f32::from_bits))
    }

    fn height_code(&self, spec: &SparseHeightSpec, code: u32) -> u32 {
        if self.legacy_u16 { spec.decode_legacy_height(code as u16).to_bits() } else { code }
    }

    /// 返回此前编码，供作者命令生成精确撤销差分。
    pub fn set(&mut self, local: [u16; 2], height: u32) -> Result<u32, String> {
        if self.legacy_u16 || !valid_height_code(height) {
            return Err("旧高度页须先在后台绑定源量程；新作者高度必须有限且使用规范零值".into());
        }
        let index = self.index(local).ok_or("块内格点坐标超出拥有范围")?;
        let meters = f32::from_bits(height);
        self.bounds = [f32::from_bits(self.bounds[0]).min(meters).to_bits(),
            f32::from_bits(self.bounds[1]).max(meters).to_bits()]; // 保守扩界，无需每次写格点重扫全页。
        Ok(std::mem::replace(&mut self.heights[index], height))
    }

    /// 回到隐式平地后，下一次保存可从清单移除此块。
    pub fn is_flat(&self, spec: &SparseHeightSpec) -> bool {
        self.height_codes(spec).all(|value| value == spec.flat_height_code())
    }

    /// 完整二进制内容用于 SHA-256 身份与测试；单块大小始终受格式上限约束。
    pub fn encode_bytes(&self) -> Vec<u8> {
        let bytes_per_sample = if self.legacy_u16 { 2 } else { 4 };
        let mut bytes = Vec::with_capacity(TILE_HEADER_BYTES + self.heights.len() * bytes_per_sample);
        bytes.extend_from_slice(&TILE_MAGIC); // 文件种类。
        bytes.extend_from_slice(&(if self.legacy_u16 { LEGACY_TILE_VERSION } else { SPARSE_HEIGHT_SCHEMA_VERSION }).to_le_bytes());
        bytes.extend_from_slice(&self.key.x.to_le_bytes()); // 块列。
        bytes.extend_from_slice(&self.key.z.to_le_bytes()); // 块行。
        bytes.extend_from_slice(&self.shape[0].to_le_bytes()); // 拥有的 X 格点数。
        bytes.extend_from_slice(&self.shape[1].to_le_bytes()); // 拥有的 Z 格点数。
        for height in &self.heights {
            if self.legacy_u16 { bytes.extend_from_slice(&(*height as u16).to_le_bytes()); }
            else { bytes.extend_from_slice(&height.to_le_bytes()); } // f32的位模式按小端保存，已有米制值不再量化。
        }
        bytes
    }

    /// 仅接受清单指定块、指定形状和恰好长度的文件。
    pub fn decode_bytes(
        spec: &SparseHeightSpec,  // 清单中的权威规格。
        key: SparseHeightTileKey, // 清单中的权威块身份。
        bytes: &[u8],             // 已按最大块长度限制读取的候选内容。
    ) -> Result<Self, String> {
        Self::decode_bytes_at_level(spec, 0, key, bytes)
    }

    /// 用当前层的权威边缘形状解码内容地址页；二进制头仍只保存页内坐标和形状。
    pub fn decode_bytes_at_level(
        spec: &SparseHeightSpec,
        level: u8,
        key: SparseHeightTileKey,
        bytes: &[u8],
    ) -> Result<Self, String> {
        let shape = spec
            .mip_tile_shape(level, key)
            .ok_or("高度页层级或坐标超出地图范围")?;
        let mut tile = Self::decode_asset_bytes(key, bytes)?; // 先执行无需清单的资产文件完整校验。
        if tile.shape != shape {
            return Err("高度页形状与清单规格或 Mip 层级不一致".into());
        }
        if tile.legacy_u16 {
            // 作者IO只转换实际加载页；未编辑页继续复用原v2哈希，不落盘、不全图迁移。
            let bounds = tile.height_bounds(spec);
            for value in &mut tile.heights { *value = spec.decode_legacy_height(*value as u16).to_bits(); }
            tile.legacy_u16 = false;
            tile.bounds = bounds.map(f32::to_bits);
        }
        Ok(tile)
    }

    /// AssetServer 未持有清单时也能受检解析一个有界块；采用前再比对清单边缘形状。
    pub fn decode_asset_bytes(
        key: SparseHeightTileKey, // 已由文件名解析的世界固定块身份。
        bytes: &[u8],             // 已经按最大块长度限制读取的候选文件。
    ) -> Result<Self, String> {
        if key.x >= 256
            || key.z >= 256
            || bytes.len() < TILE_HEADER_BYTES
            || bytes.len() > MAX_TILE_BYTES
        {
            return Err("高度块超出版本一的坐标或文件长度预算".into());
        }
        let shape = [
            u16::from_le_bytes(bytes[20..22].try_into().expect("已验证头部长度")),
            u16::from_le_bytes(bytes[22..24].try_into().expect("已验证头部长度")),
        ]; // 真正边缘块形状稍后由当前清单再次核对。
        let version = u32::from_le_bytes(bytes[8..12].try_into().expect("已验证头部长度"));
        let legacy_u16 = version == LEGACY_TILE_VERSION;
        let bytes_per_sample = if legacy_u16 { 2 } else { 4 };
        let required = TILE_HEADER_BYTES + usize::from(shape[0]) * usize::from(shape[1]) * bytes_per_sample;
        if shape.iter().any(|axis| !(1..=257).contains(axis)) || bytes.len() != required {
            return Err("高度块格点形状或文件长度无效".into());
        }
        if bytes[..8] != TILE_MAGIC
            || ![LEGACY_TILE_VERSION, SPARSE_HEIGHT_SCHEMA_VERSION].contains(&version)
            || u32::from_le_bytes(bytes[12..16].try_into().expect("已验证头部长度")) != key.x
            || u32::from_le_bytes(bytes[16..20].try_into().expect("已验证头部长度")) != key.z
        {
            return Err("高度块头部与资产文件名身份不一致".into());
        }
        let heights: Vec<u32> = bytes[TILE_HEADER_BYTES..].chunks_exact(bytes_per_sample)
            .map(|sample| if legacy_u16 { u32::from(u16::from_le_bytes(sample.try_into().unwrap())) }
                else { u32::from_le_bytes(sample.try_into().unwrap()) }).collect();
        if !legacy_u16 && heights.iter().any(|code| !valid_height_code(*code)) {
            return Err("浮点高度页包含非有限值或非规范负零".into());
        }
        let bounds = if legacy_u16 {
            [*heights.iter().min().unwrap(), *heights.iter().max().unwrap()]
        } else {
            [heights.iter().map(|code| f32::from_bits(*code)).fold(f32::INFINITY, f32::min).to_bits(),
                heights.iter().map(|code| f32::from_bits(*code)).fold(f32::NEG_INFINITY, f32::max).to_bits()]
        };
        Ok(Self {
            key,     // 与请求引用严格一致。
            shape,   // 按清单重建的唯一格点归属。
            heights, // 经过长度、版本、范围和内容哈希的块数据。
            legacy_u16,
            bounds,
        })
    }

    /// 内容地址保证清单先旧后新期间不会把同一路径改为另一版本。
    pub fn sha256(&self) -> String {
        bytes_hash(&self.encode_bytes()) // 哈希峰值至多一个块文件。
    }

    fn index(&self, local: [u16; 2]) -> Option<usize> {
        (local[0] < self.shape[0] && local[1] < self.shape[1])
            .then(|| usize::from(local[1]) * usize::from(self.shape[0]) + usize::from(local[0]))
    }
}

pub(super) fn valid_height_code(code: u32) -> bool {
    f32::from_bits(code).is_finite() && code != (-0.0_f32).to_bits()
}

/// 文件名摘要严格使用 SHA-256 小写十六进制，防止清单注入任意路径。
pub(super) fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// 块读取的硬上限，防止坏资产导致后台任务不受控分配。
pub(super) const fn maximum_tile_file_bytes() -> usize {
    MAX_TILE_BYTES
}
