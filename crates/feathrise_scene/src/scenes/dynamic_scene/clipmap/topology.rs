//! 生成一次中心方格与全部环带的组合 Mesh，再让唯一地形实体随相机平移。
//!
//! 实体只承载最细层吸附中心；各粗层在顶点 Shader 中按自身格距重新吸附到世界
//! 固定格点。环带内缘继续使用上一细层中心，已有的一格过渡带负责连接两套中心，
//! 避免粗网格每跨一个最细格就滑动并让远处山体持续变形。

use std::collections::HashMap; // 可变薄环只缓存实际出现的格点，避免按外包矩形分配临时表。

use bevy::asset::RenderAssetUsages; // 网格只需上传渲染世界。
use bevy::mesh::{Indices, PrimitiveTopology}; // 以显式三角拓扑保证环带接缝。
use bevy::prelude::{Mesh, Vec2}; // 显示拓扑不再派生物理顶点。
use feathrise_scene_content::{CompiledDisplayLodBand, CompiledDisplayLodPlan}; // 作者编译计划同时约束Mesh与分页身份。

const MAX_COMBINED_MESH_UPLOAD_BYTES: u64 = 16 * 1024 * 1024; // 仅计位置、法线、UV_0、UV_1 和 u32 索引的固定 16 MiB 上限。

/// 一个场景允许的轴对齐世界 X/Z 范围。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ClipmapBounds {
    pub min: Vec2, // 世界 X/Z 最小坐标。
    pub max: Vec2, // 世界 X/Z 最大坐标。
}

impl ClipmapBounds {
    /// 拒绝退化或无穷边界，避免片元裁切和中心吸附出现 NaN。
    pub(crate) fn new(min: Vec2, max: Vec2) -> Result<Self, ClipmapError> {
        if !min.is_finite() || !max.is_finite() || min.x >= max.x || min.y >= max.y {
            return Err(ClipmapError::InvalidBounds); // 有限正面积是后续除法和裁切前提。
        }
        Ok(Self { min, max }) // 用 Vec2.y 表示世界 Z。
    }

    /// 判断整个场景是否已经落入当前方形覆盖区。
    #[cfg(test)]
    fn inside_square(self, center: Vec2, radius: Vec2) -> bool {
        self.min.cmpge(center - radius).all() && self.max.cmple(center + radius).all()
        // 两轴均需覆盖。
    }
}

/// 原型的输入；作者格距可在 X/Z 两轴不同，LOD 只做二倍缩放。
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ClipmapConfig {
    pub finest_cell_size: Vec2, // 最细显示格距，与作者格距可分别配置。
    pub half_cells: u32,        // 中心方格的半边格数；环带外边沿沿用此数。
    pub levels: u8,             // 含中心层的总层数。
}

/// 输入错误交给调用方显示，避免无效配置进入渲染路径。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClipmapError {
    InvalidCellSize, // 格距必须是正的有限数。
    #[cfg(test)]
    InvalidHalfCells, // 半边格数必须能被二整除且至少有两个过渡格。
    #[cfg(test)]
    InvalidLevels, // 层数需有界，避免尺寸或 Mesh 数量失控。
    #[cfg(test)]
    GeometryBudgetExceeded, // 单 Mesh 的层数与格数乘积不得超过静态预算。
    InvalidBounds,   // 场景矩形必须非空且有限。
    InvalidCamera,   // 相机位置不能包含 NaN 或无穷。
    InvalidPlan,     // 编译计划的边界、倍率或预计算数量与真实拓扑不一致。
    AllocationFailed, // 受检容量仍无法在当前平台完成可失败分配。
}

/// 相同的环带 Mesh 可供所有较粗层共享一个资产句柄。
#[cfg(test)]
pub(crate) struct ClipmapMeshTemplates {
    pub center: Mesh, // LOD 0 的完整中心方格。
    pub ring: Mesh,   // LOD 1..N 的同一空心方格。
}

/// 单级实例的世界位置和水平缩放，不包含高度或物理数据。
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ClipmapPlacement {
    pub level: u8,       // 0 使用中心 Mesh，其余使用环带 Mesh。
    pub center: Vec2,    // 本级按自身格距吸附的世界中心；Shader 以同一公式定位。
    pub cell_size: Vec2, // 本级 Mesh 一个格对应的世界 X/Z 米数。
}

/// 调用方可据此判断最外圈是否已覆盖场景全貌。
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ClipmapPlanStatus {
    pub center: Vec2,            // 组合实体使用的最细层吸附中心。
    pub covers_entire_scene: bool, // 俯视全图需要为 true 或显式远景托底。
}

#[cfg(test)]
impl ClipmapConfig {
    /// 与正式Shader保持一致：按各层固定格点吸附，不把全局相机钳制到独立网格边界。
    fn snapped_center(self, focus_xz: Vec2, bounds: ClipmapBounds, level: u8) -> Vec2 {
        let cell_size = self.finest_cell_size * 2.0_f32.powi(i32::from(level));
        bounds.min + (((focus_xz - bounds.min) / cell_size) + Vec2::splat(0.5)).floor() * cell_size
    }

    /// 按实际模板顶点与索引数量限制上传；8192m/0.125m 在 64 半格时只需 11 级。
    pub(crate) fn validate(self) -> Result<(), ClipmapError> {
        if !self.finest_cell_size.is_finite()
            || self.finest_cell_size.x <= 0.0
            || self.finest_cell_size.y <= 0.0
        {
            return Err(ClipmapError::InvalidCellSize); // 非法缩放会使高度页寻址失效。
        }
        if self.half_cells < 4 || self.half_cells % 2 != 0 || self.half_cells > 256 {
            return Err(ClipmapError::InvalidHalfCells); // 不允许过大的静态 Mesh 或半格洞口。
        }
        if self.levels == 0 || self.levels > 16 {
            return Err(ClipmapError::InvalidLevels); // 实例数与二倍缩放保持固定上限。
        }
        if self.estimated_combined_mesh_bytes() > MAX_COMBINED_MESH_UPLOAD_BYTES {
            return Err(ClipmapError::GeometryBudgetExceeded); // 限制组合 Mesh 的顶点/索引内存。
        }
        Ok(()) // 其余几何参数由静态模板处理。
    }

    /// 从真实的中心格、环带和内缘拆边公式计算组合 Mesh 原始上传字节数。
    pub(crate) fn estimated_combined_mesh_bytes(self) -> u64 {
        let half = u128::from(self.half_cells); // 即使收到非法 u32 极值也不溢出。
        let rings = u128::from(self.levels.saturating_sub(1)); // 最细级是唯一中心格。
        let center_vertices = (2 * half + 1).pow(2); // 包括中心格的共享边界顶点。
        let ring_vertices = 3 * half * half + 14 * half; // 整点环带与每条内缘拆边两点。
        let center_indices = 24 * half * half; // 4*half² 个格、每格六索引。
        let ring_indices = 18 * half * half + 36 * half; // 内缘 4*half 格各额外九索引。
        let vertex_bytes = (center_vertices + rings * ring_vertices) * 40; // position12 + normal12 + UV_0 8 + UV_1 8。
        let index_bytes = (center_indices + rings * ring_indices) * 4; // U32 三角索引。
        (vertex_bytes + index_bytes).min(u128::from(u64::MAX)) as u64 // 不含 Bevy/WGPU 对齐和初始化临时缓冲。
    }

    /// 两张 Mesh 只在初始化时生成；运行时通过共享 Handle 重复实例化。
    #[cfg(test)]
    pub(crate) fn build_mesh_templates(self) -> Result<ClipmapMeshTemplates, ClipmapError> {
        self.validate()?; // 在分配网格前检查配置。
        let center = build_mesh(self.half_cells as i32, false); // 完整中心格。
        let ring = build_mesh(self.half_cells as i32, true); // 后续级共用空心格。
        Ok(ClipmapMeshTemplates { center, ring }) // 由主世界登记 Assets<Mesh>。
    }

