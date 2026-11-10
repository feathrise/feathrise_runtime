//! 稀疏高度作者会话：只加载需要的块，按内容地址写脏块并最后发布清单。
//!
//! 所有文件 IO 都是同步函数，调用方必须把创建、载入和保存放进有界后台 IO 任务。

use std::collections::{BTreeMap, BTreeSet}; // 实际加载/修改过的块才进入内存集合。
use std::fs::{File, OpenOptions}; // 按清单哈希读取块，并取得场景级作者写锁。
use std::io::Read; // 单块读取使用 take 防止损坏文件造成无界内存分配。
use std::path::{Path, PathBuf}; // 作者根路径与固定资产相对路径。
use std::sync::Arc; // 未保存实时覆盖与GPU/碰撞任务共享不可变块快照。

use super::spec::{SparseHeightSpec, SparseHeightTileKey}; // 全局格点和世界坐标换算。
use super::tile::{
    SparseHeightManifest, SparseHeightMipTileRef, SparseHeightTile, SparseHeightTileRef,
    maximum_tile_file_bytes,
}; // 作者清单和单块二进制。
use super::{
    sparse_height_manifest_asset_path,

}; // 文件路径不由外部 JSON 自由指定。
use crate::SceneId; // 公开 API 以经过验证的场景目录身份定位。
use crate::authoring_io::{
    bytes_hash, compare_content_hash, file_hash, pretty_json_hash, safe_content_path,
}; // 复用项目路径和并发版本规则。
use crate::authoring_transaction::{PreparedWrite, commit_prepared_writes_checked}; // 同目录 stage、回滚和原子替换。

/// 可在 F12 后台任务或未来独立编辑器中使用的局部高度作者会话。
pub struct SparseHeightDocument {
    asset_root: PathBuf, // canonical assets 根，所有块路径仍经 safe_content_path 校验。
    source_path: String,
    origin_guard: Option<(PathBuf, String)>, // 分支版本保存仍核对原源，不能掩盖并发作者冲突。
    manifest: SparseHeightManifest, // 上次成功发布或加载的唯一权威版本。
    disk_hash: String,   // 保存前比较磁盘原版本，拒绝覆盖另一作者的新清单。
    loaded: BTreeMap<SparseHeightTileKey, SparseHeightTile>, // 只保留调用方按需加载/编辑的块。
    dirty: BTreeSet<SparseHeightTileKey>, // 保存只重写受编辑影响的局部块。
    manifest_dirty: bool, // 新建空白场景必须写出清单，虽然没有任何块文件。
    height_bounds: [f32; 2], // 后台写入时保守扩界；主帧查询不扫描loaded或全图页索引。
}

const MAX_MANIFEST_BYTES: u64 = 32 * 1024 * 1024; // 最多 65536 个作者块清单也不得造成无界读取。

impl SparseHeightDocument {
    /// 新建空白场景只建立清单元数据；调用 save_dirty 才写盘。
    pub fn create_empty(
        asset_root: &Path,      // 已存在的 assets 根目录。
        scene_id: SceneId,          // 新图安全目录 ID。
        spec: SparseHeightSpec, // 可接受最大 8192m 与最细 0.125m 的规格。
    ) -> Result<Self, String> {
        Self::create_for_terrain(asset_root, scene_id, None, spec)
    }

    /// 新对象与旧图共用同一持久化算法，只改变受检地形命名空间。
    pub fn create_for_terrain(asset_root: &Path, scene_id: SceneId, terrain_id: Option<crate::TerrainId>, spec: SparseHeightSpec) -> Result<Self, String> {
        let root = canonical_asset_root(asset_root)?;
        let mut manifest = SparseHeightManifest::empty(scene_id, spec)?;
        manifest.terrain_id = terrain_id;
        manifest.validate()?;
        let manifest_path =
            safe_content_path(&root, &manifest.asset_path())?; // 创建前仅检验目标路径，不创建与全图面积相关的文件。
        if file_hash(&manifest_path)? != "missing" {
            return Err(format!("稀疏地形清单已经存在：{}", manifest_path.display()));
        }
        Ok(Self {
            asset_root: root,            // 后续加载和保存不接受其他目录拼写。
            source_path: manifest.asset_path(),
            origin_guard: None,
            manifest,                    // 空 tiles 即整图世界 Y=0 平地。
            disk_hash: "missing".into(), // 首次保存只允许目标仍不存在。
            loaded: BTreeMap::new(),     // 不为 65537² 格点预分配内存。
            dirty: BTreeSet::new(),      // 空白场景没有块文件。
            manifest_dirty: true,        // 即使没有笔划也必须写清单。
            height_bounds: [0.0; 2],
        })
    }

