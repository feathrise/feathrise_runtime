//! 新场景稀疏地形的局部作者接口；编辑、载入和保存都可移入有界后台任务。
//!
//! 这里不依赖 UI、ECS 或渲染器；F12 与未来独立编辑器共享同一份编辑语义。
//! 调用方负责把按需 IO、纯内存计算和结果采用放进有界后台任务，避免物化完整高度数组或 GLB 网格。

use std::collections::{BTreeSet, VecDeque}; // 一次刷印只预加载实际块，干净作者页按有界 LRU 回收。
use std::path::Path; // 调用方提供 assets 根，文件 IO 必须在 IoTaskPool 执行。
use std::sync::Arc; // 实时场景覆盖共享不可变局部块。
use std::sync::atomic::{AtomicU64, Ordering}; // 同场景重新载入也是不同作者源，零修订不能互相授信。

static NEXT_AUTHOR_SESSION: AtomicU64 = AtomicU64::new(1);

use super::{SparseHeightDocument, SparseHeightSpec, SparseHeightTile, SparseHeightTileKey}; // 作者核心只依赖同模块的持久格式与按需文档。
use crate::SceneId; // 安全场景目录身份由公共 SceneId 统一验证。

/// 单次刷印允许的最大候选格点数；过大笔刷必须明确拒绝而不是冻结输入帧。
pub const MAX_SPARSE_BRUSH_CANDIDATES: u64 = 262_144;

/// 长时间浏览最多保留的干净作者块；脏块另行保留直到保存或撤销。
pub const MAX_SPARSE_AUTHOR_CLEAN_TILES: usize = 64;

/// 建筑与联合历史候选最多涉及36页，与精确矩形查询的局部IO门一致。
pub const MAX_SPARSE_PREPARED_EDIT_TILES: usize = 36;

/// 新图高度笔刷最小可用工具集合；四工具共享米制速度，平滑和压平接近目标时不越过。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SparseTerrainBrushKind {
    Raise,   // 抬升世界 Y。
    Lower,   // 压低世界 Y。
    Smooth,  // 按米制速度趋向世界笔刷尺度的四邻域平均值。
    Flatten, // 按米制速度趋向本笔首次命中高度。
}

/// 后台任务开始前冻结一帧笔刷的全部参数，不能在运行中读取可变 UI 状态。
#[derive(Clone, Copy, Debug)]
pub struct SparseTerrainBrushInput {
    pub kind: SparseTerrainBrushKind,       // 只允许高度作者工具。
    pub center_xz: [f64; 2],                // 世界固定 X/Z，与 Clipmap 相机环无关。
    pub radius_meters: f32,                 // 有限正半径。
    pub strength: f32,                      // 与 F12 现有 0.01..1 输入一致。
    pub speed_meters_per_second: f32,       // 四种高度工具共享的米制速度；平滑和压平朝目标移动且不越过。
    pub delta_seconds: f32,                 // 单帧积分上限 0.05 秒。
    pub flatten_target_meters: Option<f32>, // 压平目标在按下帧冻结。
}

impl SparseTerrainBrushInput {
    /// 平滑以笔刷世界半径的四分之一采四邻点，至少保留一格；细分作者网格不能让宽坡平滑退化成亚毫米变化。
    /// 两轴独立折算对象缩放和实际格距；预取、冻结采样与编辑器就绪矩形共用此步长，不能各自猜测halo。
    pub fn smoothing_sample_steps(self, spec: &SparseHeightSpec, footprint_scale: [f32; 2]) -> Result<[u64; 2], String> {
        validate_brush_input(self, false)?;
        if footprint_scale.iter().any(|scale| !scale.is_finite() || *scale <= 0.0) {
            return Err("笔刷源坐标比例无效".into());
        }
        if self.kind != SparseTerrainBrushKind::Smooth { return Ok([0; 2]); }
        Ok(std::array::from_fn(|axis| {
            (f64::from(self.radius_meters) * 0.25
                / (spec.actual_spacing_meters[axis] * f64::from(footprint_scale[axis])))
                .round().clamp(1.0, spec.cells[axis] as f64) as u64
        }))
    }
}

/// 一个全局格点的无损米制f32位模式前后值，可组成跨块单次撤销命令。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SparseTerrainHeightChange {
    pub x: u64,      // 全图 X 格点，不使用块局部坐标作作者命令身份。
    pub z: u64,      // 全图 Z 格点。
    pub before: u32, // 本次笔划前米制高度bits，支持精确撤销和负高度。
    pub after: u32,  // 本次笔划后米制高度bits，不以整数大小比较高低。
}

/// 一次完整刷印的局部结果；空变化不会提升会话修订。
#[derive(Debug)]
pub struct SparseTerrainBrushResult {
    pub changes: Vec<SparseTerrainHeightChange>, // 仅记录实际变化格点。
    pub dirty_tiles: usize,                      // 保存和预览可据此规划局部工作量。
}