    /// 以一个 Mesh 承载全部环带，供现有“唯一可见地形实体”路径使用。
    /// 有限场景的空外环仍留在此共享 Mesh 中，材质必须按世界边界裁切片元。
    pub(crate) fn build_combined_mesh(self) -> Result<Mesh, ClipmapError> {
        // 唯一显示模板，不再复制整图物理拓扑。
        self.validate()?; // 先阻断超出固定预算的输入。
        let center = build_template(self.half_cells as i32, false); // 只构造一次中心拓扑。
        let ring = build_template(self.half_cells as i32, true); // 所有外级复用相同拓扑数据。
        let mut combined = MeshBuilder::default(); // 最终 Mesh 仅在初始化时拼接。
        let estimated_bytes = self.estimated_combined_mesh_bytes(); // 已由u128公式和16MiB门验证。
        let estimated_vertices =
            usize::try_from(estimated_bytes / 40).map_err(|_| ClipmapError::AllocationFailed)?; // 仅作安全容量上界，正式自定义路径使用精确计数。
        combined.try_reserve(estimated_vertices, estimated_vertices.saturating_mul(6))?; // 固定模板上界防止小Vec反复扩容。
        for level in 0..self.levels {
            let multiplier = 2.0_f32.powi(i32::from(level)); // 相邻级世界尺寸二倍。
            let cell_size = self.finest_cell_size * multiplier; // 作者两轴实际格距可不同。
            let template = if level == 0 { &center } else { &ring }; // 中心或环带模板。
            combined.append_scaled(template, cell_size, level); // UV_1直接写每顶点实际几何Mip。
        }
        combined.finish_checked() // 只返回显示Mesh，保持原环带拓扑与LOD不变。
    }

    /// 覆写调用方复用的缓冲，避免每帧按场景面积生成实体或索引。
    pub(crate) fn write_placements(
        self,                            // 当前场景视图的环带规格。
        camera_xz: Vec2,                 // 相机在世界 X/Z 平面上的位置。
        bounds: ClipmapBounds,           // 场景有限矩形，用于边缘中心钳制。
        out: &mut Vec<ClipmapPlacement>, // 调用方持有并重复使用的小 Vec。
    ) -> Result<ClipmapPlanStatus, ClipmapError> {
        self.validate()?; // 运行配置变更时仍拒绝坏输入。
        if !camera_xz.is_finite() {
            return Err(ClipmapError::InvalidCamera); // 不把 NaN 写入实体 Transform。
        }
        ClipmapBounds::new(bounds.min, bounds.max)?; // 不信任反序列化后的原始字段。
        out.clear(); // 保留 Vec 容量，只重写固定数量 placement。
        let fine_center = self.snapped_center(camera_xz, bounds, 0); // 实体只跟随最细层中心。
        for level in 0..self.levels {
            let multiplier = 2.0_f32.powi(i32::from(level)); // 二倍层级只算最多 16 次。
            let cell_size = self.finest_cell_size * multiplier; // X/Z 保留作者实际轴格距比例。
            if let Some(previous) = out.last() {
                let previous_radius = previous.cell_size * self.half_cells as f32;
                if bounds.inside_square(previous.center, previous_radius) {
                    break; // 上一层的外方形已覆盖整图，当前空环无需进入组合规划。
                }
            }
            let center = self.snapped_center(fine_center, bounds, level); // 与只能读取实体中心的Shader保持同式。
            out.push(ClipmapPlacement {
                level,
                center,
                cell_size,
            }); // Mesh 句柄按 level 选择。
        }
        let outer = out.last().expect("至少有一层中心方格"); // validate 保证 levels 非零。
        let radius = outer.cell_size * self.half_cells as f32; // 最外层世界半径。
        Ok(ClipmapPlanStatus {
            center: fine_center,
            covers_entire_scene: bounds.inside_square(outer.center, radius),
        }) // 通知远景托底需求。
    }
}

/// 构建过程中所有顶点共享同一高度采样属性约定。
#[derive(Default)]
struct MeshBuilder {
    positions: Vec<[f32; 3]>,  // 归一化 X/Z 整数格与接缝半格。
    sample_lod: Vec<[f32; 2]>, // UV_1.x：顶点直接使用的几何Mip指数；y保留为零。
    indices: Vec<u32>,         // Bevy TriangleList 的正面朝 +Y。
}

impl MeshBuilder {
    /// 在写入任何顶点前为完整候选执行可失败容量申请，避免半构建后才因扩容中止。
    fn try_reserve(&mut self, vertices: usize, indices: usize) -> Result<(), ClipmapError> {
        self.positions
            .try_reserve_exact(vertices)
            .map_err(|_| ClipmapError::AllocationFailed)?; // 位置数组是组合Mesh第一份大分配。
        self.sample_lod
            .try_reserve_exact(vertices)
            .map_err(|_| ClipmapError::AllocationFailed)?; // UV_1与位置一一对应。
        self.indices
            .try_reserve_exact(indices)
            .map_err(|_| ClipmapError::AllocationFailed)?; // 显式三角索引容量来自u128预检。
        Ok(()) // 后续push不得再触发容量增长。
    }

    /// 内缘顶点必须在世界坐标重合处采样上一层高度 Mip。
    fn vertex(&mut self, x: f32, z: f32, sample_lod: f32) -> u32 {
        let index = self.positions.len() as u32; // 上限 256 半格远低于 u32 容量。
        self.positions.push([x, 0.0, z]); // 真正高度留给 GPU 顶点采样。
        self.sample_lod.push([sample_lod, 0.0]); // UV_1.y 暂留零。
        index // 返回此模板顶点索引。
    }

    /// 把静态局部模板按世界格距装入单Mesh，并把细边选择器编译为显式几何Mip。
    fn append_scaled(&mut self, template: &Self, cell_size: Vec2, level: u8) {
        let base = self.positions.len() as u32; // 配置上限确保总顶点数小于 u32。
        for (position, sample_lod) in template.positions.iter().zip(&template.sample_lod) {
            self.positions
                .push([position[0] * cell_size.x, 0.0, position[2] * cell_size.y]); // 世界米制 X/Z，暂相对最细层实体中心。
            let geometry_mip = if sample_lod[0] < 0.5 {
                level.saturating_sub(1) // 粗侧接缝边中点和整点直接使用上一细级。
            } else {
                level // 普通顶点及过渡单元中心使用当前几何级。
            };
            self.sample_lod.push([f32::from(geometry_mip), 0.0]); // Shader不再从层号和选择器二次推导。
        }
        self.indices
            .extend(template.indices.iter().map(|index| base + *index)); // 各级索引偏移到同一顶点缓冲。
    }

    /// 把顺时针 X/Z 多边形围绕单元中心扇形剖分，法线朝 +Y。
    fn split_cell(&mut self, corners: [u32; 4], side: InnerSide, x: i32, z: i32) {
        let midpoint = match side {
            InnerSide::Left => (x as f32, z as f32 + 0.5), // 洞口左侧边。
            InnerSide::Top => (x as f32 + 0.5, z as f32 + 1.0), // 洞口上侧边。
            InnerSide::Right => (x as f32 + 1.0, z as f32 + 0.5), // 洞口右侧边。
            InnerSide::Bottom => (x as f32 + 0.5, z as f32), // 洞口下侧边。
        };
        let seam = self.vertex(midpoint.0, midpoint.1, 0.0); // 与细级边缘完全同坐标及 Mip。
        let center = self.vertex(x as f32 + 0.5, z as f32 + 0.5, 1.0); // 一格宽度内过渡到粗 Mip。
        let polygon = match side {
            InnerSide::Left => [corners[0], seam, corners[1], corners[2], corners[3]], // 左边插入中点。
            InnerSide::Top => [corners[0], corners[1], seam, corners[2], corners[3]], // 上边插入中点。
            InnerSide::Right => [corners[0], corners[1], corners[2], seam, corners[3]], // 右边插入中点。
            InnerSide::Bottom => [corners[0], corners[1], corners[2], corners[3], seam], // 下边插入中点。
        }; // 固定五点留在栈上，薄大环不会产生逐单元堆分配。
        for edge in 0..polygon.len() {
            self.indices
                .extend([center, polygon[edge], polygon[(edge + 1) % polygon.len()]]);
            // 全部朝 +Y。
        }
    }

