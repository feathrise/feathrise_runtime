//! R8 与量化地形高度联合判定的稳定水域核心，以及有限米制视觉岸带。
//!
//! 【功能设计】源 R8 只决定候选水域，量化河床再拒绝水位附近和水位以上的单元；
//! 网格只围绕真实深水核心扩展固定米数，因此整图误涂 R8 也不会在远处平地生成水面。

use std::cmp::Ordering; // Dijkstra 队列需要按最短米制距离稳定弹出顶点。
use std::collections::BinaryHeap; // 岸带距离只在最多 257² 个顶点的后台网格上计算。

use super::terrain::PaintedWaterTerrainInput; // 只读同一地形修订的量化高度快照。

/// 稳定核心沿用既有 32/255 有效覆盖门槛；该门槛只负责筛选安全几何种子。
pub(super) const STABLE_CORE_COVERAGE_THRESHOLD: u8 = 32;
/// 源 R8 单元上的半开包围盒。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SourceBounds {
    pub(super) min: [u32; 2],           // 包含的最小 X/Z 单元。
    pub(super) max_exclusive: [u32; 2], // 不包含的最大 X/Z 单元。
}

impl SourceBounds {
    /// 返回包围盒在源 R8 上的单元数量。
    pub(super) fn size(self) -> [u32; 2] {
        [
            self.max_exclusive[0] - self.min[0], // X 轴半开区间长度。
            self.max_exclusive[1] - self.min[1], // Z 轴半开区间长度。
        ]
    }

    /// 按两个轴各自的源单元数量扩张，并夹紧到作者画布内。
    pub(super) fn expanded(self, radius: [u32; 2], resolution: [u32; 2]) -> Self {
        Self {
            min: [
                self.min[0].saturating_sub(radius[0]), // 地图西侧不能发生无符号下溢。
                self.min[1].saturating_sub(radius[1]), // 地图北侧同样安全夹紧。
            ],
            max_exclusive: [
                self.max_exclusive[0]
                    .saturating_add(radius[0])
                    .min(resolution[0]), // 东侧不越过 R8 宽度。
                self.max_exclusive[1]
                    .saturating_add(radius[1])
                    .min(resolution[1]), // 南侧不越过 R8 高度。
            ],
        }
    }
}

/// 源 R8 分辨率上的稳定可见核心。
pub(super) struct StableWaterCore {
    pub(super) coverage: Vec<u8>, // 只有 R8 与稳定深水相交的单元保留原覆盖值。
    pub(super) bounds: Option<SourceBounds>, // 无稳定核心时保持 None，不退回整图水面。
}

/// 将一个源单元加入半开包围盒。
fn include_cell(bounds: &mut Option<SourceBounds>, x: u32, z: u32) {
    *bounds = Some(match *bounds {
        Some(current) => SourceBounds {
            min: [current.min[0].min(x), current.min[1].min(z)], // 扩张最小角。
            max_exclusive: [
                current.max_exclusive[0].max(x + 1), // 半开上界包含当前 X 单元。
                current.max_exclusive[1].max(z + 1), // 半开上界包含当前 Z 单元。
            ],
        },
        None => SourceBounds {
            min: [x, z],                   // 首个稳定单元直接建立最小角。
            max_exclusive: [x + 1, z + 1], // 首个稳定单元占一格。
        },
    });
}

