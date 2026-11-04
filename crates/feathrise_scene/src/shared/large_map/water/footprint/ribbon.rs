//! 样条水面同源三角形索引、静止接触与雨花动态高度采样。

use super::super::types::DynamicWaterGpuFrame;
use bevy::math::Vec3Swizzles; // XZ投影保留有界查询坐标。
use bevy::prelude::{Vec2, Vec3}; // 纯几何构建与采样值。 // 近景高度复用实际GPU当前参数。

const MAX_RIBBON_FOOTPRINT_GRID_AXIS: u32 = 128; // 单条河的稀疏查询格每轴固定上限，避免损坏样条创建无界索引。

/// 一次斜河水平查询返回的真实局部水面高度和插值作者深度。
#[derive(Clone, Copy, Debug, PartialEq)] // 小型采样值只在玩家接触系统的当前帧栈上传递。
pub(in super::super) struct RibbonWaterSample {
    pub(in super::super) surface_local_y: f32, // 玩家局部XZ所在三角形插值得到的真实静止水面Y。
    pub(in super::super) author_depth: f32,    // 同一重心权重插值得到的作者水深。
} // 结束斜河足迹采样值定义。

/// 带状河流使用固定上限二维格索引定位实际渲染截面，不在稳定帧遍历完整样条。
#[derive(Clone, Debug)] // 完成值随水面组件只读持有，热更新时整批原子替换。
pub(in super::super) struct RibbonWaterFootprint {
    local_min: Vec2,                  // 全部实际带状顶点的局部XZ最小角。
    local_max: Vec2,                  // 全部实际带状顶点的局部XZ最大角。
    grid_size: [u32; 2],              // 依据带状包围盒长宽比分配且每轴不超过一百二十八格。
    cell_offsets: std::sync::Arc<[u32]>,           // CSR每格候选纵向段起点，长度固定为格数加一。
    segment_indices: std::sync::Arc<[u32]>,        // CSR连续候选纵向段身份，同一格最多六十四项。
    vertex_width: usize,              // 每个纵向截面的实际横向顶点数，与渲染索引行宽一致。
    vertices: std::sync::Arc<[Vec3]>, // 截面主序保存渲染网格的全部静止顶点，不以两岸四边形近似中间格。
    section_depths: std::sync::Arc<[f32]>, // 每个纵向截面的作者深度，按实际小三角插值。
    wave_sections: Option<std::sync::Arc<[Vec3]>>, // 每截面累计米、作者流速和解码样式；旧构造器可以不提供。
} // 结束带状河流互动足迹定义。