    /// 保留位置、法线、PBR UV 和 Mip 标记；真实法线须在位移着色器从高度梯度重建。
    fn finish_checked(self) -> Result<Mesh, ClipmapError> {
        if self.positions.len() != self.sample_lod.len() {
            return Err(ClipmapError::InvalidPlan); // 顶点属性长度不匹配时不得交给RenderApp。
        }
        let mut normals = Vec::new(); // 尚未位移时的保守占位法线。
        normals
            .try_reserve_exact(self.positions.len())
            .map_err(|_| ClipmapError::AllocationFailed)?; // 法线是最终Mesh的第三份顶点数组。
        normals.resize(self.positions.len(), [0.0, 1.0, 0.0]); // GPU材质会从高度梯度重建真实法线。
        let mut surface_uv = Vec::new(); // TerrainSurface仍要求完整UV_0布局。
        surface_uv
            .try_reserve_exact(self.positions.len())
            .map_err(|_| ClipmapError::AllocationFailed)?; // UV_0在登记前完成可失败分配。
        surface_uv.extend(
            self.positions
                .iter()
                .map(|position| [position[0], position[2]]),
        ); // 世界米制局部X/Z供标准PBR入口声明uv_a。
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        ); // 单一静态模板资产。
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.positions); // 局部平面坐标。
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals); // 供标准管线完成布局。
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, surface_uv); // 让带标准纹理定义的地表PBR管线声明uv_a。
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, self.sample_lod); // 顶点着色器读取 uv_b.x。
        mesh.insert_indices(Indices::U32(self.indices)); // 显式控制环内洞口连接。
        Ok(mesh) // 由调用方登记为共享Mesh句柄。
    }

    /// 旧固定模板测试沿用不可失败签名；其尺寸已由固定256半格上限预检。
    #[cfg(test)]
    fn finish(self) -> Mesh {
        self.finish_checked()
            .expect("受检固定Clipmap模板分配必须成功") // 正式自定义路径只调用finish_checked并向上返回错误。
    }
}

/// 多边形四条边顺序对应 [左下, 左上, 右上, 右下]。
#[derive(Clone, Copy)]
enum InnerSide {
    Left = 0,   // 左边由下往上。
    Top = 1,    // 上边由左往右。
    Right = 2,  // 右边由上往下。
    Bottom = 3, // 下边由右往左。
}

/// 只有紧邻中央洞口的一格带需要拆边，其余仍是标准两三角格。
#[cfg(test)]
fn inner_side(x: i32, z: i32, inner: i32) -> Option<InnerSide> {
    if x == inner && (-inner..inner).contains(&z) {
        Some(InnerSide::Left) // 洞口在格子左边。
    } else if z + 1 == -inner && (-inner..inner).contains(&x) {
        Some(InnerSide::Top) // 洞口在格子上边。
    } else if x + 1 == -inner && (-inner..inner).contains(&z) {
        Some(InnerSide::Right) // 洞口在格子右边。
    } else if z == inner && (-inner..inner).contains(&x) {
        Some(InnerSide::Bottom) // 洞口在格子下边。
    } else {
        None // 普通格没有细格接缝。
    }
}

/// 稠密索引仅随固定模板尺寸分配一次，不随场景世界格点总数增长。
#[cfg(test)]
fn grid_index(x: i32, z: i32, half: i32, width: usize) -> usize {
    ((z + half) as usize) * width + (x + half) as usize // [-half,half] 映射一维数组。
}

/// `ring=false` 是中心满格；`ring=true` 删去中央一半半径的单元。
#[cfg(test)]
fn build_template(half: i32, ring: bool) -> MeshBuilder {
    let inner = half / 2; // 上一级外边沿恰好落在本级半径的一半。
    let width = (half * 2 + 1) as usize; // 固定方格点数。
    let mut vertex_indices = vec![u32::MAX; width * width]; // 初始化期的临时查表。
    let mut builder = MeshBuilder::default(); // 位置、Mip标记和三角索引。
    for z in -half..=half {
        for x in -half..=half {
            if ring && x.abs() < inner && z.abs() < inner {
                continue; // 环带中央只保留边界顶点，不覆盖细级内部。
            }
            let on_inner = ring
                && ((x.abs() == inner && z.abs() <= inner)
                    || (z.abs() == inner && x.abs() <= inner)); // 接缝整点。
            let sample_lod = if on_inner { 0.0 } else { 1.0 }; // 交界与前一级同源采样。
            let index = builder.vertex(x as f32, z as f32, sample_lod); // 固定整数格点。
            vertex_indices[grid_index(x, z, half, width)] = index; // 后续单元复用顶点。
        }
    }
    for z in -half..half {
        for x in -half..half {
            if ring && (-inner..inner).contains(&x) && (-inner..inner).contains(&z) {
                continue; // 完全在洞内的格子不产生三角形。
            }
            let a = vertex_indices[grid_index(x, z, half, width)]; // 左下。
            let c = vertex_indices[grid_index(x, z + 1, half, width)]; // 左上。
            let d = vertex_indices[grid_index(x + 1, z + 1, half, width)]; // 右上。
            let b = vertex_indices[grid_index(x + 1, z, half, width)]; // 右下。
            if ring {
                if let Some(side) = inner_side(x, z, inner) {
                    builder.split_cell([a, c, d, b], side, x, z); // 内缘从粗边变成两个细边。
                    continue; // 已写完整五角剖分。
                }
            }
            builder.indices.extend([a, c, b, b, c, d]); // 常规格正面朝 +Y。
        }
    }
    builder // 返回静态拓扑数据，临时查表随即释放。
}

/// 矩形内洞只有粗侧首格拆边，四角仍由唯一一侧拥有，避免重复过渡三角。
fn rectangular_inner_side(
    x: i32,       // 当前粗格左下X。
    z: i32,       // 当前粗格左下Z。
    inner_x: i32, // 内洞X半径。
    inner_z: i32, // 内洞Z半径。
) -> Option<InnerSide> {
    if x == inner_x && (-inner_z..inner_z).contains(&z) {
        Some(InnerSide::Left) // 正X侧格子的左边贴住洞口。
    } else if z + 1 == -inner_z && (-inner_x..inner_x).contains(&x) {
        Some(InnerSide::Top) // 负Z侧格子的上边贴住洞口。
    } else if x + 1 == -inner_x && (-inner_z..inner_z).contains(&z) {
        Some(InnerSide::Right) // 负X侧格子的右边贴住洞口。
    } else if z == inner_z && (-inner_x..inner_x).contains(&x) {
        Some(InnerSide::Bottom) // 正Z侧格子的下边贴住洞口。
    } else {
        None // 普通粗格继续保持两个三角形。
    }
}

/// 当前整数格点是否位于矩形内洞边界；这些共享整点必须使用上一细级世界中心。
fn on_rectangular_inner_edge(x: i32, z: i32, inner_x: i32, inner_z: i32) -> bool {
    ((x.abs() == inner_x && z.abs() <= inner_z) || (z.abs() == inner_z && x.abs() <= inner_x))
        && inner_x > 0
        && inner_z > 0 // 首层零内洞的原点不是接缝。
}

/// 为一个实际环带惰性创建共享整数格点，临时哈希表只随真实环带周长/面积增长。
fn rectangular_grid_vertex(
    builder: &mut MeshBuilder,               // 当前单环带连续顶点与索引。
    vertices: &mut HashMap<(i32, i32), u32>, // 只保存被至少一个可见单元引用的格点。
    x: i32,                                  // 当前整数格点X。
    z: i32,                                  // 当前整数格点Z。
    inner_x: i32,                            // 内洞X半径。
    inner_z: i32,                            // 内洞Z半径。
) -> u32 {
    if let Some(index) = vertices.get(&(x, z)) {
        return *index; // 相邻格复用同一个位置和高度Mip属性。
    }
    let sample_selector = if on_rectangular_inner_edge(x, z, inner_x, inner_z) {
        0.0 // 接缝整点与边中点同用上一细层。
    } else {
        1.0 // 其余整数点使用当前粗层。
    };
    let index = builder.vertex(x as f32, z as f32, sample_selector); // 真正米制缩放留给组合阶段。
    vertices.insert((x, z), index); // 后续单元O(1)复用。
    index
}

/// 向矩形环带写入一段连续X单元，中央洞区从不进入该函数。
fn append_rectangular_cell_row(
    builder: &mut MeshBuilder,               // 当前环带输出。
    vertices: &mut HashMap<(i32, i32), u32>, // 共享整数格点索引。
    x_start: i32,                            // 包含的首个单元X。
    x_end: i32,                              // 不包含的末个单元X。
    z: i32,                                  // 当前单元行Z。
    inner_x: i32,                            // 内洞X半径。
    inner_z: i32,                            // 内洞Z半径。
) {
    for x in x_start..x_end {
        let a = rectangular_grid_vertex(builder, vertices, x, z, inner_x, inner_z); // 左下。
        let c = rectangular_grid_vertex(builder, vertices, x, z + 1, inner_x, inner_z); // 左上。
        let d = rectangular_grid_vertex(builder, vertices, x + 1, z + 1, inner_x, inner_z); // 右上。
        let b = rectangular_grid_vertex(builder, vertices, x + 1, z, inner_x, inner_z); // 右下。
        if let Some(side) = rectangular_inner_side(x, z, inner_x, inner_z) {
            builder.split_cell([a, c, d, b], side, x, z); // 粗侧首格用五三角固定接缝。
        } else {
            builder.indices.extend([a, c, b, b, c, d]); // 其余规则格严格两个三角形。
        }
    } // 完成当前连续条带。
}