/// 单图稀疏高度作者会话；不持有 Bevy World、资产句柄或整图高度数组。
pub struct SparseTerrainAuthorSession {
    session_identity: u64,
    scene_id: SceneId,                            // 结果采用必须核对这一场景身份。
    document: SparseHeightDocument,           // 按需载入和脏块持久化的唯一作者源。
    revision: u64,                            // 每次真实局部修改推进，用于拒收迟到后台结果。
    saved_revision: u64,                      // 最近一次完整保存的作者修订。
    clean_lru: VecDeque<SparseHeightTileKey>, // 只记录实际驻留块的最近访问顺序，容量与全图无关。
}

impl SparseTerrainAuthorSession {
    /// 在 IoTaskPool 中载入清单；非空块仍按刷印或查询需要延迟读取。
    pub fn load(asset_root: &Path, scene_id: &SceneId) -> Result<Self, String> {
        Self::load_for_terrain(asset_root, scene_id, None)
    }

    pub fn load_for_terrain(asset_root: &Path, scene_id: &SceneId, terrain_id: Option<&crate::TerrainId>) -> Result<Self, String> {
        let document = SparseHeightDocument::load_for_terrain(asset_root, scene_id, terrain_id)?;
        Ok(Self::from_document(scene_id, document))
    }
    pub fn load_source(asset_root: &Path, scene_id: &SceneId, terrain_id: Option<&crate::TerrainId>, source_path: &str) -> Result<Self, String> {
        let document = SparseHeightDocument::load_source(asset_root, scene_id, terrain_id, source_path)?;
        Ok(Self::from_document(scene_id, document))
    }
    pub fn fork_to(&mut self, path: &str) -> Result<(), String> { self.document.fork_to(path) }
    fn from_document(scene_id: &SceneId, document: SparseHeightDocument) -> Self {
        Self {
            session_identity: NEXT_AUTHOR_SESSION.fetch_add(1, Ordering::Relaxed),
            scene_id: scene_id.clone(), // 迟到任务不能把另一场景的内容接回当前作者草稿。
            document,               // 只载入清单，不读取所有实际编辑过的块。
            revision: 0,            // 新会话从当前磁盘版本开始独立计数。
            saved_revision: 0,      // 尚无未保存修改。
            clean_lru: VecDeque::new(), // 清单载入不读取任何块，因此缓存顺序为空。
        }
    }

    /// 调用方在启动和采用后台任务时都可核对稳定场景身份。
    pub fn scene_id(&self) -> &SceneId {
        &self.scene_id
    }

    /// 只有完整保存后才认为作者源与磁盘一致，编辑器可直接用它驱动关闭和切换场景保护。
    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    /// 返回当前清单的固定规格；独立编辑器可据此换算世界坐标而无需接触内部文档。
    pub fn spec(&self) -> &SparseHeightSpec {
        &self.document.manifest().spec
    }

    /// 纯内存全局格点查询，供有界建筑平台规划保留原始量化值。
    pub fn sample_grid_encoded(&self, x: u64, z: u64) -> Result<Option<u32>, String> {
        self.document.sample_grid_encoded(x, z)
    }

    pub fn height_bounds(&self) -> [f32; 2] {
        self.document.height_bounds()
    }

    pub fn verify_source_version(&self) -> Result<(), String> {
        self.document.verify_source_version() // 磁盘IO由调用者安排进原单飞IO槽。
    }

    /// IO阶段先验证矩形预算，再读取全部相关基础页；越界和超预算不会部分编辑。
    pub fn prefetch_grid_rect(&mut self, min: [u64; 2], max: [u64; 2]) -> Result<(), String> {
        let spec = self.spec();
        if (0..2).any(|axis| min[axis] > max[axis] || max[axis] > spec.cells[axis]) {
            return Err("局部高度矩形越界".into());
        }
        let first = spec.tile_for_grid_point(min[0], min[1]).unwrap().0;
        let last = spec.tile_for_grid_point(max[0], max[1]).unwrap().0;
        let count = u64::from(last.x - first.x + 1) * u64::from(last.z - first.z + 1);
        if count > MAX_SPARSE_PREPARED_EDIT_TILES as u64 {
            return Err("局部高度矩形超过36个作者页预算".into());
        }
        for z in first.z..=last.z {
            for x in first.x..=last.x {
                let key = SparseHeightTileKey { x, z };
                self.document.load_tile(key)?;
                self.touch_loaded_tile(key);
            }
        }
        Ok(())
    }

    /// CPU阶段形成完整候选，原会话保留不变；失败或迟到可直接丢弃候选。
    pub fn prepare_changes_loaded(
        &mut self,
        changes: Arc<Vec<SparseTerrainHeightChange>>,
        redo: bool,
    ) -> Result<super::SparseTerrainPreparedEdit, String> {
        if changes.len() as u64 > MAX_SPARSE_BRUSH_CANDIDATES {
            return Err("稀疏局部候选超过262144个格点预算".into());
        }
        let writable_tiles = self.document.prepare_edit_tiles(&changes, redo)?;
        Ok(self.finish_prepared_edit(changes, writable_tiles))
    }