    /// 载入清单而不载入任何高度块；此函数只在后台 IO 任务调用。
    pub fn load(asset_root: &Path, scene_id: &SceneId) -> Result<Self, String> {
        Self::load_for_terrain(asset_root, scene_id, None)
    }

    pub fn load_for_terrain(asset_root: &Path, scene_id: &SceneId, terrain_id: Option<&crate::TerrainId>) -> Result<Self, String> {
        let source_path = terrain_id.map_or_else(|| sparse_height_manifest_asset_path(scene_id),
            |id| format!("scenes/{scene_id}/terrains/{}/height_tiles.scene_height_manifest.json", id.as_str()));
        Self::load_source(asset_root, scene_id, terrain_id, &source_path)
    }

    /// 版本清单与基础清单使用相同的对象身份和页格式，不把版本目录变成新的块命名空间。
    pub fn load_source(asset_root: &Path, scene_id: &SceneId, terrain_id: Option<&crate::TerrainId>, source_path: &str) -> Result<Self, String> {
        SceneId::parse(scene_id.as_str())?;
        if super::sparse_height_manifest_terrain_id(scene_id, source_path)?.as_ref() != terrain_id { return Err("高度版本路径与地形身份不一致".into()); }
        let root = canonical_asset_root(asset_root)?;
        let path = safe_content_path(&root, &source_path)?;
        let file = File::open(&path)
            .map_err(|error| format!("读取高度清单{}失败：{error}", path.display()))?;
        let mut bytes = Vec::new();
        file.take(MAX_MANIFEST_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("读取高度清单{}失败：{error}", path.display()))?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err("高度清单超过 32 MiB 读取预算".into());
        }
        let json = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes); // 与其他作者 JSON 一样接受 UTF-8 BOM。
        let manifest: SparseHeightManifest = serde_json::from_slice(json)
            .map_err(|error| format!("解析高度清单{}失败：{error}", path.display()))?;
        manifest.validate()?;
        if &manifest.scene_id != scene_id || manifest.terrain_id.as_ref() != terrain_id {
            return Err("稀疏高度清单场景 ID 与当前目录不一致".into());
        }
        let height_bounds = manifest.height_bounds();
        Ok(Self {
            asset_root: root,              // 块加载继续限定在同一个 assets 根。
            source_path: source_path.to_owned(),
            origin_guard: None,
            manifest,                      // 块引用来自已校验清单。
            disk_hash: bytes_hash(&bytes), // 解析与版本哈希来自同一次读取，不存在版本错配窗口。
            loaded: BTreeMap::new(),       // 编辑器打开大场景也不立即读全图块。
            dirty: BTreeSet::new(),        // 刚载入的块全部未修改。
            manifest_dirty: false,         // 未修改会话不得重新写盘。
            height_bounds,
        })
    }

    /// 当前唯一权威清单；GPU 高度页与碰撞可按索引请求局部块。
    pub fn manifest(&self) -> &SparseHeightManifest {
        &self.manifest
    }

    pub fn height_bounds(&self) -> [f32; 2] {
        self.height_bounds
    }

    /// 分支只写新的不可变清单；场景清单采用其路径才生效，撤销可立即恢复旧路径。
    pub fn fork_to(&mut self, path: &str) -> Result<(), String> {
        if super::sparse_height_manifest_terrain_id(&self.manifest.scene_id, path)? != self.manifest.terrain_id { return Err("高度分支不能跨场景或跨地形".into()); }
        let destination = safe_content_path(&self.asset_root, path)?;
        if file_hash(&destination)? != "missing" { return Err("高度分支版本已存在，未覆盖旧文件".into()); }
        self.verify_source_version()?;
        self.origin_guard = Some((safe_content_path(&self.asset_root, &self.source_path)?, self.disk_hash.clone()));
        self.source_path = path.to_owned(); self.disk_hash = "missing".into(); self.manifest_dirty = true;
        Ok(())
    }

    /// 当前驻留块数供后台缓存实施明确预算。
    pub fn loaded_tile_count(&self) -> usize {
        self.loaded.len()
    }

    /// 当前保存待提交块数供编辑器显示和后台队列限流。
    pub fn dirty_tile_count(&self) -> usize {
        self.dirty.len()
    }

    /// 当前已驻留但没有未保存修改的块数，供上层作者会话实施固定内存预算。
    pub fn clean_loaded_tile_count(&self) -> usize {
        self.loaded
            .keys()
            .filter(|key| !self.dirty.contains(key))
            .count() // 只扫描实际驻留块，不与整图块坐标数量相关。
    }

    /// 判断一个真实作者块是否已经驻留；隐式平地仍不算缓存占用。
    pub fn is_tile_loaded(&self, key: SparseHeightTileKey) -> bool {
        self.loaded.contains_key(&key) // O(log n) 查询只用于后台 IO 任务内的有界 LRU。
    }

    /// 按清单读取单个实际非平地块；缺席块仍是隐式平地且不分配。
    pub fn load_tile(&mut self, key: SparseHeightTileKey) -> Result<(), String> {
        if self.manifest.spec.tile_shape(key).is_none() {
            return Err("请求的高度块超出场景范围".into());
        }
        if self.loaded.contains_key(&key) {
            return Ok(()); // 已编辑的块不能被磁盘旧版本覆写。
        }
        let Some(reference) = self.manifest.tile_ref(key) else {
            return Ok(()); // 清单未列出的平地块不读取文件。
        };
        let path = safe_content_path(
            &self.asset_root,
            &self.manifest.tile_asset_path(key, &reference.sha256),
        )?;
        let file = File::open(&path)
            .map_err(|error| format!("读取高度块{}失败：{error}", path.display()))?;
        let mut bytes = Vec::new();
        file.take((maximum_tile_file_bytes() + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("读取高度块{}失败：{error}", path.display()))?; // 坏资产也只能短暂分配一块上限。
        if bytes.len() > maximum_tile_file_bytes()
            || crate::authoring_io::bytes_hash(&bytes) != reference.sha256
        {
            return Err(format!("高度块内容摘要无效：{}", path.display()));
        }
        let tile = SparseHeightTile::decode_bytes(&self.manifest.spec, key, &bytes)?;
        self.include_height_bounds(tile.height_bounds(&self.manifest.spec));
        self.loaded.insert(key, tile); // 校验完整后才使当前会话可查询这块。
        Ok(())
    }

    /// 干净块可卸载；脏块必须先保存或撤销，不能在内存预算淘汰时丢失笔划。
    pub fn unload_clean_tile(&mut self, key: SparseHeightTileKey) -> bool {
        !self.dirty.contains(&key) && self.loaded.remove(&key).is_some()
    }

    /// 为实时场景覆盖冻结一个已知局部块；隐式平地返回同形状的显式空白快照。
    pub fn tile_snapshot(&self, key: SparseHeightTileKey) -> Result<Arc<SparseHeightTile>, String> {
        if self.manifest.spec.tile_shape(key).is_none() {
            return Err("请求的高度块超出场景范围".into());
        }
        if let Some(tile) = self.loaded.get(&key) {
            return Ok(Arc::new(tile.clone())); // 单页上限约264KiB，冻结发生在有界计算任务。
        }
        if self.manifest.tile_ref(key).is_some() {
            return Err("真实非平地块尚未加载，不能发布未知实时覆盖".into());
        }
        Ok(Arc::new(SparseHeightTile::blank(&self.manifest.spec, key)?)) // 撤销到平地时显式块必须遮蔽场景仍持有的旧磁盘页。
    }

    /// 已驻留块返回高度码；清单中未加载的非平地块返回 None，不能伪装成平地。
    pub fn sample_grid_encoded(&self, x: u64, z: u64) -> Result<Option<u32>, String> {
        let (key, local) = self
            .manifest
            .spec
            .tile_for_grid_point(x, z)
            .ok_or("格点超出稀疏地形边界")?;
        if let Some(tile) = self.loaded.get(&key) {
            return Ok(tile.get_height(&self.manifest.spec, local)); // 旧页解码和格点归属都绑定该源规格。
        }
        if self.manifest.tile_ref(key).is_some() {
            Ok(None) // 异步加载尚未完成，调用方使用旧有效页或明确等待。
        } else {
            Ok(Some(self.manifest.spec.flat_height_code())) // 空白块不需要文件或驻留数组。
        }
    }

    /// 按全图格点取得世界米制高度，隐式平地精确返回 Y=0。
    pub fn sample_grid(&self, x: u64, z: u64) -> Result<Option<f32>, String> {
        Ok(self
            .sample_grid_encoded(x, z)?
            .map(|value| self.manifest.spec.decode_height(value)))
    }

    /// 按正式 `NW-NE-SW / SE-NE-SW` 两三角剖分采样世界 X/Z；缺少真实块时返回 None。
    pub fn sample_world_height(&self, world_x: f64, world_z: f64) -> Result<Option<f32>, String> {
        let spec = &self.manifest.spec;
        let world = [world_x, world_z];
        let mut low = [0_u64; 2];
        let mut high = [0_u64; 2];
        let mut fraction = [0.0_f64; 2];
        for axis in 0..2 {
            if !world[axis].is_finite()
                || world[axis] < f64::from(spec.world_min[axis])
                || world[axis] > f64::from(spec.world_max[axis])
            {
                return Err("世界采样坐标超出稀疏地形范围".into());
            }
            let grid = ((world[axis] - f64::from(spec.world_min[axis]))
                / spec.actual_spacing_meters[axis])
                .clamp(0.0, spec.cells[axis] as f64);
            low[axis] = grid.floor() as u64;
            high[axis] = (low[axis] + 1).min(spec.cells[axis]);
            fraction[axis] = grid - low[axis] as f64;
        }
        let Some(a) = self.sample_grid(low[0], low[1])? else {
            return Ok(None);
        };
        let Some(b) = self.sample_grid(high[0], low[1])? else {
            return Ok(None);
        };
        let Some(c) = self.sample_grid(low[0], high[1])? else {
            return Ok(None);
        };
        let Some(d) = self.sample_grid(high[0], high[1])? else {
            return Ok(None);
        };
        let x = fraction[0]; // 当前单元内从西到东的归一化位置。
        let z = fraction[1]; // 当前单元内从北到南的归一化位置。
        let height = if x + z <= 1.0 {
            f64::from(a) + x * (f64::from(b) - f64::from(a)) + z * (f64::from(c) - f64::from(a))
            // 西北三角使用 NW 原点及指向 NE、SW 的两条边。
        } else {
            f64::from(d) + (1.0 - z) * (f64::from(b) - f64::from(d))
                + (1.0 - x) * (f64::from(c) - f64::from(d))
            // 东南三角从 SE 反向插值到 NE、SW，共享同一对角线。
        };
        Ok(Some(height as f32)) // 与碰撞网格和旧正式高度场保持完全相同的片面语义。
    }

    /// 作者米制笔划只修改目标格点所在块；跨块笔划由上层命令聚合成一次事务。
    pub fn set_grid_height(&mut self, x: u64, z: u64, meters: f32) -> Result<(), String> {
        let code = self.manifest.spec.encode_height(meters)?;
        self.set_grid_encoded(x, z, code)
    }

    /// 低级编辑接口便于撤销差分和按原码无损恢复。
    pub fn set_grid_encoded(&mut self, x: u64, z: u64, code: u32) -> Result<(), String> {
        if !super::tile::valid_height_code(code) {
            return Err("作者高度位模式必须有限且使用规范零值".into());
        }
        let (key, local) = self
            .manifest
            .spec
            .tile_for_grid_point(x, z)
            .ok_or("编辑格点超出稀疏地形边界")?;
        if !self.loaded.contains_key(&key) {
            if self.manifest.tile_ref(key).is_some() {
                return Err("非平地块尚未加载，不能把未知作者高度当作零平地覆盖".into());
            }
            if code == self.manifest.spec.flat_height_code() {
                return Ok(()); // 对隐式平地写零既不分配内存也不产生脏文件。
            }
            self.loaded
                .insert(key, SparseHeightTile::blank(&self.manifest.spec, key)?);
        }
        let tile = self
            .loaded
            .get_mut(&key)
            .expect("块已加载或已由当前笔划创建");
        if tile.get(local) != Some(code) {
            tile.set(local, code)?;
            self.dirty.insert(key); // 后续高度页/碰撞可从这份精确脏块集合派生。
            self.include_height_bounds([f32::from_bits(code); 2]);
        }
        Ok(())
    }

    /// 后台局部候选先验证全部起点，再复制触及页；原文档始终不修改。
    pub(super) fn prepare_edit_tiles(
        &self,
        changes: &[super::SparseTerrainHeightChange],
        redo: bool,
    ) -> Result<Vec<SparseHeightTile>, String> {
        self.prepare_edit_tiles_with_limit(changes, redo, super::author::MAX_SPARSE_PREPARED_EDIT_TILES)
    }

    /// 整笔历史受调用方的历史字节预算约束，不能误用单次建筑平台的36页门截断长笔划。
    pub(super) fn prepare_history_edit_tiles(
        &self,
        changes: &[super::SparseTerrainHeightChange],
        redo: bool,
    ) -> Result<Vec<SparseHeightTile>, String> {
        self.prepare_edit_tiles_with_limit(changes, redo, usize::MAX)
    }

    fn prepare_edit_tiles_with_limit(
        &self,
        changes: &[super::SparseTerrainHeightChange],
        redo: bool,
        page_limit: usize,
    ) -> Result<Vec<SparseHeightTile>, String> {
        let mut tiles = BTreeMap::new();
        let mut points = BTreeSet::new();
        for change in changes {
            if !super::tile::valid_height_code(change.before) || !super::tile::valid_height_code(change.after) {
                return Err("稀疏局部候选包含非有限高度或非规范零值".into());
            }
            if !points.insert((change.x, change.z)) {
                return Err("稀疏局部候选包含重复格点".into());
            }
            let expected = if redo { change.before } else { change.after };
            if self.sample_grid_encoded(change.x, change.z)? != Some(expected) {
                return Err("稀疏局部候选的作者起点不一致".into());
            }
            let (key, _) = self
                .manifest
                .spec
                .tile_for_grid_point(change.x, change.z)
                .ok_or("局部候选格点越界")?;
            tiles.entry(key).or_insert_with(|| None);
            if tiles.len() > page_limit {
                return Err("局部候选超过36个作者页预算".into());
            }
        }
        for (key, value) in &mut tiles {
            *value = Some(match self.loaded.get(key) {
                Some(tile) => tile.clone(),
                None => SparseHeightTile::blank(&self.manifest.spec, *key)?,
            }); // 只复制已验证的有限局部页，不克隆清单或其他脏页。
        }
        for change in changes {
            let (key, local) = self
                .manifest
                .spec
                .tile_for_grid_point(change.x, change.z)
                .unwrap();
            tiles
                .get_mut(&key)
                .unwrap()
                .as_mut()
                .unwrap()
                .set(local, if redo { change.after } else { change.before })?;
        }
        Ok(tiles.into_values().map(Option::unwrap).collect())
    }

    /// 仅在IO阶段比较清单基线，拒绝把旧作者会话用于外部已经换代的磁盘版本。
    pub(super) fn verify_source_version(&self) -> Result<(), String> {
        let path = safe_content_path(
            &self.asset_root,
            &self.source_path,
        )?;
        compare_content_hash(&self.disk_hash, &file_hash(&path)?)
    }

    /// 候选来自同代受检输出；局部平台至多36页，完整历史的交换由调用方放入CPU后台槽。
    pub(super) fn install_edit_tiles(
        &mut self,
        tiles: Vec<SparseHeightTile>,
    ) -> Vec<(SparseHeightTileKey, Option<SparseHeightTile>, bool)> {
        tiles
            .into_iter()
            .map(|tile| {
                let key = tile.key;
                self.include_height_bounds(tile.height_bounds(&self.manifest.spec));
                let was_dirty = self.dirty.contains(&key);
                let previous = self.loaded.insert(key, tile);
                self.dirty.insert(key);
                (key, previous, was_dirty)
            })
            .collect()
    }

    pub(super) fn rollback_edit_tiles(
        &mut self,
        previous: Vec<(SparseHeightTileKey, Option<SparseHeightTile>, bool)>,
    ) {
        for (key, tile, was_dirty) in previous {
            if let Some(tile) = tile {
                self.loaded.insert(key, tile);
            } else {
                self.loaded.remove(&key);
            }
            if was_dirty {
                self.dirty.insert(key);
            } else {
                self.dirty.remove(&key);
            }
        }
    }

    fn include_height_bounds(&mut self, bounds: [f32; 2]) {
        self.height_bounds[0] = self.height_bounds[0].min(bounds[0]).min(0.0);
        self.height_bounds[1] = self.height_bounds[1].max(bounds[1]).max(0.0);
    }

    /// 发布所有脏块及清单；不可变块先落盘，清单最后一次原子替换。
    pub fn save_dirty(&mut self) -> Result<(), String> {
        if !self.manifest_dirty && self.dirty.is_empty() {
            return Ok(()); // 不触发无意义 IO 或热更新。
        }
        let _write_lock = acquire_scene_write_lock(&self.asset_root, &self.manifest.scene_id)?; // 与外层全局作者锁独立，同场景写入严格串行。
        let manifest_path = safe_content_path(
            &self.asset_root,
            &self.source_path,
        )?;
        compare_content_hash(&self.disk_hash, &file_hash(&manifest_path)?)?;
        if let Some((path, expected)) = &self.origin_guard { compare_content_hash(expected, &file_hash(path)?)?; }
        let mut next_manifest = self.manifest.clone(); // 清单规模仅与实际非平地块数相关。
        next_manifest.schema_version = super::SPARSE_HEIGHT_SCHEMA_VERSION;
        next_manifest.height_bounds = Some(self.height_bounds); // 与本次页版本共同发布，旧量程及原页哈希不改。
        let mut indexed = next_manifest
            .tiles
            .iter()
            .map(|reference| (reference.key, reference.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut writes = Vec::new(); // 只登记脏块，编码会延迟到各自 stage 创建时。
        for key in &self.dirty {
            let tile = self.loaded.get(key).ok_or("脏高度块已经从内存丢失")?;
            if tile.is_flat(&self.manifest.spec) {
                indexed.remove(key); // 文件旧版本可保留给持有旧清单的读者，当前清单已视为平地。
                continue;
            }
            let sha256 = tile.sha256(); // 峰值只额外分配当前最多约264KiB页，不保存整图编码副本。
            let path = safe_content_path(
                &self.asset_root,
                &self.manifest.tile_asset_path(*key, &sha256),
            )?;
            match file_hash(&path)?.as_str() {
                "missing" => writes.push(PreparedWrite::deferred(path, move || {
                    Ok(tile.encode_bytes())
                })),
                actual if actual == sha256 => {} // 同一不可变内容已经存在，可直接复用。
                _ => {
                    return Err(format!(
                        "内容寻址高度块已存在但摘要不符：{}",
                        path.display()
                    ));
                }
            }
            indexed.insert(*key, SparseHeightTileRef { key: *key, sha256 });
        }
        next_manifest.tiles = indexed.values().cloned().collect(); // BTreeMap 确保稳定严格升序，并保留索引供父页派生读取。
        let mut indexed_mips = next_manifest
            .mip_tiles
            .iter()
            .map(|reference| ((reference.level, reference.key), reference.clone()))
            .collect::<BTreeMap<_, _>>(); // 父页也以稳定 `(level, key)` 身份更新。
        let mut derived = BTreeMap::<(u8, SparseHeightTileKey), Arc<SparseHeightTile>>::new();
        let mut source_cache =
            BTreeMap::<(u8, SparseHeightTileKey), Option<Arc<SparseHeightTile>>>::new();
        let mut dirty_children = self.dirty.clone(); // 每个脏作者页每层只传播到一个祖先。
        for level in 1..=next_manifest.spec.max_mip_level() {
            let parents = dirty_children
                .iter()
                .map(|key| SparseHeightTileKey {
                    x: key.x / 2,
                    z: key.z / 2,
                })
                .collect::<BTreeSet<_>>();
            for key in &parents {
                let tile = Arc::new(derive_mip_tile(
                    &self.asset_root,
                    &next_manifest,
                    &indexed,
                    &indexed_mips,
                    &self.loaded,
                    &derived,
                    &mut source_cache,
                    level,
                    *key,
                )?); // 每个父页只从前一级受检页抽取全局偶数格点，不扫描更细整图。
                derived.insert((level, *key), Arc::clone(&tile)); // 下一层可直接复用本次新结果。
                source_cache.insert((level, *key), Some(Arc::clone(&tile))); // 同层相邻父页的共享边也复用同一快照。
                if tile.is_flat(&next_manifest.spec) {
                    indexed_mips.remove(&(level, *key)); // 回平后父链继续传播隐式世界零高度。
                    continue;
                }
                let sha256 = tile.sha256();
                let path = safe_content_path(
                    &self.asset_root,
                    &next_manifest.mip_tile_asset_path(level, *key, &sha256),
                )?;
                match file_hash(&path)?.as_str() {
                    "missing" => {
                        let frozen = Arc::clone(&tile);
                        writes.push(PreparedWrite::deferred(path, move || {
                            Ok(frozen.encode_bytes())
                        })); // 单页在自己的 stage 开始时编码，峰值仍受 257² 上限约束。
                    }
                    actual if actual == sha256 => {} // 同一派生内容已存在时直接复用不可变文件。
                    _ => {
                        return Err(format!(
                            "内容寻址高度 Mip 页已存在但摘要不符：{}",
                            path.display()
                        ));
                    }
                }
                indexed_mips.insert(
                    (level, *key),
                    SparseHeightMipTileRef {
                        level,
                        key: *key,
                        sha256,
                    },
                );
            }
            dirty_children = parents; // 只把真正受影响的祖先继续传播到更粗层。
            derived.retain(|(cached_level, _), _| *cached_level == level); // 下一层只需要刚生成的直接子页。
            source_cache.retain(|(cached_level, _), _| *cached_level == level); // 释放更细只读页，避免保存峰值随层数叠加。
        }
        next_manifest.mip_tiles = indexed_mips.into_values().collect();
        next_manifest.validate()?;
        let next_hash = pretty_json_hash(&next_manifest)?; // 与 PreparedWrite::json 完全相同的完成字节。
        writes.push(PreparedWrite::json(manifest_path.clone(), &next_manifest)); // 清单永远最后发布。
        let expected_hash = self.disk_hash.clone();
        let checked_path = manifest_path.clone();
        let origin_guard = self.origin_guard.clone();
        commit_prepared_writes_checked(writes, move |_, target| {
            if let Some((path, expected)) = &origin_guard { compare_content_hash(expected, &file_hash(path)?)?; }
            compare_content_hash(&expected_hash, &file_hash(&checked_path)?)?;
            if target != checked_path && file_hash(target)? != "missing" {
                return Err(format!(
                    "高度块不可变路径已被其他写者占用：{}",
                    target.display()
                ));
            }
            Ok(())
        })
        .map_err(|failure| failure.message().to_owned())?; // 版本变化或 IO 失败均回滚此前已发布候选。
        self.manifest = next_manifest; // 只有完整事务成功才能更新当前会话权威索引。
        self.disk_hash = next_hash; // 下一笔保存以这次完成清单为并发基线。
        self.dirty.clear(); // 磁盘与内存已采用同一修订。
        self.manifest_dirty = false; // 空图初始清单已存在。
        self.origin_guard = None;
        Ok(())
    }
}

/// 从当前会话、同次新父页或正式内容地址文件取得一个前一级页；缺席表示隐式平地。
#[allow(clippy::too_many_arguments)]
fn load_derivation_source(
    asset_root: &Path,
    manifest: &SparseHeightManifest,
    base_index: &BTreeMap<SparseHeightTileKey, SparseHeightTileRef>,
    mip_index: &BTreeMap<(u8, SparseHeightTileKey), SparseHeightMipTileRef>,
    loaded: &BTreeMap<SparseHeightTileKey, SparseHeightTile>,
    derived: &BTreeMap<(u8, SparseHeightTileKey), Arc<SparseHeightTile>>,
    cache: &mut BTreeMap<(u8, SparseHeightTileKey), Option<Arc<SparseHeightTile>>>,
    level: u8,
    key: SparseHeightTileKey,
) -> Result<Option<Arc<SparseHeightTile>>, String> {
    if let Some(cached) = cache.get(&(level, key)) {
        return Ok(cached.clone()); // 相邻父页共享的边界源只读一次。
    }
    let tile = if level == 0 {
        if let Some(tile) = loaded.get(&key) {
            Some(Arc::new(tile.clone())) // 脏块必须优先于仍在磁盘上的旧内容地址版本。
        } else if let Some(reference) = base_index.get(&key) {
            Some(Arc::new(read_content_addressed_page(
                asset_root,
                manifest,
                0,
                key,
                &reference.sha256,
            )?))
        } else {
            None // 缺席基础作者页严格解释为世界 Y=0。
        }
    } else if let Some(tile) = derived.get(&(level, key)) {
        Some(Arc::clone(tile)) // 同次保存已经生成的新父页优先于旧清单引用。
    } else if let Some(reference) = mip_index.get(&(level, key)) {
        Some(Arc::new(read_content_addressed_page(
            asset_root,
            manifest,
            level,
            key,
            &reference.sha256,
        )?))
    } else {
        None // 没有父页文件的区域完全为平地。
    };
    cache.insert((level, key), tile.clone());
    Ok(tile)
}

/// 由前一级页的全局偶数格点生成一个父页；共享边由同一所有者规则取得。
#[allow(clippy::too_many_arguments)]
fn derive_mip_tile(
    asset_root: &Path,
    manifest: &SparseHeightManifest,
    base_index: &BTreeMap<SparseHeightTileKey, SparseHeightTileRef>,
    mip_index: &BTreeMap<(u8, SparseHeightTileKey), SparseHeightMipTileRef>,
    loaded: &BTreeMap<SparseHeightTileKey, SparseHeightTile>,
    derived: &BTreeMap<(u8, SparseHeightTileKey), Arc<SparseHeightTile>>,
    cache: &mut BTreeMap<(u8, SparseHeightTileKey), Option<Arc<SparseHeightTile>>>,
    level: u8,
    key: SparseHeightTileKey,
) -> Result<SparseHeightTile, String> {
    let mut parent = SparseHeightTile::blank_at_level(&manifest.spec, level, key)?;
    let parent_start = [
        u64::from(key.x) * super::SPARSE_HEIGHT_TILE_CELLS,
        u64::from(key.z) * super::SPARSE_HEIGHT_TILE_CELLS,
    ];
    let child_level = level - 1; // 调用方只传一至最高层，因此减一安全。
    let child_cells = manifest
        .spec
        .mip_cells(child_level)
        .ok_or("派生高度父页的子层超出场景规格")?;
    let flat = manifest.spec.flat_height_code();
    for local_z in 0..parent.shape[1] {
        for local_x in 0..parent.shape[0] {
            let child_grid = [
                (parent_start[0] + u64::from(local_x))
                    .saturating_mul(2)
                    .min(child_cells[0]),
                (parent_start[1] + u64::from(local_z))
                    .saturating_mul(2)
                    .min(child_cells[1]),
            ]; // 最后一个不整除端点钳制到子层全图终点。
            let (child_key, child_local) = manifest
                .spec
                .mip_tile_for_grid_point(child_level, child_grid[0], child_grid[1])
                .ok_or("派生高度父页采样落在子层场景范围外")?;
            let value = load_derivation_source(
                asset_root,
                manifest,
                base_index,
                mip_index,
                loaded,
                derived,
                cache,
                child_level,
                child_key,
            )?
            .and_then(|tile| tile.get_height(&manifest.spec, child_local))
            .unwrap_or(flat);
            if value != flat {
                parent.set([local_x, local_z], value)?; // 平地默认值无需产生多余写操作。
            }
        }
    }
    Ok(parent)
}

/// 读取一个基础页或父页并同时验证哈希、头部、坐标和当前层尾页形状。
fn read_content_addressed_page(
    asset_root: &Path,
    manifest: &SparseHeightManifest,
    level: u8,
    key: SparseHeightTileKey,
    sha256: &str,
) -> Result<SparseHeightTile, String> {
    let relative = if level == 0 {
        manifest.tile_asset_path(key, sha256)
    } else {
        manifest.mip_tile_asset_path(level, key, sha256)
    };
    let path = safe_content_path(asset_root, &relative)?;
    let file =
        File::open(&path).map_err(|error| format!("读取高度页{}失败：{error}", path.display()))?;
    let mut bytes = Vec::new();
    file.take((maximum_tile_file_bytes() + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("读取高度页{}失败：{error}", path.display()))?;
    if bytes.len() > maximum_tile_file_bytes() || bytes_hash(&bytes) != sha256 {
        return Err(format!("高度页内容摘要无效：{}", path.display()));
    }
    SparseHeightTile::decode_bytes_at_level(&manifest.spec, level, key, &bytes)
}

fn canonical_asset_root(asset_root: &Path) -> Result<PathBuf, String> {
    asset_root
        .canonicalize()
        .map_err(|error| format!("资产根不可用：{error}")) // 共享作者路径校验需要真实根路径。
}

/// 高度块专用锁不与建图已经持有的公共作者锁重入冲突。
fn acquire_scene_write_lock(asset_root: &Path, scene_id: &SceneId) -> Result<File, String> {
    let identity = format!("{}:{scene_id}", asset_root.display()); // 同资产根和场景 ID 共享一把锁。
    let path = std::env::temp_dir().join(format!(
        "bevy-sparse-height-{}.lock",
        bytes_hash(identity.as_bytes())
    ));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| format!("打开高度作者锁失败：{error}"))?;
    file.try_lock()
        .map_err(|error| format!("另一作者正在保存此场景高度，请稍后重试：{error}"))?;
    Ok(file) // 句柄在 save_dirty 事务完成后释放锁。
}