/// 构造一层可变X/Z内外半径环带，不扫描或分配中央洞口的巨大空矩形。
fn build_rectangular_band(band: &CompiledDisplayLodBand) -> Result<MeshBuilder, ClipmapError> {
    let outer_x = i32::try_from(band.outer_half_cells[0]).map_err(|_| ClipmapError::InvalidPlan)?;
    let outer_z = i32::try_from(band.outer_half_cells[1]).map_err(|_| ClipmapError::InvalidPlan)?;
    let inner_x = i32::try_from(band.inner_half_cells[0]).map_err(|_| ClipmapError::InvalidPlan)?;
    let inner_z = i32::try_from(band.inner_half_cells[1]).map_err(|_| ClipmapError::InvalidPlan)?;
    if outer_x <= 0
        || outer_z <= 0
        || inner_x < 0
        || inner_z < 0
        || inner_x > outer_x
        || inner_z > outer_z
        || (inner_x == outer_x && inner_z == outer_z)
    {
        return Err(ClipmapError::InvalidPlan); // 编译器不变量仍在真正分配前复核。
    }
    if (inner_x == 0) != (inner_z == 0) {
        return Err(ClipmapError::InvalidPlan); // 单轴退化洞口没有受支持的接缝语义。
    }
    let outer_grid_vertices = (2_u128 * outer_x as u128 + 1)
        .checked_mul(2_u128 * outer_z as u128 + 1)
        .ok_or(ClipmapError::InvalidPlan)?; // 完整外矩形格点上界。
    let removed_inner_vertices = if inner_x == 0 {
        0 // 首层保留全部整数格点。
    } else {
        (2_u128 * inner_x as u128 - 1)
            .checked_mul(2_u128 * inner_z as u128 - 1)
            .ok_or(ClipmapError::InvalidPlan)? // 严格洞内点不会被任何单元引用。
    };
    let unused_boundary_vertices = if inner_x == 0 {
        0 // 中心层没有内边。
    } else {
        let vertical = if outer_x == inner_x {
            2_u128
                .checked_mul(2_u128 * inner_z as u128 - 1)
                .ok_or(ClipmapError::InvalidPlan)?
        } else {
            0
        }; // 没有左右外侧格时，两条竖直内边不会被本层引用。
        let horizontal = if outer_z == inner_z {
            2_u128
                .checked_mul(2_u128 * inner_x as u128 - 1)
                .ok_or(ClipmapError::InvalidPlan)?
        } else {
            0
        }; // 没有上下外侧格时，两条水平内边同样只属于上一细层。
        vertical
            .checked_add(horizontal)
            .ok_or(ClipmapError::InvalidPlan)?
    };
    let grid_vertices = outer_grid_vertices
        .checked_sub(removed_inner_vertices)
        .and_then(|value| value.checked_sub(unused_boundary_vertices))
        .ok_or(ClipmapError::InvalidPlan)?; // 实际哈希表键数量。
    let outer_cells = 4_u128
        .checked_mul(outer_x as u128)
        .and_then(|value| value.checked_mul(outer_z as u128))
        .ok_or(ClipmapError::InvalidPlan)?; // 外矩形单元总数。
    let inner_cells = 4_u128
        .checked_mul(inner_x as u128)
        .and_then(|value| value.checked_mul(inner_z as u128))
        .ok_or(ClipmapError::InvalidPlan)?; // 首层自然为零。
    let ring_cells = outer_cells
        .checked_sub(inner_cells)
        .ok_or(ClipmapError::InvalidPlan)?; // 真实遍历单元数。
    let transition_cells = if inner_x == 0 {
        0 // 中心层没有拆边。
    } else {
        let transition_half_length = (outer_x > inner_x)
            .then_some(inner_z as u128)
            .unwrap_or(0)
            .checked_add((outer_z > inner_z).then_some(inner_x as u128).unwrap_or(0))
            .ok_or(ClipmapError::InvalidPlan)?; // 只统计外侧实际存在格子的内边。
        4_u128
            .checked_mul(transition_half_length)
            .ok_or(ClipmapError::InvalidPlan)? // 单轴侧向环只拆左右或上下两条边。
    };
    let vertex_count = grid_vertices
        .checked_add(transition_cells * 2)
        .ok_or(ClipmapError::InvalidPlan)?; // 每过渡格增加边中点和中心。
    let index_count = ring_cells
        .checked_mul(6)
        .and_then(|value| value.checked_add(transition_cells * 9))
        .ok_or(ClipmapError::InvalidPlan)?; // 每过渡格从6索引增加为15。
    let vertex_count = usize::try_from(vertex_count).map_err(|_| ClipmapError::AllocationFailed)?;
    let index_count = usize::try_from(index_count).map_err(|_| ClipmapError::AllocationFailed)?;
    let grid_vertex_count =
        usize::try_from(grid_vertices).map_err(|_| ClipmapError::AllocationFailed)?;
    let mut builder = MeshBuilder::default(); // 单环带完成后立即拼入组合数组并释放临时表。
    builder.try_reserve(vertex_count, index_count)?; // 真正写入前先完成所有连续数组容量申请。
    let mut vertices = HashMap::new(); // 薄环不创建外包矩形稠密索引。
    vertices
        .try_reserve(grid_vertex_count)
        .map_err(|_| ClipmapError::AllocationFailed)?; // 临时容量同样可失败返回。
    for z in -outer_z..outer_z {
        if inner_x > 0 && (-inner_z..inner_z).contains(&z) {
            append_rectangular_cell_row(
                &mut builder,
                &mut vertices,
                -outer_x,
                -inner_x,
                z,
                inner_x,
                inner_z,
            ); // 洞口同高行只写左条带。
            append_rectangular_cell_row(
                &mut builder,
                &mut vertices,
                inner_x,
                outer_x,
                z,
                inner_x,
                inner_z,
            ); // 同行继续写右条带。
        } else {
            append_rectangular_cell_row(
                &mut builder,
                &mut vertices,
                -outer_x,
                outer_x,
                z,
                inner_x,
                inner_z,
            ); // 洞口上下方写完整水平条带。
        }
    } // 完成实际环带单元，不遍历洞内面积。
    if builder.positions.len() != vertex_count || builder.indices.len() != index_count {
        return Err(ClipmapError::InvalidPlan); // 闭式预算与真实拓扑必须逐项一致。
    }
    Ok(builder) // 临时哈希表在返回前释放，只移动紧凑连续数组。
}

/// 根据共享编译计划生成一个可变内外边界的组合Mesh。
pub(crate) fn build_display_lod_mesh(
    plan: &CompiledDisplayLodPlan, // 已完成结构、量化、页和16MiB预算预检。
    finest_cell_size: Vec2,        // 作者实际X/Z格距。
) -> Result<Mesh, ClipmapError> {
    if !finest_cell_size.is_finite() || finest_cell_size.cmple(Vec2::ZERO).any() {
        return Err(ClipmapError::InvalidCellSize); // 不允许NaN、无穷或零格距进入位置缩放。
    }
    if plan.bands.is_empty()
        || plan.mesh_bytes > MAX_COMBINED_MESH_UPLOAD_BYTES
        || plan.index_count % 3 != 0
    {
        return Err(ClipmapError::InvalidPlan); // 计划必须来自共享编译器且仍满足本版本硬门。
    }
    let vertex_count =
        usize::try_from(plan.vertex_count).map_err(|_| ClipmapError::AllocationFailed)?;
    let index_count =
        usize::try_from(plan.index_count).map_err(|_| ClipmapError::AllocationFailed)?;
    let mut combined = MeshBuilder::default(); // 所有有效环带最终只登记一份Mesh资产。
    combined.try_reserve(vertex_count, index_count)?; // 在任何单环带构造前申请最终连续容量。
    let mut previous: Option<&CompiledDisplayLodBand> = None; // 逐层复核2:1与世界边界连续性。
    for band in &plan.bands {
        if !band.spacing_multiplier.is_power_of_two() {
            return Err(ClipmapError::InvalidPlan); // Shader几何Mip只能表示二次幂倍率。
        }
        if let Some(fine) = previous {
            if band.spacing_multiplier != fine.spacing_multiplier.saturating_mul(2) {
                return Err(ClipmapError::InvalidPlan); // 自动补带后任何相邻环只能是2:1。
            }
            for axis in 0..2 {
                let fine_outer =
                    u64::from(fine.outer_half_cells[axis]) * u64::from(fine.spacing_multiplier);
                let coarse_inner =
                    u64::from(band.inner_half_cells[axis]) * u64::from(band.spacing_multiplier);
                if fine_outer != coarse_inner {
                    return Err(ClipmapError::InvalidPlan); // 两层必须共享完全相同世界边界。
                }
            }
        } else if band.inner_half_cells != [0, 0] {
            return Err(ClipmapError::InvalidPlan); // 第一层必须是完整中心矩形。
        }
        let template = build_rectangular_band(band)?; // 仅当前真实单元进入临时内存。
        let geometry_mip = band.spacing_multiplier.trailing_zeros() as u8; // 显式Mip指数直接写入UV_1。
        let cell_size = finest_cell_size * band.spacing_multiplier as f32; // 当前层世界米制格距。
        combined.append_scaled(&template, cell_size, geometry_mip); // 接缝选择器在此转换为上一细Mip。
        previous = Some(band); // 下一层复核当前外边界。
    }
    if combined.positions.len() != vertex_count || combined.indices.len() != index_count {
        return Err(ClipmapError::InvalidPlan); // 共享u128预算必须与实际Mesh完全一致。
    }
    combined.finish_checked() // 法线与UV_0继续可失败分配，错误保留旧正式画面。
}