    /// 完整历史先形成只读候选。调用方先按需预取，并在后台安装、失效回滚和释放旧页，
    /// 由原历史预算限制整笔大小；单次刷印及建筑平台仍遵守各自的格点与页数门。
    pub fn prepare_history_changes_loaded(
        &mut self,
        changes: Arc<Vec<SparseTerrainHeightChange>>,
        redo: bool,
    ) -> Result<super::SparseTerrainPreparedEdit, String> {
        let writable_tiles = self.document.prepare_history_edit_tiles(&changes, redo)?;
        Ok(self.finish_prepared_edit(changes, writable_tiles))
    }

    fn finish_prepared_edit(
        &mut self,
        changes: Arc<Vec<SparseTerrainHeightChange>>,
        writable_tiles: Vec<SparseHeightTile>,
    ) -> super::SparseTerrainPreparedEdit {
        let live_tiles = writable_tiles
            .iter()
            .map(|tile| Arc::new(tile.clone()))
            .collect();
        let edit = super::SparseTerrainPreparedEdit {
            scene_id: self.scene_id.clone(),
            source_revision: self.revision,
            source_session: self.session_identity,
            changes,
            writable_tiles,
            live_tiles,
        };
        self.trim_clean_tile_cache(); // 完整结果已冻结，预取工作集可回到原64干净页预算。
        edit
    }

    /// 常数身份复核后，只交换有限页；不在采用帧遍历格点差分或复制高度数组。
    pub fn install_prepared_edit(
        &mut self,
        edit: super::SparseTerrainPreparedEdit,
    ) -> Result<super::SparseTerrainAppliedEdit, String> {
        if edit.scene_id != self.scene_id
            || edit.source_revision != self.revision
            || edit.source_session != self.session_identity
        {
            return Err("稀疏局部候选所属场景或作者修订已过期".into());
        }
        let previous = self.document.install_edit_tiles(edit.writable_tiles);
        let source_revision = self.revision;
        if !edit.changes.is_empty() {
            self.revision = self.revision.wrapping_add(1).max(1);
        }
        Ok(super::SparseTerrainAppliedEdit {
            changes: edit.changes,
            live_tiles: edit.live_tiles,
            rollback: super::SparseTerrainEditRollback {
                source_revision,
                adopted_revision: self.revision,
                source_session: self.session_identity,
                previous,
            },
        })
    }

    /// 同次建筑/世界采用失败时，恢复页、原脏集合和修订；不产生历史或保存责任。
    pub fn rollback_prepared_edit(
        &mut self,
        rollback: super::SparseTerrainEditRollback,
    ) -> Result<(), String> {
        if self.revision != rollback.adopted_revision
            || self.session_identity != rollback.source_session
        {
            return Err("局部页回滚期间作者修订已变化".into());
        }
        self.document.rollback_edit_tiles(rollback.previous);
        self.revision = rollback.source_revision;
        Ok(())
    }

    /// 返回当前已驻留作者页总数，供诊断和有界缓存验收使用。
    pub fn loaded_tile_count(&self) -> usize {
        self.document.loaded_tile_count()
    }

    /// 返回当前已驻留且未修改的作者页数，供编辑器展示缓存状态和测试上限。
    pub fn clean_loaded_tile_count(&self) -> usize {
        self.document.clean_loaded_tile_count()
    }

    /// 纯内存世界查询；真实非空块未载入时返回 None，不能伪装零平地。
    pub fn sample_world_height(&self, x: f64, z: f64) -> Result<Option<f32>, String> {
        self.document.sample_world_height(x, z)
    }

    /// 返回世界采样所在的主作者块，供视口按块抑制坏资产的重复预取。
    pub fn world_sample_tile_key(&self, x: f64, z: f64) -> Result<SparseHeightTileKey, String> {
        let spec = &self.document.manifest().spec; // 只读取固定大小规格，不触发块 IO。
        let world = [x, z]; // 世界 X/Z 使用与采样入口相同的轴顺序。
        let mut grid = [0_u64; 2]; // 主块按目标点左上格点确定。
        for axis in 0..2 {
            if !world[axis].is_finite()
                || world[axis] < f64::from(spec.world_min[axis])
                || world[axis] > f64::from(spec.world_max[axis])
            {
                return Err("世界采样坐标超出稀疏地形范围".into()); // 越界点不进入预取队列。
            }
            grid[axis] = ((world[axis] - f64::from(spec.world_min[axis]))
                / spec.actual_spacing_meters[axis])
                .clamp(0.0, spec.cells[axis] as f64)
                .floor() as u64; // 与正式世界采样的低端格点完全一致。
        }
        spec.tile_for_grid_point(grid[0], grid[1])
            .map(|(key, _)| key)
            .ok_or_else(|| "世界采样格点超出稀疏地形边界".to_owned()) // 规格已验证时只保留防御错误。
    }