impl RibbonWaterFootprint {
    /// 从渲染网格的同一批截面主序顶点建立固定预算查询格；过密自交样条明确失败。
    pub(in super::super) fn from_ribbon_vertices(
        vertices: Vec<Vec3>,      // 与Mesh位置逐顶点相同的静止几何。
        section_depths: Vec<f32>, // 与纵向截面逐项相同的作者深度。
        vertex_width: usize,      // 与Mesh索引构建使用相同的横向行宽。
    ) -> Result<Self, String> {
        if vertex_width < 2 || vertices.len() % vertex_width != 0 {
            return Err("带状水面互动顶点行宽或数量无效".to_owned()); // 不完整行无法复现渲染小三角。
        }
        let section_count = vertices.len() / vertex_width; // 完整行数就是实际纵向截面数。
        if section_count < 2 || section_depths.len() != section_count {
            return Err("带状水面互动截面或作者深度数量无效".to_owned()); // 至少两行且深度必须逐行对应。
        }
        if !section_depths
            .iter()
            .all(|depth| depth.is_finite() && *depth > 0.0)
            || !vertices.iter().all(|point| point.is_finite())
        {
            return Err("带状水面互动顶点包含无效坐标或深度".to_owned()); // 坏完成值不得进入玩法接触组件。
        }
        let mut local_min = Vec2::splat(f32::INFINITY); // 从正无穷开始收集全部实际顶点最小角。
        let mut local_max = Vec2::splat(f32::NEG_INFINITY); // 从负无穷开始收集全部实际顶点最大角。
        for point in &vertices {
            local_min = local_min.min(point.xz()); // XZ最小角覆盖实际每个横向格顶点。
            local_max = local_max.max(point.xz()); // XZ最大角覆盖实际每个横向格顶点。
        }
        let size = (local_max - local_min).max(Vec2::splat(0.001)); // 极窄河段仍保留有限映射分母。
        let segment_count = section_count - 1; // 相邻两行全部横向小格共同形成一个候选纵向段。
        let aspect = (size.x / size.y).clamp(1.0 / 128.0, 128.0); // 限制极端长宽比对另一轴格数的挤压。
        let grid_x = ((segment_count as f32 * aspect).sqrt().ceil() as u32)
            .clamp(1, MAX_RIBBON_FOOTPRINT_GRID_AXIS); // 长河优先沿主跨度获得更多查询格。
        let grid_z = ((segment_count as f32 / aspect).sqrt().ceil() as u32)
            .clamp(1, MAX_RIBBON_FOOTPRINT_GRID_AXIS); // 横向仍至少保留一个合法查询格。
        let grid_size = [grid_x, grid_z]; // 固定保存本次受检二维格尺寸。
        let cell_count = grid_x as usize * grid_z as usize; // 两轴硬上限保证乘积不会溢出或无界分配。
        let mut cells = vec![Vec::<u32>::new(); cell_count]; // 后台临时格只在纯构建任务中分配。
        for segment_index in 0..segment_count {
            let first_vertex = segment_index * vertex_width; // 当前候选段第一行在连续顶点中的起点。
            let end_vertex = first_vertex + vertex_width * 2; // 相邻两行完整覆盖所有实际横向小格。
            let segment_vertices = &vertices[first_vertex..end_vertex]; // 已由完整行校验保证切片合法。
            let segment_min = segment_vertices
                .iter()
                .fold(Vec2::splat(f32::INFINITY), |minimum, point| {
                    minimum.min(point.xz())
                }); // 全部实际横向顶点共同确定投影包围盒最小角。
            let segment_max = segment_vertices
                .iter()
                .fold(Vec2::splat(f32::NEG_INFINITY), |maximum, point| {
                    maximum.max(point.xz())
                }); // 全部实际横向顶点共同确定投影包围盒最大角。
            let min_cell = footprint_cell(segment_min, local_min, size, grid_size); // 包围盒最小角映射到合法格索引。
            let max_cell = footprint_cell(segment_max, local_min, size, grid_size); // 包围盒最大角映射到合法格索引。
            for z in min_cell[1]..=max_cell[1] {
                for x in min_cell[0]..=max_cell[0] {
                    let cell_index = (z * grid_x + x) as usize; // 二维格使用确定的行主序身份。
                    let cell = &mut cells[cell_index]; // 只修改当前后台临时候选表。
                    cell.push(segment_index as u32); // 当前实际带状段进入所有相交查询格。
                }
            }
        }
        let mut cell_offsets = Vec::with_capacity(cell_count + 1); // CSR偏移一次性按最终格数预分配。
        let mut segment_indices = Vec::new(); // CSR段表仅移动已经受限的临时候选身份。
        cell_offsets.push(0); // 第一格从连续段表零位置开始。
        for cell in cells {
            segment_indices.extend(cell); // 保持格顺序和段顺序确定，便于测试复现。
            cell_offsets.push(u32::try_from(segment_indices.len())
                .map_err(|_| "河流互动空间索引无法由U32偏移表示".to_owned())?); // 真实格式边界，不能截断偏移。
        }
        Ok(Self {
            local_min,           // 发布完整带状包围盒最小角。
            local_max,           // 发布完整带状包围盒最大角。
            grid_size,           // 发布固定预算查询格尺寸。
            cell_offsets: cell_offsets.into(),        // 发布完整CSR偏移数组。
            segment_indices: segment_indices.into(),     // 发布完整CSR候选段数组。
            vertex_width,        // 发布与实际Mesh索引相同的横向行宽。
            vertices: vertices.into(),            // 发布与实际Mesh位置逐顶点相同的静止几何。
            section_depths: section_depths.into(),      // 发布逐纵向截面的作者深度。
            wave_sections: None, // 未提供真实UV来源的足迹不授权动态水花。
        })
    }