/// 相机或F12观察中心只更新受检最细格焦点和实体平移，不触发任何网格重建。
pub(crate) fn snapped_display_focus(
    focus_xz: Vec2,         // 玩法相机或F12轨道观察中心。
    bounds: ClipmapBounds,  // 当前场景受检有限边界。
    finest_cell_size: Vec2, // 作者基础X/Z格距。
) -> Result<Vec2, ClipmapError> {
    if !focus_xz.is_finite() {
        return Err(ClipmapError::InvalidCamera); // NaN焦点不得污染实体Transform。
    }
    ClipmapBounds::new(bounds.min, bounds.max)?; // 防御反序列化绕过边界校验。
    if !finest_cell_size.is_finite() || finest_cell_size.cmple(Vec2::ZERO).any() {
        return Err(ClipmapError::InvalidCellSize); // 吸附除法必须使用正有限格距。
    }
    let snapped = bounds.min
        + (((focus_xz - bounds.min) / finest_cell_size) + Vec2::splat(0.5)).floor() * finest_cell_size;
    Ok(snapped) // 覆盖计划负责完整场景；观察中心不能被每个对象分别钳制。
}

/// 独立模板可给多个实体共享，适合按级别裁剪或多视图独立实例。
#[cfg(test)]
fn build_mesh(half: i32, ring: bool) -> Mesh {
    build_template(half, ring).finish() // 高度仍由未来 GPU 顶点材质读取。
}

#[cfg(test)]
mod tests {
    use super::*; // 只检验纯几何与布局，不启动游戏窗口。
    use bevy::mesh::VertexAttributeValues; // 读取 Mesh 顶点做几何断言。
    use feathrise_scene_content::{SceneTerrainDisplayLod, SparseHeightSpec, compile_display_lod_plan}; // 直接走正式作者编译入口，避免手造不可能出现的计划。
    use std::collections::BTreeSet; // 比较相邻层的边界坐标集合。

    /// 从显式索引计算水平投影面积，检测洞口漏三角或重复三角。
    fn projected_area(mesh: &Mesh) -> f32 {
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(VertexAttributeValues::as_float3)
            .unwrap(); // 顶点模板。
        let mut area = 0.0; // 累计所有三角形的正面积。
        for triangle in mesh
            .indices()
            .unwrap()
            .iter()
            .collect::<Vec<_>>()
            .chunks_exact(3)
        {
            let a = positions[triangle[0] as usize]; // 第一个 X/Z 点。
            let b = positions[triangle[1] as usize]; // 第二个 X/Z 点。
            let c = positions[triangle[2] as usize]; // 第三个 X/Z 点。
            let cross = (b[0] - a[0]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[0] - a[0]); // X/Z 有向面积两倍。
            assert!(cross < 0.0, "三角绕序必须面向 +Y"); // 不允许反面或退化。
            area += cross.abs() * 0.5; // 每个格只覆盖一次。
        }
        area // 用闭式方形面积校验。
    }

    /// 只收集某条方形边上的坐标，整数倍二确保半格也可精确比较。
    fn square_edge_points(mesh: &Mesh, radius: f32, spacing: f32) -> BTreeSet<(i32, i32)> {
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(VertexAttributeValues::as_float3)
            .unwrap(); // 已生成的静态局部顶点。
        positions
            .iter()
            .filter_map(|p| {
                let on_edge = p[0].abs() == radius || p[2].abs() == radius; // 当前方形边。
                (on_edge && p[0].abs() <= radius && p[2].abs() <= radius)
                    .then_some(((p[0] * spacing * 2.0) as i32, (p[2] * spacing * 2.0) as i32))
                // 世界半格坐标。
            })
            .collect() // 相邻级世界边应完全相等。
    }

    /// 测试点取非整数偏移，避免恰好落在共享三角边上而误计两次。
    fn containing_triangle_count(mesh: &Mesh, point: Vec2, spacing: f32) -> usize {
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(VertexAttributeValues::as_float3)
            .unwrap(); // 局部模板位置。
        let indices = mesh.indices().unwrap().iter().collect::<Vec<_>>(); // 三角序列。
        indices
            .chunks_exact(3)
            .filter(|triangle| {
                let a = Vec2::new(
                    positions[triangle[0] as usize][0],
                    positions[triangle[0] as usize][2],
                ) * spacing; // 首个世界 X/Z 点。
                let b = Vec2::new(
                    positions[triangle[1] as usize][0],
                    positions[triangle[1] as usize][2],
                ) * spacing; // 第二点。
                let c = Vec2::new(
                    positions[triangle[2] as usize][0],
                    positions[triangle[2] as usize][2],
                ) * spacing; // 第三点。
                let inside =
                    |start: Vec2, end: Vec2| (end - start).perp_dot(point - start) < -0.0001; // 三条顺时针边的同侧条件。
                inside(a, b) && inside(b, c) && inside(c, a) // 只计严格位于三角内部的点。
            })
            .count() // 横跨中心/环带应恰好为一。
    }

    /// 按生产Shader直接记录的几何Mip重定位组合网格，用覆盖计数验证
    /// 独立层中心之间的过渡带没有留下裂口或重复面。
    fn containing_stabilized_triangle_count(
        mesh: &Mesh,
        point: Vec2,
        placements: &[ClipmapPlacement],
    ) -> usize {
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(VertexAttributeValues::as_float3)
            .unwrap();
        let Some(VertexAttributeValues::Float32x2(sample_lods)) =
            mesh.attribute(Mesh::ATTRIBUTE_UV_1)
        else {
            panic!("组合 Mesh 需要 float2 级别属性");
        };
        let indices = mesh.indices().unwrap().iter().collect::<Vec<_>>();
        let world_point = |index: usize| {
            let attribute = sample_lods[index];
            let geometry_lod = attribute[0].max(0.0) as usize; // UV_1.x已经是实际几何Mip。
            let center = placements[geometry_lod].center;
            Vec2::new(positions[index][0], positions[index][2]) + center
        };
        indices
            .chunks_exact(3)
            .filter(|triangle| {
                let a = world_point(triangle[0] as usize);
                let b = world_point(triangle[1] as usize);
                let c = world_point(triangle[2] as usize);
                let inside =
                    |start: Vec2, end: Vec2| (end - start).perp_dot(point - start) < -0.0001;
                inside(a, b) && inside(b, c) && inside(c, a)
            })
            .count()
    }

    #[test]
    fn center_and_ring_cover_squares_without_holes_or_reversed_triangles() {
        let meshes = ClipmapConfig {
            finest_cell_size: Vec2::splat(1.0),
            half_cells: 8,
            levels: 3,
        }
        .build_mesh_templates()
        .unwrap(); // 小规格便于闭式检验。
        assert_eq!(projected_area(&meshes.center), 16.0 * 16.0); // 中心全格面积。
        assert_eq!(projected_area(&meshes.ring), 16.0 * 16.0 - 8.0 * 8.0); // 环带仅有中央一半洞口。
        let fine_outer = square_edge_points(&meshes.center, 8.0, 1.0); // LOD0 外边。
        let coarse_inner = square_edge_points(&meshes.ring, 4.0, 2.0); // LOD1 内边。
        assert_eq!(fine_outer, coarse_inner); // 拆边后 X/Z 无 T 型漏缝。
        let coarse_outer = square_edge_points(&meshes.ring, 8.0, 2.0); // LOD1 外边。
        let next_inner = square_edge_points(&meshes.ring, 4.0, 4.0); // LOD2 内边。
        assert_eq!(coarse_outer, next_inner); // 所有后续层复用相同模板。
    }