    /// 用已驻留作者块把透视射线迭代到真实高度面；未知块明确返回 None 让视口使用安全托底。
    pub fn intersect_loaded_height_ray(
        &self,
        origin: [f64; 3],
        direction: [f64; 3],
    ) -> Option<[f32; 3]> {
        if origin
            .iter()
            .chain(direction.iter())
            .any(|value| !value.is_finite())
            || direction[1].abs() < 1.0e-7
        {
            return None; // 近水平或非有限射线不能用高度固定点可靠收敛。
        }
        let spec = self.document.manifest().spec.clone();
        let mut minimum_t = 0.0_f64;
        let mut maximum_t = f64::INFINITY;
        for (axis, direction_axis) in [(0_usize, direction[0]), (1_usize, direction[2])] {
            let minimum = f64::from(spec.world_min[axis]);
            let maximum = f64::from(spec.world_max[axis]);
            let origin_axis = origin[axis * 2]; // 世界数组顺序是 X/Y/Z，二维规格顺序是 X/Z。
            if direction_axis.abs() < 1.0e-12 {
                if origin_axis < minimum || origin_axis > maximum {
                    return None; // 平行于该轴且在场景外，永远不会进入高度场。
                }
                continue;
            }
            let first = (minimum - origin_axis) / direction_axis;
            let second = (maximum - origin_axis) / direction_axis;
            minimum_t = minimum_t.max(first.min(second));
            maximum_t = maximum_t.min(first.max(second));
        }
        if maximum_t < minimum_t.max(0.0) {
            return None;
        }
        let [height_min, height_max] = self.document.height_bounds().map(f64::from);
        let vertical_a = (height_min - origin[1]) / direction[1];
        let vertical_b = (height_max - origin[1]) / direction[1];
        minimum_t = minimum_t.max(vertical_a.min(vertical_b)).max(0.0);
        maximum_t = maximum_t.min(vertical_a.max(vertical_b));
        if maximum_t < minimum_t {
            return None; // 射线在场景 X/Z 内也没有穿过合法高度量程。
        }
        if direction[0].abs() < 1.0e-12 && direction[2].abs() < 1.0e-12 {
            let height = self.document.sample_world_height(origin[0], origin[2]).ok().flatten()?;
            let distance = (f64::from(height) - origin[1]) / direction[1];
            return (distance >= minimum_t && distance <= maximum_t)
                .then_some([origin[0] as f32, height, origin[2] as f32]); // 竖直线一次精确求交，不因山高扩大阻尼误差。
        }
        let zero_t = (-origin[1] / direction[1]).clamp(minimum_t, maximum_t);
        let mut distance = zero_t;
        let tolerance = spec.actual_spacing_meters[0]
            .min(spec.actual_spacing_meters[1])
            .mul_add(0.002, 0.0005); // 最细图亚毫米、粗图仍远小于一个作者单元。
        for _ in 0..32 {
            let point = [
                origin[0] + direction[0] * distance,
                origin[1] + direction[1] * distance,
                origin[2] + direction[2] * distance,
            ];
            let height = f64::from(
                self.document
                    .sample_world_height(point[0], point[2])
                    .ok()
                    .flatten()?,
            ); // 正式非平地块尚未预取时不能伪造零高度命中。
            let residual = point[1] - height;
            if residual.abs() <= tolerance {
                return Some([point[0] as f32, height as f32, point[2] as f32]);
            }
            let target = ((height - origin[1]) / direction[1]).clamp(minimum_t, maximum_t);
            distance = (distance + target) * 0.5; // 阻尼固定点避免陡坡在两侧来回跳动。
        }
        let point = [
            origin[0] + direction[0] * distance,
            origin[1] + direction[1] * distance,
            origin[2] + direction[2] * distance,
        ];
        let height = f64::from(
            self.document
                .sample_world_height(point[0], point[2])
                .ok()
                .flatten()?,
        );
        ((point[1] - height).abs() <= tolerance * 4.0).then_some([
            point[0] as f32,
            height as f32,
            point[2] as f32,
        ]) // 未收敛的陡坡不发布看似精确的错误光标。
    }

