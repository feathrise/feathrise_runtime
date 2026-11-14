//! 场景高度作者精度与全局格点归属；不会建立按整图格点数量分配的数组。

use serde::{Deserialize, Serialize}; // 清单保留可独立编辑器读取的稳定数值合同。

/// 每个作者块最多管理 256 × 256 个逻辑单元。
pub const SPARSE_HEIGHT_TILE_CELLS: u64 = 256;

/// 最大场景每轴最多 256 个作者块，因此八级父页足以把整轴收敛到一个 256 单元页。
pub const SPARSE_HEIGHT_MAX_MIP_LEVEL: u8 = 8;

/// 与相机位置无关的作者块身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SparseHeightTileKey {
    pub x: u32, // 从场景 X 最小端开始的块列。
    pub z: u32, // 从场景 Z 最小端开始的块行。
}

/// 两轴作者格距和高度编码的权威规格。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SparseHeightSpec {
    pub world_min: [f32; 2],             // 世界 X/Z 最小端点。
    pub world_max: [f32; 2],             // 世界 X/Z 最大端点。
    pub requested_spacing_meters: f32,   // 作者选择的最大格距，不能随场景变大自动放粗。
    pub cells: [u64; 2],                 // 每轴逻辑单元数，8192m / 0.125m 时仍能表达 65536。
    pub actual_spacing_meters: [f64; 2], // 两轴独立均分，以便最后格点精确落在世界上界。
    pub base_height: f32,                // 仅用于兼容旧v2页的u16解码下界，不限制新作者高度。
    pub height_scale: f32,               // 仅用于兼容旧v2页的u16解码跨度，不是Clipmap雕刻上限。
}

impl SparseHeightSpec {
    /// 任何合法尺寸与格距组合都只计算标量元数据，不分配整图高度。
    pub fn new(
        world_min: [f32; 2],           // 沿用场景总清单的范围。
        world_max: [f32; 2],           // 最终格点必须命中这个端点。
        requested_spacing_meters: f32, // 允许 0.125..64 米。
        base_height: f32,              // 高度编码下界。
        height_scale: f32,             // 高度编码跨度。
    ) -> Result<Self, String> {
        if !requested_spacing_meters.is_finite()
            || !(0.125..=64.0).contains(&requested_spacing_meters)
        {
            return Err("稀疏地形作者格距必须在 0.125 至 64 米之间".into());
        }
        if !base_height.is_finite()
            || !height_scale.is_finite()
            || height_scale <= 0.0
            || !((f64::from(base_height))..=(f64::from(base_height) + f64::from(height_scale)))
                .contains(&0.0)
        {
            return Err("稀疏地形高度量程必须有限、为正且包含世界 Y=0".into());
        }
        let mut cells = [0_u64; 2]; // 只保留两轴格数，不建立格点网格。
        let mut actual_spacing_meters = [0.0_f64; 2]; // 矩形场景允许两轴末端间距不同。
        for axis in 0..2 {
            let extent = f64::from(world_max[axis]) - f64::from(world_min[axis]);
            // 【功能设计】高度地形统一采用 Clipmap，小模型大小的地形也使用同一分页源。
            // 一格仍覆盖完整边长；不因不足一页而向外扩大作者范围。
            if !extent.is_finite() || !(f64::from(0.01_f32)..=8192.0).contains(&extent) {
                return Err(format!("稀疏地形第{axis}轴尺寸必须在 0.01 至 8192 米之间"));
            }
            let count = (extent / f64::from(requested_spacing_meters)).ceil(); // 不因总图增大而提高最细格距。
            if !(1.0..=u64::MAX as f64).contains(&count) {
                return Err(format!("稀疏地形第{axis}轴格数无效"));
            }
            cells[axis] = count as u64; // 最大合法请求为 65536，远小于 u64 上限。
            actual_spacing_meters[axis] = extent / count; // 最后一格没有额外伸出边界。
        }
        Ok(Self {
            world_min,                // 与场景总清单同一坐标范围。
            world_max,                // 最后格点保持精确端点。
            requested_spacing_meters, // 可从清单还原作者原始请求。
            cells,                    // 与场景面积无关地占固定两个整数。
            actual_spacing_meters,    // 局部查询与 Blender 分块导入复用同一世界映射。
            base_height,              // 旧高度编码参数可以原样传给新格式。
            height_scale,             // 零高度仍在可雕刻量程内。
        })
    }