    #[test]
    fn every_sampled_world_point_has_one_center_or_ring_triangle() {
        let meshes = ClipmapConfig {
            finest_cell_size: Vec2::ONE,
            half_cells: 4,
            levels: 2,
        }
        .build_mesh_templates()
        .unwrap(); // 世界覆盖 [-8,8]²。
        for z in -8..8 {
            for x in -8..8 {
                let point = Vec2::new(x as f32 + 0.233, z as f32 + 0.419); // 避开所有整数与半格边界。
                let covering = containing_triangle_count(&meshes.center, point, 1.0)
                    + containing_triangle_count(&meshes.ring, point, 2.0); // 两级共同覆盖。
                assert_eq!(covering, 1, "地形在 {point:?} 必须既无裂口也无重叠");
                // 拓扑层面无洞。
            }
        }
    }

    #[test]
    fn inner_ring_edge_samples_the_finer_height_mip() {
        let meshes = ClipmapConfig {
            finest_cell_size: Vec2::splat(1.0),
            half_cells: 8,
            levels: 2,
        }
        .build_mesh_templates()
        .unwrap(); // 两层足够检查接缝属性。
        let positions = meshes
            .ring
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(VertexAttributeValues::as_float3)
            .unwrap(); // 网格位置。
        let Some(VertexAttributeValues::Float32x2(sample_lods)) =
            meshes.ring.attribute(Mesh::ATTRIBUTE_UV_1)
        else {
            panic!("环带需要 float2 高度采样属性"); // UV_1.x 着色器契约。
        };
        for (position, sample_lod) in positions.iter().zip(sample_lods) {
            let edge = (position[0].abs() == 4.0 && position[2].abs() <= 4.0)
                || (position[2].abs() == 4.0 && position[0].abs() <= 4.0); // 内缘含半格中点。
            if edge {
                assert_eq!(sample_lod[0], 0.0);
            } // 内边精确采样上一细级 Mip。
        }
    }

    #[test]
    fn combined_mesh_covers_the_outer_square_with_one_world_space_asset() {
        let config = ClipmapConfig {
            finest_cell_size: Vec2::new(1.0, 2.0), // 两轴尺寸不同，面积必须按 X/Z 分别缩放。
            half_cells: 8,                         // 每级固定 16 格宽。
            levels: 3,                             // 中心加两个同源环带。
        };
        let mesh = config.build_combined_mesh().unwrap(); // 单实体用一份静态顶点/索引。
        assert_eq!(projected_area(&mesh), 64.0 * 128.0); // 最外层世界宽高 64m 与 128m。
        let vertex_count = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(VertexAttributeValues::as_float3)
            .unwrap()
            .len() as u64; // 真实生成顶点数。
        let index_count = mesh.indices().unwrap().iter().count() as u64; // 真实生成索引数。
        assert_eq!(
            config.estimated_combined_mesh_bytes(),
            vertex_count * 40 + index_count * 4
        ); // 预算公式与网格载荷一致。
        let Some(VertexAttributeValues::Float32x2(surface_uv)) =
            mesh.attribute(Mesh::ATTRIBUTE_UV_0)
        else {
            panic!("组合 Mesh 需要标准PBR使用的float2 UV_0"); // 缺失时嵌入PBR片元中的uv变量不会生成。
        };
        assert_eq!(surface_uv.len() as u64, vertex_count); // 每个Clipmap顶点都必须满足地表材质布局。
        let Some(VertexAttributeValues::Float32x2(levels)) = mesh.attribute(Mesh::ATTRIBUTE_UV_1)
        else {
            panic!("组合 Mesh 需要 float2 级别属性"); // 每顶点都带高度页级别。
        };
        assert!(levels.iter().any(|attribute| attribute[0] == 0.0)); // 中心级及第一环细边存在。
        assert!(levels.iter().any(|attribute| attribute[0] == 1.0)); // 第一环普通点及第二环细边存在。
        assert!(levels.iter().any(|attribute| attribute[0] == 2.0)); // 第二环普通点存在。
        assert!(levels.iter().all(|attribute| attribute[1] == 0.0)); // 第二分量不再参与Shader推导。
    }

    #[test]
    fn placements_keep_global_center_and_report_finite_scene_coverage() {
        let config = ClipmapConfig {
            finest_cell_size: Vec2::new(0.125, 0.2),
            half_cells: 8,
            levels: 4,
        }; // 轴格距不相同也保持同源坐标。
        let bounds = ClipmapBounds::new(Vec2::ZERO, Vec2::new(9.0, 10.0)).unwrap(); // 有限矩形场景。
        let mut placements = Vec::new(); // 模拟运行时复用缓冲。
        let status = config
            .write_placements(Vec2::new(-20.0, 0.1), bounds, &mut placements)
            .unwrap(); // 场景外相机仍保留真实观察中心。
        assert_eq!(status.center.x, -20.0); // 远处对象不能复制自己的细环。
        assert_eq!(placements[0].level, 0); // 最细级永远存在。
        assert!(placements.len() <= usize::from(config.levels)); // 场景规模不改变实体数上限。
        assert_eq!(placements[0].center, status.center); // 实体平移仍只使用最细层中心。
        assert!(
            placements
                .windows(2)
                .any(|pair| pair[0].center != pair[1].center)
        ); // 粗层拥有自身世界固定吸附中心。
        for placement in &placements {
            let lattice = (placement.center - bounds.min) / placement.cell_size;
            assert_eq!(lattice, lattice.round()); // 所有层都锚定场景原点的自身整数格。
        }
        assert!(!status.covers_entire_scene); // 此配置最外圈不足以覆盖远角，必须由调用方托底。
        let small = ClipmapBounds::new(Vec2::ZERO, Vec2::splat(0.2)).unwrap(); // 小于中心网格的场景。
        config
            .write_placements(Vec2::ZERO, small, &mut placements)
            .unwrap(); // 复用同一个 Vec。
        assert_eq!(placements.len(), 1); // 多余的空环不绘制。
    }

    #[test]
    fn coarse_lod_centers_do_not_slide_on_every_fine_cell_step() {
        let config = ClipmapConfig {
            finest_cell_size: Vec2::ONE,
            half_cells: 8,
            levels: 4,
        };
        let bounds = ClipmapBounds::new(Vec2::splat(-100.0), Vec2::splat(100.0)).unwrap();
        let mut before = Vec::new();
        let mut after = Vec::new();
        config
            .write_placements(Vec2::new(3.1, 5.1), bounds, &mut before)
            .unwrap();
        config
            .write_placements(Vec2::new(4.1, 5.1), bounds, &mut after)
            .unwrap(); // 焦点只跨过一个最细格。
        assert_ne!(before[0].center, after[0].center); // 近场窗口正常跟随。
        assert_eq!(before[2].center, after[2].center); // 四米格粗层仍停在原世界格。
        assert_eq!(before[3].center, after[3].center); // 更远层同样不会连续滑动。
        for pair in after.windows(2) {
            let delta = (pair[1].center - pair[0].center).abs();
            assert!(delta.cmple(pair[1].cell_size * 0.5).all()); // 一格过渡带不会被拉反。
        }
    }

    #[test]
    fn independently_snapped_lods_remain_watertight_through_transition_bands() {
        let config = ClipmapConfig {
            finest_cell_size: Vec2::ONE,
            half_cells: 4,
            levels: 3,
        };
        let bounds = ClipmapBounds::new(Vec2::splat(-100.0), Vec2::splat(100.0)).unwrap();
        let mut placements = Vec::new();
        config
            .write_placements(Vec2::new(3.1, 5.1), bounds, &mut placements)
            .unwrap();
        assert_eq!(placements[0].center, Vec2::new(3.0, 5.0));
        assert_eq!(placements[1].center, Vec2::new(4.0, 6.0));
        assert_eq!(placements[2].center, Vec2::new(4.0, 4.0)); // 两次不同方向偏移覆盖最坏缝合形态。
        let mesh = config.build_combined_mesh().unwrap();
        let outer = placements.last().unwrap();
        let radius = outer.cell_size * config.half_cells as f32;
        let min = outer.center - radius;
        let max = outer.center + radius;
        for z in min.y as i32..max.y as i32 {
            for x in min.x as i32..max.x as i32 {
                let point = Vec2::new(x as f32 + 0.233, z as f32 + 0.419);
                assert_eq!(
                    containing_stabilized_triangle_count(&mesh, point, &placements),
                    1,
                    "独立层中心在 {point:?} 必须既无裂口也无重叠"
                );
            }
        }
    }