    /// 在 IoTaskPool 预读一次单元两三角世界采样所需的至多四个作者块。
    pub fn prefetch_world_sample_tiles(&mut self, x: f64, z: f64) -> Result<usize, String> {
        let spec = self.document.manifest().spec.clone(); // 后续可变加载期间不持有文档内部借用。
        let world = [x, z];
        let mut low = [0_u64; 2];
        let mut high = [0_u64; 2];
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
        }
        let mut keys = BTreeSet::new();
        for [grid_x, grid_z] in [
            [low[0], low[1]],
            [high[0], low[1]],
            [low[0], high[1]],
            [high[0], high[1]],
        ] {
            let (key, _) = spec
                .tile_for_grid_point(grid_x, grid_z)
                .ok_or("世界采样格点超出稀疏地形边界")?;
            keys.insert(key);
        }
        let tile_count = keys.len();
        for key in keys {
            self.document.load_tile(key)?;
            self.touch_loaded_tile(key); // 光标移动只保留最近固定数量的真实页。
        }
        self.trim_clean_tile_cache(); // 一次采样所需邻页完整载入后再淘汰最旧页。
        Ok(tile_count)
    }

    /// 串行工具入口；交互式编辑器应分别调度预取与纯内存刷印，不能在帧系统直接调用。
    pub fn apply_brush(
        &mut self,
        input: SparseTerrainBrushInput,
    ) -> Result<SparseTerrainBrushResult, String> {
        self.prefetch_brush_tiles(input)?; // 离线/测试调用可串行处理；运行时不可在帧系统直接调用。
        self.apply_brush_loaded(input)
    }

    /// 在 IoTaskPool 预读实际命中块；预算检查必须先于任何文件 IO。
    pub fn prefetch_brush_tiles(
        &mut self,
        input: SparseTerrainBrushInput,
    ) -> Result<usize, String> {
        self.prefetch_scaled_brush_tiles(input, [1.0; 2])
    }
    pub fn prefetch_scaled_brush_tiles(&mut self, input: SparseTerrainBrushInput, footprint_scale: [f32; 2]) -> Result<usize, String> {
        let spec = self.document.manifest().spec.clone(); // 固定小型规格快照允许随后按需加载作者块。
        let affected = planned_scaled_points(&spec, input, false, footprint_scale)?;
        let smoothing_steps = input.smoothing_sample_steps(&spec, footprint_scale)?;
        let mut keys = BTreeSet::new(); // 实际圆形区域通常小于候选矩形，缺席块不会分配高度。
        for (x, z, _) in &affected {
            if let Some((key, _)) = spec.tile_for_grid_point(*x, *z) {
                keys.insert(key); // 所有工具至少读取自身作者格点所属块。
            }
            if input.kind == SparseTerrainBrushKind::Smooth {
                for [neighbor_x, neighbor_z] in smoothing_neighbors(&spec, *x, *z, smoothing_steps) {
                    if let Some((key, _)) = spec.tile_for_grid_point(neighbor_x, neighbor_z) {
                        keys.insert(key); // 块边界平滑必须连同相邻所有者页完整预取。
                    }
                }
            }
            if keys.len() > MAX_SPARSE_AUTHOR_CLEAN_TILES {
                return Err(format!("笔刷及平滑采样超过单次{MAX_SPARSE_AUTHOR_CLEAN_TILES}页后台预算；请缩小半径"));
            } // 任何IO之前证明完整工作集可保留；否则LRU会在计算前淘汰本次必需的干净页。
        }
        let tile_count = keys.len();
        for key in keys {
            self.document.load_tile(key)?; // IO 失败发生在任何高度修改之前。
            self.touch_loaded_tile(key); // 当前笔刷块提升为最近使用，避免同笔内被淘汰。
        }
        self.trim_clean_tile_cache(); // 脏块尚未形成，保留当前有限工作集并淘汰更旧页。
        Ok(tile_count)
    }

    /// 在 AsyncComputeTaskPool 仅基于已预取块计算并提交局部差分，不触发文件 IO。
    pub fn apply_brush_loaded(
        &mut self,
        input: SparseTerrainBrushInput,
    ) -> Result<SparseTerrainBrushResult, String> {
        self.apply_scaled_brush_loaded(input, [1.0; 2])
    }
    pub fn apply_scaled_brush_loaded(&mut self, input: SparseTerrainBrushInput, footprint_scale: [f32; 2]) -> Result<SparseTerrainBrushResult, String> {
        let spec = self.document.manifest().spec.clone(); // 规格只含小型元数据，不克隆任何格点数组。
        let affected = planned_scaled_points(&spec, input, true, footprint_scale)?;
        let smoothing_steps = input.smoothing_sample_steps(&spec, footprint_scale)?;
        let mut changes = Vec::new();
        for (x, z, falloff) in affected {
            let before = self
                .document
                .sample_grid_encoded(x, z)?
                .ok_or("已请求的高度块未能完整载入")?;
            let old = spec.decode_height(before);
            let next = match input.kind {
                SparseTerrainBrushKind::Raise | SparseTerrainBrushKind::Lower => {
                    let direction = if input.kind == SparseTerrainBrushKind::Raise {
                        1.0
                    } else {
                        -1.0
                    };

                    old + direction
                        * input.speed_meters_per_second
                        * input.strength.clamp(0.01, 1.0)
                        * input.delta_seconds.clamp(0.0, 0.05)
                        * falloff
                }
                SparseTerrainBrushKind::Flatten => {
                    let target = input
                        .flatten_target_meters
                        .ok_or("压平笔刷缺少按下时冻结的高度")?;
                    move_towards_height(old, f64::from(target), input, falloff)
                }
                SparseTerrainBrushKind::Smooth => {
                    let mut sum = f64::from(old); // 有限极值相加在f64累积，不能先在f32溢出再求平均。
                    let mut count = 1_u32; // 边界钳制后的重复自身邻点不重复计入。
                    let mut unique = BTreeSet::new(); // 固定最多四项，不随场景规模增长。
                    for [neighbor_x, neighbor_z] in smoothing_neighbors(&spec, x, z, smoothing_steps) {
                        if !unique.insert([neighbor_x, neighbor_z])
                            || neighbor_x == x && neighbor_z == z
                        {
                            continue; // 场景边缘钳制到自身或同一格点时只保留一个样本。
                        }
                        let encoded = self
                            .document
                            .sample_grid_encoded(neighbor_x, neighbor_z)?
                            .ok_or("平滑所需相邻高度块未能完整载入")?;
                        sum += f64::from(spec.decode_height(encoded)); // 同世界尺度的四邻点等权参与，不随作者细分缩小平滑作用范围。
                        count += 1; // 有效四邻点最多四个。
                    }
                    let average = sum / f64::from(count); // 当前格与有效四邻域的冻结平均值。
                    move_towards_height(old, average, input, falloff)
                }
            }; // 创建量程只解码旧页；新增高度由有限米制浮点表达，不裁掉合法雕刻。
            let after = spec.encode_height(next)?;
            if after != before {
                changes.push(SparseTerrainHeightChange {
                    x,
                    z,
                    before,
                    after,
                }); // 量化后仍相同则不制造虚假脏修订。
            }
        }
        for change in &changes {
            self.document
                .set_grid_encoded(change.x, change.z, change.after)?; // 预读和全量目标校验已完成。
        }
        if !changes.is_empty() {
            self.revision = self.revision.wrapping_add(1).max(1); // 当前完整刷印一次推进作者代次。
        }
        Ok(self.result(changes))
    }

    /// 串行测试入口；生产历史恢复仍分开调度 IO 与纯内存计算。
    pub fn replay_changes(
        &mut self,
        changes: &[SparseTerrainHeightChange], // 一个已经完整形成的跨块笔划。
        redo: bool,                            // false 恢复 before，true 恢复 after。
    ) -> Result<(), String> {
        self.prefetch_change_tiles(changes)?;
        self.replay_changes_loaded(changes, redo)
    }

    /// 在 IoTaskPool 预读完整笔划涉及的真实块；整笔可由多个受预算限制的刷印组成。
    pub fn prefetch_change_tiles(
        &mut self,
        changes: &[SparseTerrainHeightChange],
    ) -> Result<usize, String> {
        self.prefetch_change_tiles_with_limit(changes, usize::MAX)
    }

    /// 联合事务先证明有限页预算再读取；坏历史不能先加载大量页再被CPU候选拒绝。
    pub fn prefetch_prepared_change_tiles(
        &mut self,
        changes: &[SparseTerrainHeightChange],
    ) -> Result<usize, String> {
        if changes.len() as u64 > MAX_SPARSE_BRUSH_CANDIDATES {
            return Err("联合稀疏历史超过262144个格点预算".into());
        } // 有限建筑候选保留格点门，并且必须在任何页 IO 前拒绝。
        self.prefetch_change_tiles_with_limit(changes, MAX_SPARSE_PREPARED_EDIT_TILES)
    }

    fn prefetch_change_tiles_with_limit(
        &mut self,
        changes: &[SparseTerrainHeightChange],
        page_limit: usize,
    ) -> Result<usize, String> {
        let spec = self.document.manifest().spec.clone(); // 小规格快照不会固定整个文档借用。
        let mut keys = BTreeSet::<SparseHeightTileKey>::new();
        for change in changes {
            let (key, _) = spec
                .tile_for_grid_point(change.x, change.z)
                .ok_or("撤销差分超出场景边界")?;
            keys.insert(key); // 首先验证全部坐标，不发生任何编辑。
            if keys.len() > page_limit {
                return Err("联合稀疏历史超过36个作者页预算".into());
            }
        }
        let tile_count = keys.len();
        for key in keys {
            self.document.load_tile(key)?; // 真实块必须完整读取后才能判定当前代次。
            self.touch_loaded_tile(key); // 历史命令触及页进入最近工作集。
        }
        Ok(tile_count) // 原子历史应用前必须暂时保留全部目标页，不能按缓存预算拆成半条命令。
    }

    /// 在有界计算任务中冻结本次差分实际触及块，包含撤销后已经回平的显式空白块。
    pub fn tile_snapshots_for_changes(
        &self,
        changes: &[SparseTerrainHeightChange],
    ) -> Result<Vec<Arc<SparseHeightTile>>, String> {
        let spec = &self.document.manifest().spec;
        let mut keys = BTreeSet::new();
        for change in changes {
            let (key, _) = spec
                .tile_for_grid_point(change.x, change.z)
                .ok_or("实时覆盖差分超出场景边界")?;
            keys.insert(key);
        }
        keys.into_iter()
            .map(|key| self.document.tile_snapshot(key))
            .collect() // 每个实际块至多冻结一次，不按格点重复克隆。
    }

    /// 在 AsyncComputeTaskPool 对已预取块做全量前置检查后原子恢复整笔差分。
    pub fn replay_changes_loaded(
        &mut self,
        changes: &[SparseTerrainHeightChange],
        redo: bool,
    ) -> Result<(), String> {
        // 刷印预算只限制新编辑的单次计算；已记录的完整笔划必须能整笔恢复。
        // 仍先验证全部格点，不能按刷印大小分段写入或暴露半条撤销结果。
        for change in changes {
            if !super::tile::valid_height_code(change.before) || !super::tile::valid_height_code(change.after) {
                return Err("撤销差分包含非有限高度或非规范零值".into());
            }
            let expected = if redo { change.before } else { change.after };
            if self.document.sample_grid_encoded(change.x, change.z)? != Some(expected) {
                return Err("撤销差分与当前作者修订不一致".into());
            }
        }
        for change in changes {
            let target = if redo { change.after } else { change.before };
            self.document.set_grid_encoded(change.x, change.z, target)?; // 全量前置检查使跨块命令不可部分采用。
        }
        if !changes.is_empty() {
            self.revision = self.revision.wrapping_add(1).max(1);
        }
        self.trim_clean_tile_cache(); // 修改页已经成为不可淘汰脏块，此时只回收历史之外的旧干净页。
        Ok(())
    }

    /// 在 IoTaskPool 中保存脏作者块；文档内部保证块先写、清单最后原子发布。
    pub fn save_dirty(&mut self) -> Result<usize, String> {
        let dirty = self.document.dirty_tile_count();
        self.document.save_dirty()?;
        self.saved_revision = self.revision; // 磁盘事务完整成功后才清除当前会话脏身份。
        self.trim_clean_tile_cache(); // 刚保存块已可安全纳入固定干净页预算。
        Ok(dirty)
    }

    /// 更新一个实际驻留块的后台访问顺序；隐式平地从不占用 LRU 项。
    fn touch_loaded_tile(&mut self, key: SparseHeightTileKey) {
        if !self.document.is_tile_loaded(key) {
            return; // 清单缺席块由零高度隐式表达，不制造与浏览范围相关的元数据。
        }
        if let Some(index) = self
            .clean_lru
            .iter()
            .position(|candidate| *candidate == key)
        {
            self.clean_lru.remove(index); // 固定最多几十项，线性移动比额外哈希表更小且确定。
        }
        self.clean_lru.push_back(key); // 队尾始终是最近一次完成 IO 或查询的块。
    }

    /// 淘汰最旧干净页；脏页轮转到队尾并始终保留到成功保存。
    fn trim_clean_tile_cache(&mut self) {
        let mut attempts = self.clean_lru.len(); // 一轮最多查看每个驻留身份一次，杜绝全脏时死循环。
        while self.document.clean_loaded_tile_count() > MAX_SPARSE_AUTHOR_CLEAN_TILES
            && attempts > 0
        {
            let Some(key) = self.clean_lru.pop_front() else {
                break; // 身份表异常为空时宁可保留数据，也不能丢失作者值。
            };
            if !self.document.unload_clean_tile(key) && self.document.is_tile_loaded(key) {
                self.clean_lru.push_back(key); // 未保存脏块仍保留顺序，保存后即可正常淘汰。
            }
            attempts -= 1; // 本次调用不会反复检查同一批脏块。
        }
    }

    fn result(&self, changes: Vec<SparseTerrainHeightChange>) -> SparseTerrainBrushResult {
        SparseTerrainBrushResult {
            changes,                                       // 后续可直接合并为按下至松开的单次历史命令。
            dirty_tiles: self.document.dirty_tile_count(), // 不统计未编辑的全图块。
        }
    }
}

