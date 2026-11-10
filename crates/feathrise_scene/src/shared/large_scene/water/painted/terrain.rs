//! 绘制水面后台构建共用的不可变量化高度输入。
//!
//! 【功能设计】F12 可以直接共享当前GLB派生草稿的 `Arc<Vec<u16>>`，正式运行时可以
//! 共享完整地形生成结果的 `Arc<[u16]>`；两者都只通过场景内容 crate 的权威三角剖分
//! 采样函数读取高度，不依赖磁盘R16文件。

use std::sync::Arc; // 高度网格快照只增加引用计数，不为每个水层复制整张派生数组。

use feathrise_scene_content::sample_heightfield_world; // 水域核心与地形、植被使用完全相同的三角剖分高度。

/// 同时兼容资产加载器切片和F12草稿Vec的零拷贝共享所有权。
#[derive(Clone, Debug)]
enum SharedTerrainHeights {
    Slice(Arc<[u16]>),  // 正式运行时从GLB完整派生结果取得的不可变切片。
    Vec(Arc<Vec<u16>>), // F12未保存草稿的GLB派生Vec快照。
}

impl SharedTerrainHeights {
    /// 统一借用连续量化高度样本，不暴露内部Arc容器差异。
    fn as_slice(&self) -> &[u16] {
        match self {
            Self::Slice(heights) => heights.as_ref(), // 资产切片可以直接借用。
            Self::Vec(heights) => heights.as_slice(), // 草稿Vec同样不发生复制。
        }
    }
}

/// 绘制水面稳定核心判定使用的完整不可变地形快照。
#[derive(Clone, Debug)]
pub struct PaintedWaterTerrainInput {
    heights: SharedTerrainHeights, // 全部水层只共享同一份GLB派生高度修订。
    resolution: [u32; 2],          // 顶点数量，单元数量为每轴减一。
    world_min: [f32; 2],           // 高度场世界X/Z最小端点。
    world_max: [f32; 2],           // 高度场世界X/Z最大端点。
    base_height: f32,              // u16零值对应的世界Y。
    height_scale: f32,             // u16最大值覆盖的米制高度跨度。
}

impl PaintedWaterTerrainInput {
    /// 从正式运行时的共享GLB派生高度切片建立受检地形输入。
    pub fn new(
        heights: Arc<[u16]>,  // 当前完整地形生成结果导出的稳定不可变样本。
        resolution: [u32; 2], // 地形顶点横纵数量。
        world_min: [f32; 2],  // 地形世界X/Z最小端点。
        world_max: [f32; 2],  // 地形世界X/Z最大端点。
        base_height: f32,     // u16零值对应世界Y。
        height_scale: f32,    // u16完整米制跨度。
    ) -> Result<Self, String> {
        Self::from_shared(
            SharedTerrainHeights::Slice(heights), // 保留原Arc切片所有权。
            resolution,                           // 传递受检顶点分辨率。
            world_min,                            // 传递世界最小端点。
            world_max,                            // 传递世界最大端点。
            base_height,                          // 传递高度解码基准。
            height_scale,                         // 传递高度解码跨度。
        )
    }

    /// 从F12未保存草稿的共享Vec建立零拷贝地形输入。
    pub fn from_shared_vec(
        heights: Arc<Vec<u16>>, // EditorSceneDraft当前GLB派生高度快照。
        resolution: [u32; 2],   // 地形顶点横纵数量。
        world_min: [f32; 2],    // 地形世界X/Z最小端点。
        world_max: [f32; 2],    // 地形世界X/Z最大端点。
        base_height: f32,       // u16零值对应世界Y。
        height_scale: f32,      // u16完整米制跨度。
    ) -> Result<Self, String> {
        Self::from_shared(
            SharedTerrainHeights::Vec(heights), // 保留草稿Vec的同一Arc分配。
            resolution,                         // 传递受检顶点分辨率。
            world_min,                          // 传递世界最小端点。
            world_max,                          // 传递世界最大端点。
            base_height,                        // 传递高度解码基准。
            height_scale,                       // 传递高度解码跨度。
        )
    }