/// 在源分辨率逐单元求 `R8 候选 ∩ 量化地形稳定深水`。
pub(super) fn build_stable_water_core(
    mask: &[u8],                        // 完整无 Mip R8 作者覆盖。
    resolution: [u32; 2],               // R8 单元宽高。
    world_min: [f32; 2],                // 画布世界 X/Z 最小端点。
    world_max: [f32; 2],                // 画布世界 X/Z 最大端点。
    terrain: Option<&PaintedWaterTerrainInput>, // 同代真实高度；None仅用于独立静水，没有假河床。
    water_level: f32,                   // 当前层静止水位世界 Y。
    minimum_depth: f32,                 // 当前外观最大负波位移加互动波与安全余量。
) -> StableWaterCore {
    let expected = resolution[0] as usize * resolution[1] as usize; // 调用前已经校验乘积和切片长度。
    let mut stable_coverage = vec![0u8; expected]; // 非稳定单元严格保持零。
    let mut bounds = None; // 首个稳定可见单元到来时再建立局部范围。
    let world_size = [world_max[0] - world_min[0], world_max[1] - world_min[1]]; // 两轴米制跨度。
    let source_step = [
        world_size[0] / resolution[0] as f32, // 一个 R8 像素覆盖的世界 X 米数。
        world_size[1] / resolution[1] as f32, // 一个 R8 像素覆盖的世界 Z 米数。
    ];
    for z in 0..resolution[1] {
        for x in 0..resolution[0] {
            let index = (z * resolution[0] + x) as usize; // R8 行主序下标。
            let coverage = mask[index]; // 保留作者强度供水面顶点 alpha 使用。
            if coverage < STABLE_CORE_COVERAGE_THRESHOLD {
                continue; // 擦除残留和空白像素都不触发昂贵高度范围检查。
            }
            let cell_min = [
                world_min[0] + x as f32 * source_step[0], // 当前像素世界西端。
                world_min[1] + z as f32 * source_step[1], // 当前像素世界北端。
            ];
            let cell_max = [
                world_min[0] + (x + 1) as f32 * source_step[0], // 当前像素世界东端。
                world_min[1] + (z + 1) as f32 * source_step[1], // 当前像素世界南端。
            ];
            if let Some(terrain) = terrain {
                let Some(maximum_terrain_height) = terrain.maximum_surface_height(cell_min, cell_max)
                else {
                    continue; // 越界或坏高度输入不能授权水面核心。
                };
                if water_level - maximum_terrain_height < minimum_depth {
                    continue; // 有真实地形的既有水面保留安全核心，不改变其防穿模策略。
                }
            }
            stable_coverage[index] = coverage; // 只保留真实深河床上的作者覆盖。
            include_cell(&mut bounds, x, z); // 活跃网格稍后只围绕这些单元构建。
        }
    }
    StableWaterCore {
        coverage: stable_coverage, // 返回可用于有界最大值降采样的稳定覆盖。
        bounds,                    // 返回局部核心范围或 None。
    }
}

/// 根据米制岸带计算源单元扩张半径；额外一格覆盖顶点插值边界。
pub(super) fn expanded_active_bounds(
    core: SourceBounds,     // 稳定核心最小源包围盒。
    resolution: [u32; 2],   // 完整 R8 单元宽高。
    world_min: [f32; 2],    // 完整地图世界最小端点。
    world_max: [f32; 2],    // 完整地图世界最大端点。
    shore_band_meters: f32, // 与当前派生高度格距一致的唯一岸带宽度。
) -> SourceBounds {
    let source_step = [
        (world_max[0] - world_min[0]) / resolution[0] as f32, // X 轴每源像素米数。
        (world_max[1] - world_min[1]) / resolution[1] as f32, // Z 轴每源像素米数。
    ];
    let radius = [
        (shore_band_meters / source_step[0]).ceil() as u32 + 1, // 保留完整西东岸带和一个插值保护格。
        (shore_band_meters / source_step[1]).ceil() as u32 + 1, // 保留完整南北岸带和一个插值保护格。
    ];
    core.expanded(radius, resolution) // 地图边缘安全夹紧。
}