/// 返回世界笔刷尺度折算后的四邻域；场景边缘钳制后由调用方负责去重。
/// 速度决定每秒最多移动的米数，接近冻结目标时只走剩余距离；f64差值保护有限极值。
fn move_towards_height(old: f32, target: f64, input: SparseTerrainBrushInput, falloff: f32) -> f32 {
    let step = f64::from(input.speed_meters_per_second) * f64::from(input.strength.clamp(0.01, 1.0))
        * f64::from(input.delta_seconds.clamp(0.0, 0.05)) * f64::from(falloff);
    (f64::from(old) + (target - f64::from(old)).clamp(-step, step)) as f32
}

fn smoothing_neighbors(spec: &SparseHeightSpec, x: u64, z: u64, steps: [u64; 2]) -> [[u64; 2]; 4] {
    [
        [x.saturating_sub(steps[0]), z],                        // 西侧格点。
        [x.saturating_add(steps[0]).min(spec.cells[0]), z], // 东侧格点。
        [x, z.saturating_sub(steps[1])],                        // 北侧格点。
        [x, z.saturating_add(steps[1]).min(spec.cells[1])], // 南侧格点。
    ] // 固定数组不为每个受影响点分配堆内存。
}

fn validate_brush_input(
    input: SparseTerrainBrushInput,
    require_flatten_target: bool,
) -> Result<(), String> {
    if input.center_xz.iter().any(|value| !value.is_finite())
        || !input.radius_meters.is_finite()
        || input.radius_meters <= 0.0
        || !input.strength.is_finite()
        || !input.speed_meters_per_second.is_finite()
        || input.speed_meters_per_second < 0.0
        || !input.delta_seconds.is_finite()
        || input.delta_seconds < 0.0
        || require_flatten_target
            && input.kind == SparseTerrainBrushKind::Flatten
            && !input.flatten_target_meters.is_some_and(f32::is_finite)
    {
        return Err("稀疏地形笔刷参数无效".into());
    }
    Ok(())
}