    /// 从清单的两轴格点数恢复同一拓扑，不能把均分后的X实际格距误当成作者请求格距。
    pub fn from_resolution(
        world_min: [f32; 2],
        world_max: [f32; 2],
        resolution: [u32; 2],
        base_height: f32,
        height_scale: f32,
    ) -> Result<Self, String> {
        if resolution.iter().any(|count| *count < 2) {
            return Err("Clipmap两轴分辨率必须至少包含两个格点".into());
        }
        let cells = resolution.map(|count| u64::from(count - 1));
        // ceil会让不整除边长的实际格距略小于0.125m；两轴共同限定可恢复请求，不能放粗或增加格数。
        let lower = (0..2).fold(0.125_f64, |spacing, axis| {
            let extent = f64::from(world_max[axis]) - f64::from(world_min[axis]);
            spacing.max(extent / cells[axis] as f64)
        });
        let mut spacing = lower as f32;
        if f64::from(spacing) < lower {
            spacing = spacing.next_up();
        }
        // 浮点除法在整数边界可能多出一个格；最多前进一个f32值并复核两轴，工作量固定。
        for _ in 0..2 {
            let spec = Self::new(world_min, world_max, spacing, base_height, height_scale)?;
            if spec.cells == cells {
                return Ok(spec);
            }
            spacing = spacing.next_up();
        }
        Err("Clipmap两轴分辨率与源范围、格距不一致".into())
    }

    /// 拒绝手工损坏的清单，不能让各消费者采用不同的格点映射。
    pub fn validate(&self) -> Result<(), String> {
        let expected = Self::new(
            self.world_min,
            self.world_max,
            self.requested_spacing_meters,
            self.base_height,
            self.height_scale,
        )?;
        if self.cells != expected.cells
            || self
                .actual_spacing_meters
                .iter()
                .zip(expected.actual_spacing_meters)
                .any(|(value, expected)| !value.is_finite() || (value - expected).abs() > 1e-9)
        {
            return Err("稀疏地形清单格数或实际格距与世界范围不一致".into());
        }
        Ok(())
    }

    /// 每轴世界固定作者块数；空白场景仅计算数量，不创建对应文件。
    pub fn tile_counts(&self) -> [u32; 2] {
        self.mip_tile_counts(0)
            .expect("基础作者层始终位于受支持的 Mip 范围") // 合法场景两轴各最多 256 块。
    }

    /// 当前场景把整轴收敛到单页所需的最高持久 Mip；空白图也只计算标量。
    pub fn max_mip_level(&self) -> u8 {
        let counts = self.tile_counts_without_mip(); // 避免通过 tile_counts 递归调用本函数。
        let mut level = 0_u8; // 单页场景无需派生父页。
        let mut longest = counts[0].max(counts[1]); // 矩形图按较长轴确定公共层数。
        while longest > 1 {
            longest = longest.div_ceil(2); // 每一级父页覆盖前一级的二乘二页。
            level += 1;
        }
        level.min(SPARSE_HEIGHT_MAX_MIP_LEVEL) // 规格验证保证实际不会超过八级。
    }

    /// 给定层级的全图逻辑单元数；最后一个单元仍精确落在场景世界上界。
    pub fn mip_cells(&self, level: u8) -> Option<[u64; 2]> {
        (level <= self.max_mip_level()).then(|| {
            let scale = 1_u64 << level; // level 最大八，不存在移位溢出。
            [self.cells[0].div_ceil(scale), self.cells[1].div_ceil(scale)]
        })
    }

    /// 给定层级的世界固定页数；每升一级都把前一级二乘二页收敛为一个父页。
    pub fn mip_tile_counts(&self, level: u8) -> Option<[u32; 2]> {
        self.mip_cells(level).map(|cells| {
            [
                cells[0].div_ceil(SPARSE_HEIGHT_TILE_CELLS) as u32,
                cells[1].div_ceil(SPARSE_HEIGHT_TILE_CELLS) as u32,
            ]
        })
    }

    /// 给定层级的页拥有格点数；内部共享边仍只归右侧或下侧页。
    pub fn mip_tile_shape(&self, level: u8, key: SparseHeightTileKey) -> Option<[u16; 2]> {
        let cells = self.mip_cells(level)?;
        let counts = self.mip_tile_counts(level)?;
        if key.x >= counts[0] || key.z >= counts[1] {
            return None;
        }
        let axis_shape = |axis: usize, tile_index: u32| {
            let start = u64::from(tile_index) * SPARSE_HEIGHT_TILE_CELLS;
            let owned_cells = (cells[axis] - start).min(SPARSE_HEIGHT_TILE_CELLS);
            (owned_cells + u64::from(tile_index == counts[axis] - 1)) as u16
        }; // 尾页额外拥有当前层的全图终点。
        Some([axis_shape(0, key.x), axis_shape(1, key.z)])
    }