/// 把完整源核心最大值降采样到局部有界网格，窄河槽不会因点采样丢失。
pub(super) fn downsample_stable_coverage(
    stable: &[u8],               // 完整源分辨率稳定覆盖。
    source_resolution: [u32; 2], // 完整 R8 单元宽高。
    active: SourceBounds,        // 仅围绕稳定核心的局部源范围。
    output_cells: [u32; 2],      // 每轴不超过固定预算的网格单元数。
) -> Vec<u8> {
    let active_size = active.size(); // 局部分区只覆盖真实核心附近。
    let mut output = vec![0u8; output_cells[0] as usize * output_cells[1] as usize];
    for output_z in 0..output_cells[1] {
        let source_min_z = active.min[1]
            + (u64::from(output_z) * u64::from(active_size[1]) / u64::from(output_cells[1])) as u32; // 当前输出格对应的源 Z 起点。
        let source_max_z = (active.min[1]
            + (u64::from(output_z + 1) * u64::from(active_size[1]) / u64::from(output_cells[1]))
                as u32)
            .max(source_min_z + 1); // 先变成全局下标，再保证半开区间至少覆盖一个源格。
        for output_x in 0..output_cells[0] {
            let source_min_x = active.min[0]
                + (u64::from(output_x) * u64::from(active_size[0]) / u64::from(output_cells[0]))
                    as u32; // 当前输出格对应的源 X 起点。
            let source_max_x = (active.min[0]
                + (u64::from(output_x + 1) * u64::from(active_size[0]) / u64::from(output_cells[0]))
                    as u32)
                .max(source_min_x + 1); // 先变成全局下标，再保证半开区间至少覆盖一个源格。
            let mut maximum = 0u8; // 最大值保留比输出网格更窄的稳定水笔划。
            for source_z in source_min_z..source_max_z.min(active.max_exclusive[1]) {
                for source_x in source_min_x..source_max_x.min(active.max_exclusive[0]) {
                    maximum =
                        maximum.max(stable[(source_z * source_resolution[0] + source_x) as usize]); // 汇聚当前局部分区全部稳定源像素。
                }
            }
            output[(output_z * output_cells[0] + output_x) as usize] = maximum; // 写入当前有界单元覆盖。
        }
    }
    output
}

/// Dijkstra 最小堆节点；Rust BinaryHeap 默认最大堆，因此距离比较反向。
#[derive(Clone, Copy, Debug)]
struct QueueEntry {
    distance: f32, // 从最近稳定核心顶点传播的米制距离。
    vertex: usize, // 局部规则网格顶点下标。
}

impl PartialEq for QueueEntry {
    fn eq(&self, other: &Self) -> bool {
        self.vertex == other.vertex && self.distance.to_bits() == other.distance.to_bits()
    }
}

impl Eq for QueueEntry {}

impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueueEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .distance
            .total_cmp(&self.distance) // 更短距离在最大堆中拥有更高优先级。
            .then_with(|| other.vertex.cmp(&self.vertex)) // 同距离按下标稳定排序，保证测试确定性。
    }
}

/// 读取一个网格顶点相邻的最多四个核心单元的最大覆盖。
fn adjacent_core_max(core: &[u8], cells: [u32; 2], vertex_x: u32, vertex_z: u32) -> u8 {
    let mut maximum = 0u8; // 地图角落也能安全从零开始汇聚。
    let min_x = vertex_x.saturating_sub(1); // 左侧相邻单元或零。
    let max_x = vertex_x.min(cells[0].saturating_sub(1)); // 右侧相邻单元或末格。
    let min_z = vertex_z.saturating_sub(1); // 上侧相邻单元或零。
    let max_z = vertex_z.min(cells[1].saturating_sub(1)); // 下侧相邻单元或末格。
    for z in min_z..=max_z {
        for x in min_x..=max_x {
            maximum = maximum.max(core[(z * cells[0] + x) as usize]); // 任一稳定核心邻格即可成为岸带源。
        }
    }
    maximum
}