/// 预取与计算阶段共享同一受检触点计划，保证两阶段覆盖完全相同的作者块。
/// 非等比缩放后的源坐标圆为椭圆；距离仍以世界米制计算，不让实际刷印扩大。
fn planned_scaled_points(spec: &SparseHeightSpec, input: SparseTerrainBrushInput,
    require_flatten_target: bool, footprint_scale: [f32; 2]) -> Result<Vec<(u64, u64, f32)>, String> {
    validate_brush_input(input, require_flatten_target)?;
    if footprint_scale.iter().any(|scale| !scale.is_finite() || *scale <= 0.0) { return Err("笔刷源坐标比例无效".into()); }
    let scale = footprint_scale.map(f64::from);
    let bounds = brush_grid_bounds(spec, input.center_xz, scale.map(|s| f64::from(input.radius_meters) / s))?;
    let Some((min, max)) = bounds else {
        return Ok(Vec::new()); // 圆完全在场景外不会读块或制造脏修订。
    };
    let width = max[0] - min[0] + 1;
    let height = max[1] - min[1] + 1;
    let candidates = width.checked_mul(height).ok_or("稀疏笔刷候选格点数溢出")?;
    if candidates > MAX_SPARSE_BRUSH_CANDIDATES {
        return Err(format!(
            "当前精度下笔刷需检查{candidates}个格点，超过单次后台预算{MAX_SPARSE_BRUSH_CANDIDATES}；请缩小半径"
        ));
    }
    Ok(affected_points(
        spec,
        min,
        max,
        input.center_xz,
        f64::from(input.radius_meters),
        scale,
    ))
}