    /// 按局部XZ只检查所在查询格的有界候选段，并返回最高可见斜河表面。
    pub(in super::super) fn sample(&self, local_xz: Vec2) -> Option<RibbonWaterSample> {
        self.sample_height(local_xz, |_, position| position.y) // 既有玩家接触继续使用静止表面。
    }

    /// 后台移交与Mesh UV0/UV1同源的逐截面参数，不复制整份Mesh。
    pub(in super::super) fn with_wave_sections(
        mut self,
        sections: Vec<Vec3>,
    ) -> Result<Self, String> {
        if sections.len() != self.section_depths.len()
            || !sections
                .iter()
                .all(|s| s.is_finite() && s.y >= 0.0 && (0.0..=1.0).contains(&s.z))
        {
            return Err("河流波形截面必须与实际Mesh对应且不得包含瀑布样式".to_owned()); // 错位或幕布不得授权雨花。
        }
        self.wave_sections = Some(sections.into()); // 移交逐截面轻量参数，与足迹共同原子发布。
        Ok(self) // 保持链式后台构造，不修改活动资源。
    }

    /// 只对命中三角的真实顶点求高度，稳定帧仍使用现有CSR固定候选预算。
    fn sample_height(
        &self,
        local_xz: Vec2,
        height: impl Fn(usize, Vec3) -> f32,
    ) -> Option<RibbonWaterSample> {
        if !local_xz.is_finite()
            || local_xz.x < self.local_min.x
            || local_xz.x > self.local_max.x
            || local_xz.y < self.local_min.y
            || local_xz.y > self.local_max.y
        {
            return None; // 坏坐标与完整带状包围盒外位置不能授权玩家水波。
        }
        let size = (self.local_max - self.local_min).max(Vec2::splat(0.001)); // 使用与构建阶段相同的安全分母。
        let cell = footprint_cell(local_xz, self.local_min, size, self.grid_size); // 常数次映射到唯一查询格。
        let cell_index = (cell[1] * self.grid_size[0] + cell[0]) as usize; // 取得CSR行主序格身份。
        let start = *self.cell_offsets.get(cell_index)? as usize; // 损坏完成值安全退化为不命中。
        let end = *self.cell_offsets.get(cell_index + 1)? as usize; // 读取当前格候选段结束位置。
        let mut best = None; // 多段投影重叠时选择最高实际水面，避免下层河面穿透授权。
        for segment_index in self.segment_indices.get(start..end)? {
            let segment_index = *segment_index as usize; // CSR身份转换为受检连续行下标。
            let upstream_start = segment_index.checked_mul(self.vertex_width)?; // 当前上游行起点。
            let downstream_start = upstream_start.checked_add(self.vertex_width)?; // 当前下游行起点。
            let upstream_depth = *self.section_depths.get(segment_index)?; // 当前上游截面作者深度。
            let downstream_depth = *self.section_depths.get(segment_index + 1)?; // 当前下游截面作者深度。
            for lateral_index in 0..self.vertex_width - 1 {
                let upstream_first = *self.vertices.get(upstream_start + lateral_index)?; // 当前小格上游第一角。
                let upstream_second = *self.vertices.get(upstream_start + lateral_index + 1)?; // 当前小格上游第二角。
                let downstream_first = *self.vertices.get(downstream_start + lateral_index)?; // 当前小格下游第一角。
                let downstream_second = *self.vertices.get(downstream_start + lateral_index + 1)?; // 当前小格下游第二角。
                let triangles = [
                    [upstream_first, downstream_first, downstream_second], // 与Mesh第一片小三角的角点和绕序一致。
                    [upstream_first, downstream_second, upstream_second], // 与Mesh第二片小三角的对角线一致。
                ];
                let depths = [
                    [upstream_depth, downstream_depth, downstream_depth], // 第一片按角点所属纵向截面插值深度。
                    [upstream_depth, downstream_depth, upstream_depth], // 第二片保持完全相同的Mesh三角剖分。
                ];
                let vertex_indices = [
                    [
                        upstream_start + lateral_index,
                        downstream_start + lateral_index,
                        downstream_start + lateral_index + 1,
                    ], // 第一片复用Mesh角点。
                    [
                        upstream_start + lateral_index,
                        downstream_start + lateral_index + 1,
                        upstream_start + lateral_index + 1,
                    ], // 第二片复用Mesh对角线。
                ];
                for ((triangle, triangle_depths), indices) in
                    triangles.into_iter().zip(depths).zip(vertex_indices)
                {
                    let Some(weights) = barycentric_xz(local_xz, triangle) else {
                        continue; // 当前实际小三角不覆盖查询点时继续检查同一纵向段其余小格。
                    };
                    let surface_local_y = height(indices[0], triangle[0]) * weights[0]
                        + height(indices[1], triangle[1]) * weights[1]
                        + height(indices[2], triangle[2]) * weights[2]; // 先采样顶点再按GPU三角片面插值。
                    let author_depth = triangle_depths[0] * weights[0]
                        + triangle_depths[1] * weights[1]
                        + triangle_depths[2] * weights[2]; // 作者深度与水面高度严格使用相同小三角。
                    let sample = RibbonWaterSample {
                        surface_local_y, // 发布当前位置真实斜河表面高度。
                        author_depth,    // 发布当前位置插值作者水深。
                    };
                    if best.is_none_or(|current: RibbonWaterSample| {
                        sample.surface_local_y > current.surface_local_y
                    }) {
                        best = Some(sample); // 重叠投影选择最高表面作为玩家脚下可见水体。
                    }
                }
            }
        }
        best // 返回有界候选中实际命中的最高斜河采样。
    }
}