    /// 校验全部尺寸和米制参数后建立统一输入。
    fn from_shared(
        heights: SharedTerrainHeights, // 任一受支持的共享量化高度容器。
        resolution: [u32; 2],          // 地形顶点横纵数量。
        world_min: [f32; 2],           // 地形世界X/Z最小端点。
        world_max: [f32; 2],           // 地形世界X/Z最大端点。
        base_height: f32,              // u16零值对应世界Y。
        height_scale: f32,             // u16完整米制跨度。
    ) -> Result<Self, String> {
        let expected = resolution[0] as usize * resolution[1] as usize; // 计算完整派生高度样本数。
        if resolution[0] < 2
            || resolution[1] < 2
            || heights.as_slice().len() != expected
            || !world_min
                .iter()
                .chain(&world_max)
                .all(|value| value.is_finite())
            || world_min[0] >= world_max[0]
            || world_min[1] >= world_max[1]
            || !base_height.is_finite()
            || !height_scale.is_finite()
            || height_scale <= 0.0
        {
            return Err("绘制水面的派生高度尺寸、世界范围或高度解码参数无效".to_owned()); // 坏输入不得退回平地猜测。
        }
        Ok(Self {
            heights,      // 保存已经验证的共享派生高度所有权。
            resolution,   // 保存权威顶点分辨率。
            world_min,    // 保存世界最小X/Z端点。
            world_max,    // 保存世界最大X/Z端点。
            base_height,  // 保存米制高度基准。
            height_scale, // 保存米制高度跨度。
        })
    }

    /// 按权威地形三角剖分采样指定世界X/Z高度。
    pub fn sample_world_height(&self, world_xz: [f32; 2]) -> Option<f32> {
        sample_heightfield_world(
            self.heights.as_slice(), // 读取当前不可变量化高度快照。
            self.resolution,         // 使用真实地形顶点分辨率。
            world_xz,                // 采样目标世界X/Z。
            self.world_min,          // 使用地形世界最小端点。
            self.world_max,          // 使用地形世界最大端点。
            self.base_height,        // 解码u16零值。
            self.height_scale,       // 解码u16米制跨度。
        )
    }

    /// 判断一个水域世界范围是否完全包含在当前地形输入内。
    pub(super) fn contains_world_bounds(
        &self,               // 只读受检地形范围。
        world_min: [f32; 2], // 水域世界最小X/Z端点。
        world_max: [f32; 2], // 水域世界最大X/Z端点。
    ) -> bool {
        world_min[0] >= self.world_min[0]
            && world_min[1] >= self.world_min[1]
            && world_max[0] <= self.world_max[0]
            && world_max[1] <= self.world_max[1]
    }

    /// 返回与作者岸坡口径一致的视觉岸带宽度。
    pub(super) fn visual_shore_band_meters(&self) -> f32 {
        let terrain_step_x =
            (self.world_max[0] - self.world_min[0]) / (self.resolution[0] - 1) as f32;
        let terrain_step_z =
            (self.world_max[1] - self.world_min[1]) / (self.resolution[1] - 1) as f32;
        2.0f32.max(2.0 * terrain_step_x.max(terrain_step_z))
        // 高分辨率场景至少两米；粗网格至少跨两个完整地形格，不在岸坡中途截断alpha。
    }