/// 把世界圆包围盒限制到格点矩形，最坏工作量随后由固定候选预算判定。
fn brush_grid_bounds(
    spec: &SparseHeightSpec, // 受检清单规格。
    center: [f64; 2],        // 世界笔刷中心。
    radius: [f64; 2],        // 世界圆转换为源坐标的两轴半径。
) -> Result<Option<([u64; 2], [u64; 2])>, String> {
    let mut min = [0_u64; 2];
    let mut max = [0_u64; 2];
    for axis in 0..2 {
        if center[axis] + radius[axis] < f64::from(spec.world_min[axis])
            || center[axis] - radius[axis] > f64::from(spec.world_max[axis])
        {
            return Ok(None); // 不相交的世界圆不进入块加载。
        }
        let grid_min = ((center[axis] - radius[axis] - f64::from(spec.world_min[axis]))
            / spec.actual_spacing_meters[axis])
            .ceil();
        let grid_max = ((center[axis] + radius[axis] - f64::from(spec.world_min[axis]))
            / spec.actual_spacing_meters[axis])
            .floor();
        min[axis] = grid_min.clamp(0.0, spec.cells[axis] as f64) as u64;
        max[axis] = grid_max.clamp(0.0, spec.cells[axis] as f64) as u64;
        if min[axis] > max[axis] {
            return Ok(None); // 半径小于格距且圆内无作者格点时保持空笔。
        }
    }
    Ok(Some((min, max)))
}

/// 只收集圆内格点及世界距离衰减；候选矩形已通过固定预算。
fn affected_points(
    spec: &SparseHeightSpec, // 世界格点规格。
    min: [u64; 2],           // 已裁剪格点下界。
    max: [u64; 2],           // 已裁剪格点上界。
    center: [f64; 2],        // 世界笔刷圆心。
    radius: f64,             // 世界笔刷半径。
    scale: [f64; 2],
) -> Vec<(u64, u64, f32)> {
    let mut affected = Vec::new();
    for z in min[1]..=max[1] {
        for x in min[0]..=max[0] {
            let world = spec
                .grid_point_world(x, z)
                .expect("裁剪后的格点必在世界范围内");
            let distance = ((world[0] - center[0]) * scale[0]).hypot((world[1] - center[1]) * scale[1]);
            if distance < radius {
                affected.push((x, z, (1.0 - distance / radius) as f32)); // 和旧 F12 一样使用线性软边。
            }
        }
    }
    affected
}

#[cfg(test)] // 公共作者核心必须能在不启动 F12、ECS 或渲染器时独立完成编辑与重载。
mod tests;