    #[test]
    fn invalid_inputs_cannot_generate_unbounded_meshes_or_nan_transforms() {
        let config = ClipmapConfig {
            finest_cell_size: Vec2::splat(0.125),
            half_cells: 8,
            levels: 2,
        }; // 有效参考配置。
        assert_eq!(
            ClipmapConfig {
                half_cells: 257,
                ..config
            }
            .validate(),
            Err(ClipmapError::InvalidHalfCells)
        ); // 固定拓扑规模上限。
        assert_eq!(
            ClipmapConfig {
                levels: 0,
                ..config
            }
            .validate(),
            Err(ClipmapError::InvalidLevels)
        ); // 不允许空布局。
        assert_eq!(
            ClipmapConfig {
                half_cells: 256,
                levels: 16,
                ..config
            }
            .validate(),
            Err(ClipmapError::GeometryBudgetExceeded)
        ); // 不允许组合 Mesh 静态内存失控。
        assert!(
            ClipmapConfig {
                half_cells: 64,
                levels: 11,
                ..config
            }
            .estimated_combined_mesh_bytes()
                < MAX_COMBINED_MESH_UPLOAD_BYTES
        ); // 8km/0.125m 常用组合留有预算。
        assert_eq!(
            ClipmapConfig {
                finest_cell_size: Vec2::ZERO,
                ..config
            }
            .validate(),
            Err(ClipmapError::InvalidCellSize)
        ); // 不允许零缩放。
        assert_eq!(
            ClipmapBounds::new(Vec2::ZERO, Vec2::ZERO),
            Err(ClipmapError::InvalidBounds)
        ); // 不允许退化场景。
        let bounds = ClipmapBounds::new(Vec2::ZERO, Vec2::ONE).unwrap(); // 有效场景。
        assert_eq!(
            config.write_placements(Vec2::splat(f32::NAN), bounds, &mut Vec::new()),
            Err(ClipmapError::InvalidCamera)
        ); // NaN 不进入 Transform。
    }

    #[test] // 可变矩形环、1→8跳级和任意格相位必须直接在正式组合Mesh上无洞无重叠。
    fn compiled_variable_bands_build_exact_watertight_mesh() {
        let spec = SparseHeightSpec::new(
            [0.0, 0.0],
            [73.0, 97.0], // 两轴不可整除最粗8米格，末层实际量化边界应不同。
            1.0,
            -8.0,
            16.0,
        )
        .unwrap();
        let authored = vec![
            SceneTerrainDisplayLod {
                outer_half_extent_meters: Some(3.1), // 向外量化到下一粗格的4米边界。
                spacing_multiplier: 1,
            },
            SceneTerrainDisplayLod {
                outer_half_extent_meters: None,
                spacing_multiplier: 8, // 编译器内部插入×2、×4的一格2:1过渡环。
            },
        ];
        let plan = compile_display_lod_plan(&spec, &authored).unwrap();
        assert_eq!(
            plan.bands
                .iter()
                .map(|band| band.spacing_multiplier)
                .collect::<Vec<_>>(),
            vec![1, 2, 4, 8]
        );
        assert_eq!(plan.actual_half_extents_meters.last(), Some(&[80.0, 104.0]));
        let mesh = build_display_lod_mesh(&plan, Vec2::ONE).unwrap();
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(VertexAttributeValues::as_float3)
            .unwrap();
        assert_eq!(positions.len() as u64, plan.vertex_count); // u128预检与真实可失败分配逐项一致。
        assert_eq!(
            mesh.indices().unwrap().iter().count() as u64,
            plan.index_count
        );
        assert_eq!(projected_area(&mesh), 160.0 * 208.0); // 未吸附时完整覆盖最终矩形且没有重复面。
        let Some(VertexAttributeValues::Float32x2(sample_mips)) =
            mesh.attribute(Mesh::ATTRIBUTE_UV_1)
        else {
            panic!("自定义组合Mesh必须直接记录每顶点几何Mip");
        };
        for mip in 0..=3 {
            assert!(sample_mips.iter().any(|value| value[0] == mip as f32));
        }
        assert!(sample_mips.iter().all(|value| {
            value[1] == 0.0 && value[0].fract() == 0.0 && (0.0..=3.0).contains(&value[0])
        })); // Shader不再从层号和接缝标记二次推导。

        let bounds = ClipmapBounds::new(Vec2::ZERO, Vec2::new(73.0, 97.0)).unwrap();
        let config = ClipmapConfig {
            finest_cell_size: Vec2::ONE,
            half_cells: 4,
            levels: 4,
        }; // 只复用与Shader同式的世界固定中心吸附公式。
        for focus in [
            Vec2::ZERO,
            Vec2::new(73.0, 0.0),
            Vec2::new(0.0, 97.0),
            Vec2::new(73.0, 97.0), // 四角证明最外层从任意合法焦点覆盖全图。
            Vec2::new(3.1, 5.1),
            Vec2::new(69.7, 93.4), // 两组非整数相位检查各层独立吸附。
        ] {
            let placements: Vec<_> = (0..4)
                .map(|level| ClipmapPlacement {
                    level,
                    center: config.snapped_center(focus, bounds, level),
                    cell_size: Vec2::splat((1_u32 << level) as f32),
                })
                .collect();
            let outer_center = placements[3].center;
            let minimum = outer_center - Vec2::new(80.0, 104.0);
            let maximum = outer_center + Vec2::new(80.0, 104.0);
            for z in minimum.y as i32..maximum.y as i32 {
                for x in minimum.x as i32..maximum.x as i32 {
                    let point = Vec2::new(x as f32 + 0.233, z as f32 + 0.419);
                    assert_eq!(
                        containing_stabilized_triangle_count(&mesh, point, &placements),
                        1,
                        "自定义环带在焦点{focus:?}的{point:?}必须无洞无重叠"
                    );
                }
            }
        }
    }

    #[test] // 单层也必须使用作者指定倍率覆盖完整矩形，而不是隐式补出最细层。
    fn single_custom_band_uses_its_direct_geometry_mip() {
        let spec = SparseHeightSpec::new(
            [-32.0, -40.0],
            [32.0, 40.0], // 64×80米矩形，满足稀疏场景最小尺寸。
            1.0,
            -8.0,
            16.0,
        )
        .unwrap();
        let plan = compile_display_lod_plan(
            &spec,
            &[SceneTerrainDisplayLod {
                outer_half_extent_meters: None,
                spacing_multiplier: 4,
            }],
        )
        .unwrap();
        assert_eq!(plan.bands.len(), 1);
        assert_eq!(plan.actual_half_extents_meters, vec![[64.0, 80.0]]);
        let mesh = build_display_lod_mesh(&plan, Vec2::ONE).unwrap();
        assert_eq!(projected_area(&mesh), 128.0 * 160.0); // helper同时逐三角检查绕序和退化面。
        let Some(VertexAttributeValues::Float32x2(sample_mips)) =
            mesh.attribute(Mesh::ATTRIBUTE_UV_1)
        else {
            panic!("单层自定义Mesh必须携带直接几何Mip");
        };
        assert!(sample_mips.iter().all(|value| *value == [2.0, 0.0])); // ×4直接对应Mip 2，不伪造×1中心。
        assert_eq!(sample_mips.len() as u64, plan.vertex_count);
        assert_eq!(
            mesh.indices().unwrap().iter().count() as u64,
            plan.index_count
        );
    }

    #[test] // 短轴已覆盖后的末层侧向条带必须与共享编译计数一致且仍无洞、重叠或反面。
    fn rectangular_default_builds_side_only_outer_band() {
        let spec = SparseHeightSpec::new([0.0, 0.0], [256.0, 128.0], 1.0, -8.0, 16.0).unwrap();
        let authored = feathrise_scene_content::default_display_lods_for_spec(&spec).unwrap();
        let plan = compile_display_lod_plan(&spec, &authored).unwrap();
        let outer = plan.bands.last().unwrap();
        assert_eq!(outer.inner_half_cells, [32, 32]);
        assert_eq!(outer.outer_half_cells, [64, 32]); // ×4层只生成左右侧向条带。
        let mesh = build_display_lod_mesh(&plan, Vec2::ONE).unwrap();
        assert_eq!(projected_area(&mesh), 512.0 * 256.0); // 三层合计恰覆盖最终矩形一次。
        assert_eq!(
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
                .and_then(VertexAttributeValues::as_float3)
                .unwrap()
                .len() as u64,
            plan.vertex_count
        );
        assert_eq!(
            mesh.indices().unwrap().iter().count() as u64,
            plan.index_count
        );
        let bounds = ClipmapBounds::new(Vec2::ZERO, Vec2::new(256.0, 128.0)).unwrap();
        let config = ClipmapConfig {
            finest_cell_size: Vec2::ONE,
            half_cells: 64,
            levels: 3,
        };
        for focus in [Vec2::new(0.1, 0.1), Vec2::new(255.7, 127.6)] {
            let placements: Vec<_> = (0..3)
                .map(|level| ClipmapPlacement {
                    level,
                    center: config.snapped_center(focus, bounds, level),
                    cell_size: Vec2::splat((1_u32 << level) as f32),
                })
                .collect();
            for z in (0..128).step_by(17) {
                for x in (0..256).step_by(19) {
                    let point = Vec2::new(x as f32 + 0.233, z as f32 + 0.419);
                    assert_eq!(
                        containing_stabilized_triangle_count(&mesh, point, &placements),
                        1,
                        "侧向末环在边缘焦点{focus:?}的场景点{point:?}必须无洞无重叠"
                    );
                }
            }
        } // 两端焦点跨不同层格相位，抽样覆盖中心、完整环和仅左右延伸的末环。
    }
}