/// 在局部网格顶点上生成核心 alpha 与向外两米平滑归零的视觉岸带。
pub(super) fn build_shore_vertex_alpha(
    core: &[u8],            // 已最大值降采样的稳定核心单元。
    cells: [u32; 2],        // 局部规则网格单元数。
    world_size: [f32; 2],   // 局部网格实际 X/Z 米制跨度。
    shore_band_meters: f32, // active裁剪与距离传播共用同一个米制值。
) -> Vec<f32> {
    let vertex_width = cells[0] + 1; // 共享顶点横向数量。
    let vertex_height = cells[1] + 1; // 共享顶点纵向数量。
    let vertex_count = vertex_width as usize * vertex_height as usize; // 固定预算内最多 257²。
    let step = [
        world_size[0] / cells[0] as f32, // 横向队列边权采用真实米数。
        world_size[1] / cells[1] as f32, // 纵向队列边权采用真实米数。
    ];
    let mut distance = vec![f32::INFINITY; vertex_count]; // 非核心顶点等待最短路传播。
    let mut source_alpha = vec![0.0f32; vertex_count]; // 保存最近核心的原 R8 不透明度。
    let mut queue = BinaryHeap::new(); // 只在后台任务内分配一次有界优先队列。
    for z in 0..vertex_height {
        for x in 0..vertex_width {
            let coverage = adjacent_core_max(core, cells, x, z); // 核心单元的四角都是零距离源。
            if coverage < STABLE_CORE_COVERAGE_THRESHOLD {
                continue; // 非核心顶点由队列传播有限岸带。
            }
            let vertex = (z * vertex_width + x) as usize; // 当前规则顶点下标。
            distance[vertex] = 0.0; // 核心本体不衰减。
            source_alpha[vertex] = coverage as f32 / 255.0; // 保留作者覆盖强度。
            queue.push(QueueEntry {
                distance: 0.0,
                vertex,
            }); // 所有核心顶点共同作为多源最短路起点。
        }
    }
    while let Some(entry) = queue.pop() {
        if entry.distance > distance[entry.vertex] {
            continue; // 丢弃已被更短路径替代的旧队列项。
        }
        let x = entry.vertex as u32 % vertex_width; // 从行主序下标恢复顶点 X。
        let z = entry.vertex as u32 / vertex_width; // 从行主序下标恢复顶点 Z。
        for (next_x, next_z, edge_meters) in [
            (x.saturating_sub(1), z, step[0]),            // 西侧相邻顶点。
            ((x + 1).min(vertex_width - 1), z, step[0]),  // 东侧相邻顶点。
            (x, z.saturating_sub(1), step[1]),            // 北侧相邻顶点。
            (x, (z + 1).min(vertex_height - 1), step[1]), // 南侧相邻顶点。
        ] {
            let next = (next_z * vertex_width + next_x) as usize; // 相邻顶点行主序下标。
            if next == entry.vertex {
                continue; // 边界夹紧产生的自环无需处理。
            }
            let next_distance = entry.distance + edge_meters; // 累加真实米制曼哈顿距离。
            if next_distance > shore_band_meters {
                continue; // 有限岸带外绝不产生可见 alpha 或额外队列工作。
            }
            let improves_distance = next_distance + f32::EPSILON < distance[next];
            let ties_with_stronger_alpha = (next_distance - distance[next]).abs() <= f32::EPSILON
                && source_alpha[entry.vertex] > source_alpha[next];
            if !improves_distance && !ties_with_stronger_alpha {
                continue; // 当前路径既不更近也不保留更强作者覆盖。
            }
            distance[next] = next_distance; // 采用新的最近核心距离。
            source_alpha[next] = source_alpha[entry.vertex]; // 岸带继承该核心边缘的作者强度。
            queue.push(QueueEntry {
                distance: next_distance,
                vertex: next,
            }); // 继续向固定半径内传播。
        }
    }
    distance
        .into_iter()
        .zip(source_alpha)
        .map(|(distance, source_alpha)| {
            if !distance.is_finite() || distance >= shore_band_meters {
                return 0.0; // 岸带边界及之外严格透明，避免网格矩形轮廓。
            }
            let linear = 1.0 - distance / shore_band_meters; // 从核心一到外边界零。
            let smooth = linear * linear * (3.0 - 2.0 * linear); // 平滑 alpha 一阶导数。
            source_alpha * smooth // 核心保留 R8 强度，外带在此基础上衰减。
        })
        .collect()
}