/// 把局部XZ映射到固定查询格，并把最大端点收敛到最后一个合法格。
fn footprint_cell(point: Vec2, local_min: Vec2, size: Vec2, grid_size: [u32; 2]) -> [u32; 2] {
    let ratio = ((point - local_min) / size).clamp(Vec2::ZERO, Vec2::ONE); // 包围盒内坐标归一化到零至一。
    [
        (ratio.x * grid_size[0] as f32)
            .floor()
            .min(grid_size[0].saturating_sub(1) as f32) as u32, // 映射X并处理最大边界。
        (ratio.y * grid_size[1] as f32)
            .floor()
            .min(grid_size[1].saturating_sub(1) as f32) as u32, // 映射Z并处理最大边界。
    ]
}

/// 在XZ投影中计算点对三角形的重心权重；退化幕布投影不会被误当成可行走水面。
fn barycentric_xz(point: Vec2, triangle: [Vec3; 3]) -> Option<[f32; 3]> {
    let a = triangle[0].xz(); // 第一角点压到互动XZ平面。
    let b = triangle[1].xz(); // 第二角点压到互动XZ平面。
    let c = triangle[2].xz(); // 第三角点压到互动XZ平面。
    let denominator = (b.y - c.y) * (a.x - c.x) + (c.x - b.x) * (a.y - c.y); // 计算有向投影面积的两倍。
    if denominator.abs() <= 0.000001 {
        return None; // 近竖直瀑布的退化XZ投影绝不授权水平踏水。
    }
    let first = ((b.y - c.y) * (point.x - c.x) + (c.x - b.x) * (point.y - c.y)) / denominator; // 第一角重心权重。
    let second = ((c.y - a.y) * (point.x - c.x) + (a.x - c.x) * (point.y - c.y)) / denominator; // 第二角重心权重。
    let third = 1.0 - first - second; // 第三角权重由总和为一得到。
    (first >= -0.00001 && second >= -0.00001 && third >= -0.00001).then_some([first, second, third]) // 只接受三角形内部和浮点容差边界。
}