#[cfg(test)] // 使用实际生成Mesh作为独立三角面真值。
mod blade_surface_oracle_tests {
    // 验证通用草根查询与地形真实拓扑一致。
    use super::*; // 直接调用正式网格生成与稳定中心。
    use bevy::mesh::VertexAttributeValues; // 读取真实顶点及内缘Mip标记。
    use bevy::prelude::{Handle, UVec2}; // 纯CPU快照不创建GPU资源。
    use feathrise_environment::vegetation::blade::{
        GpuBladeClipmapBand, GpuBladeClipmapHeight, sample_clipmap_surface_for_validation,
    }; // 通用GPU算法的验证镜像。

    fn height(point: Vec2, mip: u32) -> f32 {
        // 非线性鞍形与Mip差值能够检测错误对角线及接缝。
        point.x * point.y * 0.031 + point.x * point.x * 0.007 + point.y.sin() + mip as f32 * 0.37 // 不采用会掩盖拓扑错误的单一平面。
    } // 顶点高度来自受控页采样器。

    #[test] // 中心、五角过渡、角点、页与非方形边界共同对照。
    fn blade_height_matches_actual_clipmap_mesh_triangles() {
        // 任何三角拓扑修改都会触发此回归。
        let config = ClipmapConfig {
            finest_cell_size: Vec2::new(1.25, 0.75),
            half_cells: 4,
            levels: 5,
        }; // 缩小固定模板保留所有接缝结构。
        let bounds = ClipmapBounds::new(Vec2::new(-3.0, -2.0), Vec2::new(34.0, 17.0)).unwrap(); // 不整除且非方形场景。
        let mesh = config.build_combined_mesh().unwrap(); // 正式生产网格即真值来源。
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("缺少地形位置");
        }; // 获取真实局部顶点。
        let Some(VertexAttributeValues::Float32x2(lods)) = mesh.attribute(Mesh::ATTRIBUTE_UV_1)
        else {
            panic!("缺少地形采样级别");
        }; // 接缝顶点必须采用细层Mip。
        let indices: Vec<usize> = mesh.indices().unwrap().iter().collect(); // 实际三角剖分，不猜测对角线。
        let mut degenerate_faces = 0; // 独立中心半格偏移可压扁粗层角面，该面没有实际栅格覆盖。
        let mut bands = [GpuBladeClipmapBand::default(); 16]; // 固定模板转换成正式可变环带描述。
        for (level, band) in bands.iter_mut().enumerate().take(config.levels as usize) {
            *band = GpuBladeClipmapBand {
                inner_half_cells: if level == 0 {
                    UVec2::ZERO
                } else {
                    UVec2::splat(config.half_cells / 2)
                },
                outer_half_cells: UVec2::splat(config.half_cells),
                geometry_mip: level as u32,
            };
        }
        for focus in [
            Vec2::new(13.0, 9.0),
            bounds.min,
            bounds.max,
            Vec2::new(28.0, 1.0),
            Vec2::new(-20.0, 35.0), // 全局相机落在源外时，草根与地形仍使用同一粗层中心。
        ] {
            // 中心移动和四边钳制。
            let center = config.snapped_center(focus, bounds, 0); // 生产吸附规则。
            let source = GpuBladeClipmapHeight {stitches: Default::default(),
                holes: [bevy::prelude::Vec4::ZERO; 32],
                hole_count: 0,
                // 只为调用采样镜像提供几何字段。
                atlas: Handle::default(),
                page_table: Handle::default(), // CPU验证不用真实纹理。
                world_min: bounds.min,
                world_max: bounds.max, // 与生产网格同边界。
                cell_size: config.finest_cell_size,
                logical_cells: UVec2::new(30, 26), // 有限世界映射。
                max_mip: 3,
                fine_center: center,
                bands,
                band_count: u32::from(config.levels), // 含最高Mip钳制。
                height_min: -100.0,
                height_max: 100.0,
                revision: [1, 1, 1],
                ready: true,
                missing_pages: [bevy::prelude::UVec4::ZERO; 16], // 完整页oracle不引入异步缺页。 // 完整有效快照。
            }; // 不创建CPU整图高度。
            let vertices: Vec<(Vec2, f32)> = positions
                .iter()
                .zip(lods)
                .map(|(position, lod)| {
                    // 按地形Shader实际变换每个顶点。
                    let level = lod[0] as u8; // UV_1.x 已直接记录普通或接缝顶点实际使用的几何Mip。
                    let point = Vec2::new(position[0], position[2])
                        + config.snapped_center(center, bounds, level); // Shader抵消fine_center后的结果。
                    let y = if point.cmpge(bounds.min).all() && point.cmple(bounds.max).all() {
                        height(point, u32::from(level).min(source.max_mip))
                    } else {
                        0.0
                    }; // 与地形场景外顶点规则一致。
                    (point, y) // 完整实际三维顶点。
                })
                .collect(); // 仅测试中允许完整遍历固定网格。
            let mut checked = 0; // 防止夹具未命中有效地形。
            for tri in indices.chunks_exact(3) {
                // 遍历真实三角索引。
                let ab = vertices[tri[1]].0 - vertices[tri[0]].0; // 实际变换后第一条投影边。
                let ac = vertices[tri[2]].0 - vertices[tri[0]].0; // 实际变换后第二条投影边。
                if ab.perp_dot(ac).abs() < 0.00000001 {
                    // 仅剔除GPU也不栅格化的零面积面，不放宽非退化采高容差。
                    degenerate_faces += 1; // 记录明确覆盖此合法拓扑情形。
                    continue; // 线段不存在唯一面内高度，不能对其重心进行面插值断言。
                } // 有面积的所有接缝、角点与普通面仍逐项严格验证。
                for weights in [[1.0 / 3.0; 3], [0.15, 0.25, 0.6]] {
                    // 重心和偏心均在真实面内。
                    let point = vertices[tri[0]].0 * weights[0]
                        + vertices[tri[1]].0 * weights[1]
                        + vertices[tri[2]].0 * weights[2]; // 几何插值位置。
                    if !point.cmpge(bounds.min).all() || !point.cmple(bounds.max).all() {
                        continue;
                    } // 场景外片元不会显示草。
                    let expected = vertices[tri[0]].1 * weights[0]
                        + vertices[tri[1]].1 * weights[1]
                        + vertices[tri[2]].1 * weights[2]; // 实际渲染面的高度。
                    let actual = sample_clipmap_surface_for_validation(&source, point, height)
                        .expect("场景内必须找到真实三角形"); // 通用Blade独立定位面。
                    assert!(
                        (actual - expected).abs() < 0.002,
                        "focus={focus:?}, point={point:?}, actual={actual}, expected={expected}, tri={:?}, area={}",
                        tri.iter().map(|index| vertices[*index]).collect::<Vec<_>>(),
                        ab.perp_dot(ac)
                    ); // 允许单精度运算误差。
                    checked += 1; // 记录确实覆盖三角面。
                } // 每面多个点防止偶然命中同值。
            } // 接缝和角点不跳过。
            // 源外相机让对象只落在少量粗三角形中；逐面一致性仍全部检查，不能要求近场样本数。
            let inside = center.cmpge(bounds.min).all() && center.cmple(bounds.max).all();
            assert!(checked > if inside { 100 } else { 0 }, "有效三角面样本不足：{center:?}, {checked}");
        } // 所有视图都独立验证。
        assert!(degenerate_faces > 0); // 保证夹具确实覆盖粗层中心偏移后的零面积角面。
    } // 高度真值不依赖采样实现内部拓扑。
} // 不在运行时引入CPU逐株工作。