    /// 当前层全局格点转唯一页和局部坐标，供派生页、运行时页表和测试共用。
    pub fn mip_tile_for_grid_point(
        &self,
        level: u8,
        x: u64,
        z: u64,
    ) -> Option<(SparseHeightTileKey, [u16; 2])> {
        let cells = self.mip_cells(level)?;
        if x > cells[0] || z > cells[1] {
            return None;
        }
        let counts = self.mip_tile_counts(level)?;
        let key = SparseHeightTileKey {
            x: (x / SPARSE_HEIGHT_TILE_CELLS).min(u64::from(counts[0] - 1)) as u32,
            z: (z / SPARSE_HEIGHT_TILE_CELLS).min(u64::from(counts[1] - 1)) as u32,
        };
        let local = [
            (x - u64::from(key.x) * SPARSE_HEIGHT_TILE_CELLS) as u16,
            (z - u64::from(key.z) * SPARSE_HEIGHT_TILE_CELLS) as u16,
        ];
        Some((key, local))
    }

    /// 当前层格点映射回基础作者格点；尾点用 clamp 精确命中不整除的场景边界。
    pub fn mip_grid_point_to_base(&self, level: u8, x: u64, z: u64) -> Option<[u64; 2]> {
        let cells = self.mip_cells(level)?;
        if x > cells[0] || z > cells[1] {
            return None;
        }
        let scale = 1_u64 << level;
        Some([
            x.saturating_mul(scale).min(self.cells[0]),
            z.saturating_mul(scale).min(self.cells[1]),
        ])
    }

    /// 一个基础作者页在指定层级的唯一祖先页，用于只重建脏链而不扫描整图。
    pub fn mip_ancestor_key(
        &self,
        base_key: SparseHeightTileKey,
        level: u8,
    ) -> Option<SparseHeightTileKey> {
        let base_counts = self.tile_counts_without_mip();
        if base_key.x >= base_counts[0] || base_key.z >= base_counts[1] {
            return None;
        }
        let counts = self.mip_tile_counts(level)?;
        Some(SparseHeightTileKey {
            x: (base_key.x >> level).min(counts[0] - 1),
            z: (base_key.z >> level).min(counts[1] - 1),
        })
    }

    /// 右边界和下边界格点归最后一个块，内部边界归右/下块，只有一个权威值。
    pub fn tile_for_grid_point(
        &self,
        x: u64, // 全图 X 格点，允许等于单元数。
        z: u64, // 全图 Z 格点，允许等于单元数。
    ) -> Option<(SparseHeightTileKey, [u16; 2])> {
        self.mip_tile_for_grid_point(0, x, z) // 基础层与派生层严格使用同一归属规则。
    }

    /// 块只存自己拥有的格点；末块额外拥有场景最外侧端点。
    pub fn tile_shape(&self, key: SparseHeightTileKey) -> Option<[u16; 2]> {
        self.mip_tile_shape(0, key) // 作者页只是层级零的持久页。
    }

    /// 全局格点转世界 X/Z，禁止用相机相对环带坐标改写作者位置。
    pub fn grid_point_world(&self, x: u64, z: u64) -> Option<[f64; 2]> {
        (x <= self.cells[0] && z <= self.cells[1]).then(|| {
            [
                f64::from(self.world_min[0]) + x as f64 * self.actual_spacing_meters[0],
                f64::from(self.world_min[1]) + z as f64 * self.actual_spacing_meters[1],
            ] // 精确端点在浮点误差内与场景清单一致。
        })
    }

    /// 隐式平地和新作者页都使用精确+0米的浮点位模式。
    pub fn flat_height_code(&self) -> u32 {
        0.0_f32.to_bits()
    }

    /// 作者源直接保存有限f32米制高度；创建时的旧编码域不再限制正、负雕刻。
    pub fn encode_height(&self, meters: f32) -> Result<u32, String> {
        if !meters.is_finite() {
            return Err("作者高度必须是有限米制数值".into());
        }
        Ok(if meters == 0.0 { 0 } else { meters.to_bits() }) // 统一正负零，整笔历史仍按位精确比较。
    }

    /// 新作者码是米制浮点位模式；输入文件、写入和历史候选须先验证有限性。
    pub fn decode_height(&self, code: u32) -> f32 {
        f32::from_bits(code)
    }

    /// v2没有逐页量程，必须使用该页所属清单的原量程解码，不能借当前默认值重解释旧图。
    pub fn decode_legacy_height(&self, code: u16) -> f32 {
        let flat = (((-f64::from(self.base_height)) / f64::from(self.height_scale)) * 65535.0).round() as u16;
        if code == flat {
            0.0 // 保留旧版将量化中点精确解释成零的规则。
        } else {
            self.base_height + (f32::from(code) / 65535.0) * self.height_scale
        }
    }

    /// 基础页数的无递归实现，只供层级标量计算使用。
    fn tile_counts_without_mip(&self) -> [u32; 2] {
        [
            self.cells[0].div_ceil(SPARSE_HEIGHT_TILE_CELLS) as u32,
            self.cells[1].div_ceil(SPARSE_HEIGHT_TILE_CELLS) as u32,
        ]
    }
}