    /// 返回世界矩形内正式三角地形的精确最大值。
    ///
    /// 【修复设计】R8 水像素可能远小于一个派生高度格，不能把地形格远端、实际并未
    /// 落入水像素的高顶点算进来。这里逐个裁剪相交地形格，并检查裁剪矩形四角与正式
    /// `NE-SW` 三角对角线的交点；分片线性曲面在这些顶点上的最大值就是矩形内精确最大值。
    pub(super) fn maximum_surface_height(
        &self,              // 只读同一地形修订。
        rect_min: [f32; 2], // 待检查矩形世界X/Z最小端点。
        rect_max: [f32; 2], // 待检查矩形世界X/Z最大端点。
    ) -> Option<f32> {
        if !rect_min
            .iter()
            .chain(&rect_max)
            .all(|value| value.is_finite())
            || rect_min[0] < self.world_min[0]
            || rect_min[1] < self.world_min[1]
            || rect_max[0] > self.world_max[0]
            || rect_max[1] > self.world_max[1]
            || rect_min[0] >= rect_max[0]
            || rect_min[1] >= rect_max[1]
        {
            return None; // 越界矩形不能被误判成安全深水。
        }
        let grid_extent = [
            (self.resolution[0] - 1) as f32, // X轴高度单元数量。
            (self.resolution[1] - 1) as f32, // Z轴高度单元数量。
        ];
        let to_grid = |world: [f32; 2]| {
            [
                (world[0] - self.world_min[0]) / (self.world_max[0] - self.world_min[0])
                    * grid_extent[0], // 把世界X映射到连续高度网格。
                (world[1] - self.world_min[1]) / (self.world_max[1] - self.world_min[1])
                    * grid_extent[1], // 把世界Z映射到连续高度网格。
            ]
        };
        let grid_min = to_grid(rect_min); // 取得矩形最小角连续网格坐标。
        let grid_max = to_grid(rect_max); // 取得矩形最大角连续网格坐标。
        let min_x = grid_min[0].floor().clamp(0.0, grid_extent[0] - 1.0) as u32; // 最小角所在的第一个相交地形格。
        let min_z = grid_min[1].floor().clamp(0.0, grid_extent[1] - 1.0) as u32; // Z轴同样从实际相交地形格开始。
        let max_x_exclusive = grid_max[0].ceil().clamp(1.0, grid_extent[0]) as u32; // 精确网格线终点只属于左侧有效面积格。
        let max_z_exclusive = grid_max[1].ceil().clamp(1.0, grid_extent[1]) as u32; // 半开上界避免无面积的相邻格重复检查。
        let decode = |raw: u16| {
            self.base_height + raw as f32 / u16::MAX as f32 * self.height_scale // 将量化高度还原为世界Y。
        };
        let mut maximum = f32::NEG_INFINITY; // 从最低有限值开始汇聚实际表面最大高度。
        for z in min_z..max_z_exclusive {
            for x in min_x..max_x_exclusive {
                let local_min_x = (grid_min[0] - x as f32).clamp(0.0, 1.0); // 裁剪矩形在当前格内的局部西端。
                let local_max_x = (grid_max[0] - x as f32).clamp(0.0, 1.0); // 裁剪矩形在当前格内的局部东端。
                let local_min_z = (grid_min[1] - z as f32).clamp(0.0, 1.0); // 裁剪矩形在当前格内的局部北端。
                let local_max_z = (grid_max[1] - z as f32).clamp(0.0, 1.0); // 裁剪矩形在当前格内的局部南端。
                if local_min_x >= local_max_x || local_min_z >= local_max_z {
                    continue; // 浮点边界只能接触但没有面积时不参与本矩形最大值。
                }
                let row_width = self.resolution[0]; // 当前完整派生网格行宽供四角索引共用。
                let nw = decode(self.heights.as_slice()[(z * row_width + x) as usize]); // 当前格西北高度。
                let ne = decode(self.heights.as_slice()[(z * row_width + x + 1) as usize]); // 当前格东北高度。
                let sw = decode(self.heights.as_slice()[((z + 1) * row_width + x) as usize]); // 当前格西南高度。
                let se = decode(
                    self.heights.as_slice()[((z + 1) * row_width + x + 1) as usize], // 当前格东南高度。
                );
                let sample_local = |local_x: f32, local_z: f32| {
                    if local_x + local_z <= 1.0 {
                        nw + local_x * (ne - nw) + local_z * (sw - nw) // 与正式高度场西北三角完全相同。
                    } else {
                        se + (1.0 - local_z) * (ne - se) + (1.0 - local_x) * (sw - se) // 与正式东南三角完全相同。
                    }
                };
                for [local_x, local_z] in [
                    [local_min_x, local_min_z], // 裁剪矩形西北角。
                    [local_max_x, local_min_z], // 裁剪矩形东北角。
                    [local_min_x, local_max_z], // 裁剪矩形西南角。
                    [local_max_x, local_max_z], // 裁剪矩形东南角。
                ] {
                    maximum = maximum.max(sample_local(local_x, local_z)); // 单个三角片内的线性极值只会出现在多边形顶点。
                }
                for [local_x, local_z] in [
                    [local_min_x, 1.0 - local_min_x], // 对角线与西边相交。
                    [local_max_x, 1.0 - local_max_x], // 对角线与东边相交。
                    [1.0 - local_min_z, local_min_z], // 对角线与北边相交。
                    [1.0 - local_max_z, local_max_z], // 对角线与南边相交。
                ] {
                    if (local_min_x..=local_max_x).contains(&local_x)
                        && (local_min_z..=local_max_z).contains(&local_z)
                    {
                        maximum = maximum.max(sample_local(local_x, local_z)); // 折线脊的边界交点同样属于精确候选。
                    }
                }
            }
        }
        maximum.is_finite().then_some(maximum) // 有效矩形至少包含一个实际曲面候选点。
    }
}