/// 按实际GPU顶点波高恢复雨花发射面；河流仅Y位移，XZ索引始终与渲染相同。
pub(in super::super) fn ribbon_splash_position(
    footprint: &RibbonWaterFootprint,    // 已原子采用的实际河面索引。
    frame: &DynamicWaterGpuFrame,        // 当前水材质固定Uniform。
    elapsed: f32,                        // 与Render Globals相同的3600秒视觉时钟。
    point: Vec2,                         // 水体局部XZ候选落点。
    player_height: impl Fn(Vec2) -> f32, // 共用八槽玩家波已限幅高度，只读不修改波池。
) -> Option<Vec3> {
    let sections = footprint.wave_sections.as_ref()?; // 无真实UV来源时不猜测波高。
    let sample = footprint.sample_height(point, |index, position| {
        let section = sections[index / footprint.vertex_width]; // 取与Mesh同源的累计米和样式。
        let lateral = (index % footprint.vertex_width) as f32 / (footprint.vertex_width - 1) as f32; // 重建Mesh UV0.y。
        let rapid = section.z.clamp(0.0, 1.0); // 构造校验已排除瀑布。
        let frequency = 0.105 + rapid * 0.055; // GPU主波沿程频率。
        let crossing_frequency = 0.045 + rapid * 0.035; // GPU次波沿程频率。
        let speed = frame.surface_settings.w.clamp(0.0, 8.0); // production相位使用uniform平均流速，非UV1局部速度。
        let primary = section.x * frequency - phase_offset(elapsed, speed * frequency); // 一小时整数圈量化主相位。
        let crossing = section.x * crossing_frequency
            - phase_offset(elapsed, speed * crossing_frequency)
            - (lateral - 0.5) * (3.0 + rapid * 2.0); // 横向次波与WGSL一致。
        let bank = (lateral.min(1.0 - lateral).max(0.0)
            / frame.surface_settings.x.max(f32::EPSILON))
        .clamp(0.0, 1.0); // 同源UV与材质岸宽。
        let fade = bank * bank * (3.0 - 2.0 * bank); // water_smoothstep_unit同一三次曲线。
        let background =
            primary.sin() * frame.wave_directions_amplitudes[0].z * (1.0 + rapid * 0.70)
                + crossing.sin() * frame.wave_directions_amplitudes[1].z * (1.0 + rapid * 0.80); // 样条实际两组沿程波；静水才使用四组。
        position.y + (background + player_height(position.xz())) * fade // 先变形真实顶点，再插值渲染三角片面。
    })?;
    let position = Vec3::new(point.x, sample.surface_local_y, point.y); // 保留已验证的原始水平落点。
    position.is_finite().then_some(position) // 坏Uniform不能发布NaN粒子变换。
}

/// 与water.wgsl同源的一小时整数圈相位，回绕时保持发射面连续。
fn phase_offset(elapsed: f32, angular_speed: f32) -> f32 {
    let tau = std::f32::consts::TAU; // 与WGSL同一f32圆周常量。
    let cycles = (angular_speed * 3600.0 / tau).round(); // 整小时相位量化成整数圈。
    let phase = elapsed / 3600.0 * cycles; // 先归一化圈数，顺序与GPU相同。
    (phase - phase.floor()) * tau // WGSL fract对负值也使用向下取整。
}

#[cfg(test)]
mod tests; // 动态三角面、岸边与同源截面约束回归。
