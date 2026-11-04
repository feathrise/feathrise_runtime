//! 任意三维地形网格的法线、室外顶层高度网格和 GPU 高度单元派生。

use bevy::math::Vec3; // 世界空间向量计算与Bevy运行时保持相同精度和坐标约定。

use crate::{
    // 共享输入、输出和硬预算集中从类型模块取得。
    MAX_TERRAIN_BIN_REFERENCES,
    MAX_TERRAIN_RASTER_BINS_PER_AXIS,
    MAX_TERRAIN_RASTER_TESTS,
    TerrainDerivedData,
    TerrainMeshInput,
    TerrainSampleSpec,
}; // 完成共享类型导入。

const FACE_EPSILON: f32 = 1.0e-16; // 密度等值面会产生合法毫米级小面，不能套用高度投影的面积阈值删除它们。
const HEIGHT_PROJECTION_EPSILON: f32 = 1.0e-8; // 高度投影仍使用原门限，竖壁只进入真实三维查询。
const BARYCENTRIC_EPSILON: f32 = 1.0e-5; // 共享边上的浮点误差不得制造假缺口。

/// 校验固定高度网格的采样分辨率和世界范围。
pub(crate) fn validate_sample_spec(spec: TerrainSampleSpec) -> Result<(), String> {
    let [width, height] = spec.resolution; // 拆出两轴顶点采样数。
    if width < 2 || height < 2 || width > 513 || height > 513 {
        return Err("动态地形高度分辨率必须位于2×2至513×513".to_owned()); // 保持现有内存和Solari上限。
    } // 完成分辨率门。
    validate_world_height_bounds(spec)?; // 高度采样与新建平面共用世界及高度量程门。
    Ok(()) // 规格可以安全进入后台派生。
} // 结束采样规格校验。

/// 校验与网格细分无关的地图范围和高度量程，供平面生成及高度采样分别复用。
pub(crate) fn validate_world_height_bounds(spec: TerrainSampleSpec) -> Result<(), String> {
    if !spec // 所有世界与高度参数都必须可进入有限浮点计算。
        .world_min
        .iter()
        .chain(spec.world_max.iter())
        .chain([&spec.base_height, &spec.height_scale])
        .all(|value| value.is_finite())
        || spec.world_min[0] >= spec.world_max[0]
        || spec.world_min[1] >= spec.world_max[1]
        || spec.height_scale <= 0.0
        || !(spec.world_max[0] - spec.world_min[0]).is_finite()
        || !(spec.world_max[1] - spec.world_min[1]).is_finite()
        || !(spec.base_height + spec.height_scale).is_finite()
    {
        return Err("动态地形世界范围或高度量程无效".to_owned()); // 非有限或倒置范围整份拒绝。
    } // 完成范围门。
    Ok(()) // 世界跨度和完整高度量程都能安全参与插值。
} // 结束共享范围校验。

/// 校验唯一地形 primitive 的 TriangleList 与任意三维曲面局部条件。
pub(crate) fn validate_mesh(mesh: &TerrainMeshInput) -> Result<(), String> {
    if mesh.positions.is_empty() {
        return Err("动态地形必须提供至少一个顶点".to_owned()); // 顶点不再设置独立数量上限，但空网格仍不能进入派生。
    } // 完成顶点总量门。
    if mesh.indices.is_empty() || !mesh.indices.len().is_multiple_of(3) {
        return Err("动态地形必须提供非空且为3倍数的TriangleList索引".to_owned()); // 不猜测条带或扇形拓扑。
    } // 完成索引形状门。
    let triangle_count = mesh.indices.len() / 3; // TriangleList每三个索引形成一个面。
    if u32::try_from(triangle_count).is_err() {
        return Err(format!(
            "动态地形三角形数{triangle_count}超过U32三角身份可表示范围"
        )); // 空间桶和投影索引使用U32三角身份，不能允许截断回绕。
    } // 完成三角身份表示门；不再施加独立数量预算。
    if mesh.positions.len() > mesh.indices.len() {
        return Err(format!(
            "动态地形顶点数{}超过逻辑索引数{}，必然包含未被三角形引用的顶点",
            mesh.positions.len(),
            mesh.indices.len()
        )); // 没有独立顶点上限后，仍用既有全顶点必须参与拓扑的契约阻止无意义大属性表。
    } // 完成顶点可引用性必要条件门；精确孤立顶点仍由法线派生检查。
    if mesh
        .positions
        .iter()
        .any(|position| !Vec3::from_array(*position).is_finite())
    {
        return Err("动态地形POSITION包含NaN或无穷值".to_owned()); // 非有限顶点会污染高度、包围盒和物理。
    } // 完成顶点有限值门。
    for (triangle_index, triangle) in mesh.indices.chunks_exact(3).enumerate() {
        let [a, b, c] = triangle else { unreachable!() }; // chunks_exact已保证固定三个索引。
        let Some(first) = mesh.positions.get(*a as usize) else {
            return Err(format!("动态地形三角形{triangle_index}索引{a}越界")); // 损坏索引必须指出首个面。
        }; // 完成首顶点读取。
        let Some(second) = mesh.positions.get(*b as usize) else {
            return Err(format!("动态地形三角形{triangle_index}索引{b}越界")); // 损坏索引必须指出首个面。
        }; // 完成次顶点读取。
        let Some(third) = mesh.positions.get(*c as usize) else {
            return Err(format!("动态地形三角形{triangle_index}索引{c}越界")); // 损坏索引必须指出首个面。
        }; // 完成末顶点读取。
        let cross = (Vec3::from_array(*second) - Vec3::from_array(*first))
            .cross(Vec3::from_array(*third) - Vec3::from_array(*first)); // 保留面积权重的世界面法线。
        if !cross.is_finite() || cross.length_squared() <= FACE_EPSILON {
            return Err(format!(
                "动态地形三角形{triangle_index}三维面积退化；请在Blender中清理重合点或零面积面"
            )); // 真正零面积面既不能稳定渲染，也不能建立可靠Trimesh。
        } // 完成单面三维面积门。
    } // 完成全部三角校验。
    Ok(()) // 网格可进入法线和高度派生。
} // 结束网格校验。

/// 按实际任意拓扑重算法线；面绕序完整保留以支持洞顶、洞壁和倒扣。
pub fn derive_vertex_normals(mesh: &TerrainMeshInput) -> Result<Vec<[f32; 3]>, String> {
    validate_mesh(mesh)?; // 法线派生必须独立保持完整输入校验。
    derive_vertex_normals_validated(mesh) // 完整地形派生已校验输入时复用同一累加逻辑，避免再次扫描数千万三角形。
} // 结束对外法线派生入口。

/// 为已经通过网格校验的拓扑累计面积加权法线。
fn derive_vertex_normals_validated(mesh: &TerrainMeshInput) -> Result<Vec<[f32; 3]>, String> {
    let mut sums = vec![Vec3::ZERO; mesh.positions.len()]; // 面积加权累加保持共享顶点平滑。
    let mut first_faces = vec![u32::MAX; mesh.positions.len()]; // 只存首面编号而非三维法线，大网格可少占每顶点八字节。
    for (triangle_index, triangle) in mesh.indices.chunks_exact(3).enumerate() {
        let a = triangle[0] as usize; // 读取当前面的三个稳定顶点索引。
        let b = triangle[1] as usize; // 读取当前面的三个稳定顶点索引。
        let c = triangle[2] as usize; // 读取当前面的三个稳定顶点索引。
        let first = Vec3::from_array(mesh.positions[a]); // 转为Bevy向量执行面积权重计算。
        let second = Vec3::from_array(mesh.positions[b]); // 转为Bevy向量执行面积权重计算。
        let third = Vec3::from_array(mesh.positions[c]); // 转为Bevy向量执行面积权重计算。
        let weighted = (second - first).cross(third - first); // 长度等于平行四边形面积。
        sums[a] += weighted; // 每个角共享同一面面积权重。
        sums[b] += weighted; // 每个角共享同一面面积权重。
        sums[c] += weighted; // 每个角共享同一面面积权重。
        if first_faces[a] == u32::MAX {
            first_faces[a] = triangle_index as u32; // 记录首个按作者绕序计算的面身份。
        }
        if first_faces[b] == u32::MAX {
            first_faces[b] = triangle_index as u32; // 属性拆点通常只会命中同一平滑扇区。
        }
        if first_faces[c] == u32::MAX {
            first_faces[c] = triangle_index as u32; // 洞穴面不得回退为固定世界上方向。
        }
    } // 完成面积法线累加。
    if let Some(index) = first_faces.iter().position(|face| *face == u32::MAX) {
        return Err(format!("动态地形顶点{index}没有被任何三角形引用")); // 孤立顶点会破坏保存与编辑语义。
    } // 完成孤立顶点门。
    let normals = sums
        .into_iter()
        .enumerate()
        .map(|(index, sum)| {
            sum.try_normalize()
                .or_else(|| {
                    let base = first_faces[index] as usize * 3; // 只在相反绕序抵消时回读该顶点的首面。
                    let [a, b, c] = [
                        mesh.positions[mesh.indices[base] as usize], // 首面第一个世界顶点。
                        mesh.positions[mesh.indices[base + 1] as usize], // 首面第二个世界顶点。
                        mesh.positions[mesh.indices[base + 2] as usize], // 首面第三个世界顶点。
                    ]; // 重新求首面法线，不为每个顶点常驻三维回退向量。
                    (Vec3::from_array(b) - Vec3::from_array(a))
                        .cross(Vec3::from_array(c) - Vec3::from_array(a))
                        .try_normalize() // 非流形或相反绕序抵消时仍保留作者首面方向。
                })
                .map(|normal| normal.to_array()) // Bevy 0.19的to_array借用self，闭包适配Option拥有值。
                .ok_or_else(|| format!("动态地形顶点{index}没有可用的相邻面法线"))
        })
        .collect::<Result<Vec<_>, _>>()?; // 先形成完整候选，失败不暴露半份法线。
    Ok(normals) // 返回与源顶点严格等长的完整法线。
} // 结束任意拓扑法线派生。

/// 把高度顶点按 NW、NE、SW、SE 打包为 RGBA16_UNORM 小端基础级字节。
pub fn pack_height_cells_rgba16(
    heights: &[u16],      // Z行X列排列的完整顶点高度。
    resolution: [u32; 2], // 输出尺寸固定为每轴减一。
) -> Result<Vec<u8>, String> {
    let [width, height] = resolution; // 拆出顶点网格宽高。
    if width < 2 || height < 2 || heights.len() != width as usize * height as usize {
        return Err("高度单元输入尺寸与顶点高度数量不一致".to_owned()); // 不编码部分或空纹理。
    } // 完成高度输入门。
    let mut bytes = Vec::with_capacity((width - 1) as usize * (height - 1) as usize * 8); // 每单元四个u16。
    for z in 0..height - 1 {
        for x in 0..width - 1 {
            let north_west = heights[(z * width + x) as usize]; // 当前行当前列。
            let north_east = heights[(z * width + x + 1) as usize]; // 当前行下一列。
            let south_west = heights[((z + 1) * width + x) as usize]; // 下一行当前列。
            let south_east = heights[((z + 1) * width + x + 1) as usize]; // 下一行下一列。
            for value in [north_west, north_east, south_west, south_east] {
                bytes.extend_from_slice(&value.to_le_bytes()); // KTX2旧契约与内存Image都使用小端通道字节。
            } // 完成当前单元四通道编码。
        } // 完成当前Z行单元。
    } // 完成全部高度单元编码。
    Ok(bytes) // 输出不生成Mip，也不写任何磁盘文件。
} // 结束高度单元打包。

/// 从一个完整三维 GLB 网格派生运行时所需的全部地形数据。
pub fn derive_terrain(
    mesh: TerrainMeshInput,  // 后台任务拥有冻结的完整世界空间输入。
    spec: TerrainSampleSpec, // 清单采样规格与任务身份由调用方绑定。
) -> Result<TerrainDerivedData, String> {
    validate_sample_spec(spec)?; // 先拒绝损坏清单，避免按异常尺寸分配。
    validate_mesh(&mesh)?; // 先拒绝损坏拓扑，避免构建大型空间桶。
    validate_vertex_height_range(&mesh, spec)?; // 地图范围外的顶点也属于地形，不能绕过量程门。
    let normals = derive_vertex_normals_validated(&mesh)?; // 已校验网格直接累计法线，避免超大地图重复拓扑扫描。
    let heights = if let Some(grid) = infer_rectilinear_grid(&mesh).filter(|grid| {
        grid.world_min[0] <= spec.world_min[0]
            && grid.world_min[1] <= spec.world_min[1]
            && grid.world_max[0] >= spec.world_max[0]
            && grid.world_max[1] >= spec.world_max[1]
    }) {
        rasterize_rectilinear_grid(&mesh, spec, grid)? // 规则地形直接按格定位，避免为数千万三角形建立空间桶。
    } else {
        rasterize_heights(&mesh, spec)? // 洞穴和任意拓扑继续使用有界空间桶求最高包络。
    }; // 两条路径使用同一高度量化与完整覆盖契约。
    let height_cells_rgba16 = pack_height_cells_rgba16(&heights, spec.resolution)?; // 只在内存形成GPU基础级。
    let (min, max) = mesh_bounds(&mesh.positions); // 输入有限且非空，包围盒必然有效。
    Ok(TerrainDerivedData {
        positions: mesh.positions, // 原样保留源顶点顺序。
        normals,                   // 安装重算后的安全平滑法线。
        indices: mesh.indices,     // 原样保留源三角顺序和连接关系。
        heights,                   // 供水域与Mesh植被共享。
        height_cells_rgba16,       // 供GPU Blade高度采样共享。
        min,                       // 发布真实地形包围盒下界。
        max,                       // 发布真实地形包围盒上界。
    }) // 完成原子派生结果构造。
} // 结束完整地形派生。

/// 校验地形全部顶点的世界 Y 都位于清单编码量程，而不只检查规则采样点。
fn validate_vertex_height_range(
    mesh: &TerrainMeshInput,
    spec: TerrainSampleSpec,
) -> Result<(), String> {
    let maximum = spec.base_height + spec.height_scale; // 规格已验证有限且跨度为正。
    for (index, position) in mesh.positions.iter().enumerate() {
        if position[1] < spec.base_height || position[1] > maximum {
            return Err(format!(
                "动态地形顶点{index}高度{:.4}超出清单可表示范围{:.4}..{maximum:.4}",
                position[1], spec.base_height
            )); // 即使该顶点位于声明X/Z范围外，也不能让非法作者数据静默进入保存或碰撞。
        }
    }
    Ok(())
} // 结束全部顶点高度量程校验。

/// 计算有限非空位置表的三轴包围盒。
pub(crate) fn mesh_bounds(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3]; // 从正无穷开始累计每轴下界。
    let mut max = [f32::NEG_INFINITY; 3]; // 从负无穷开始累计每轴上界。
    for position in positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(position[axis]); // 更新当前轴下界。
            max[axis] = max[axis].max(position[axis]); // 更新当前轴上界。
        } // 完成当前顶点三轴累计。
    } // 完成全部顶点包围盒累计。
    (min, max) // 返回可直接写入GLB accessor的有限边界。
} // 结束包围盒计算。

/// 严格识别新地图采用的行优先等距矩形网格；高度可变，但 X/Z 格线和每格两个三角形必须完整。
#[derive(Clone, Copy, Debug)]
pub(crate) struct RectilinearGrid {
    pub(crate) width: usize,        // 每行的共享顶点数量。
    pub(crate) height: usize,       // 行数，包含地图两条 Z 边。
    pub(crate) world_min: [f32; 2], // 第一行、第一列的 X/Z 坐标。
    pub(crate) world_max: [f32; 2], // 最后一行、最后一列的 X/Z 坐标。
    pub(crate) step: [f32; 2],      // 两轴真实等距步长，供编辑器直接定位单元。
}

/// 只有完整规则拓扑才可跳过通用空间桶；缺面、洞穴、额外层或重排索引均回到通用路径。
pub(crate) fn infer_rectilinear_grid(mesh: &TerrainMeshInput) -> Option<RectilinearGrid> {
    let positions = &mesh.positions; // 识别器只读已验证的有限位置与索引。
    let first_z = positions.first()?[2]; // 第一行应具有相同的 Z 坐标。
    let width = positions.iter().position(|point| point[2] != first_z)?; // 第二行首点决定行宽。
    if width < 2 || positions.len() % width != 0 {
        return None; // 不完整行或单列不可能覆盖完整矩形。
    }
    let height = positions.len() / width; // 已确认所有行可整除。
    let width_u32 = u32::try_from(width).ok()?; // 规则顶点身份与 GLB U32 索引保持一致。
    let height_u32 = u32::try_from(height).ok()?; // 避免大网格尺寸转换时截断回绕。
    let expected_indices = (width - 1)
        .checked_mul(height.checked_sub(1)?)?
        .checked_mul(6)?; // 每格严格两个三角形。
    if height < 2 || mesh.indices.len() != expected_indices {
        return None; // 缺面、叠层或附加洞壁不得被误判为普通高度场。
    }
    let world_min = [positions[0][0], first_z]; // 取真实左上角而非依赖调用方的清单范围。
    let world_max = [positions[width - 1][0], positions[(height - 1) * width][2]]; // 取真实右下角。
    let step = [
        (world_max[0] - world_min[0]) / (width - 1) as f32, // X 单元米制长度。
        (world_max[1] - world_min[1]) / (height - 1) as f32, // Z 单元米制长度。
    ];
    if !step.iter().all(|value| value.is_finite() && *value > 0.0) {
        return None; // 非有限或无法分辨的细分不可供直接定位。
    }
    for row in 0..height {
        let z = positions[row * width][2]; // 当前行真实 Z 格线由平面生成器统一计算。
        if z != sample_axis(world_min[1], world_max[1], row as u32, height_u32)
            || (row > 0 && z <= positions[(row - 1) * width][2])
        {
            return None; // 只接收与平面生成器完全一致且严格递增的等距格线。
        }
        for column in 0..width {
            let point = positions[row * width + column]; // 当前共享顶点的 X/Z 必须构成矩形格线。
            if point[0] != positions[column][0] || point[2] != z {
                return None; // 允许雕刻 Y，但不把横向挪点或属性拆点当成规则网格。
            }
            if row == 0
                && (point[0] != sample_axis(world_min[0], world_max[0], column as u32, width_u32)
                    || (column > 0 && point[0] <= positions[column - 1][0]))
            {
                return None; // X 格线必须与创建器等距采样完全一致。
            }
        }
    }
    for (cell_index, triangles) in mesh.indices.chunks_exact(6).enumerate() {
        let row = cell_index / (width - 1); // 由行优先单元编号还原 Z 行。
        let column = cell_index % (width - 1); // 还原该行的 X 列。
        let a = row * width + column; // 当前单元北西角。
        let Some(a) = u32::try_from(a).ok() else {
            return None;
        }; // 顶点索引必须保持 U32 身份。
        let Some(c) = a.checked_add(width_u32) else {
            return None;
        }; // 当前单元南西角。
        let Some(b) = a.checked_add(1) else {
            return None;
        }; // 当前单元北东角。
        let Some(d) = c.checked_add(1) else {
            return None;
        }; // 当前单元南东角。
        if triangles != [a, c, b, b, c, d] {
            return None; // 严格保留新地图 NE/SW 对角及作者绕序。
        }
    }
    Some(RectilinearGrid {
        width,
        height,
        world_min,
        world_max,
        step,
    }) // 无洞完整网格可供派生与编辑直接定位。
}

/// 在递增 X 或 Z 格线中找到采样点所在单元，包括最右端点所属的最后一格。
fn rectilinear_cell(
    positions: &[[f32; 3]],
    value: f32,
    count: usize,
    stride: usize,
    axis: usize,
) -> usize {
    let mut lower = 0usize; // 二分左界包含仍可能命中的格线。
    let mut upper = count; // 二分右界不包含已经排除的格线。
    while lower < upper {
        let middle = lower + (upper - lower) / 2; // 避免求和溢出。
        if positions[middle * stride][axis] <= value {
            lower = middle + 1; // 采样点位于当前格线或其右侧。
        } else {
            upper = middle; // 采样点位于当前格线左侧。
        }
    }
    lower.saturating_sub(1).min(count - 2) // 边界点归入合法的相邻单元。
}

/// 已验证规则格网仅查询每条采样竖线所属的两个三角形，工作量与固定高度采样数成正比。
fn rasterize_rectilinear_grid(
    mesh: &TerrainMeshInput, // 完整规则网格的真实顶点高度。
    spec: TerrainSampleSpec, // 室外系统固定高度采样规格。
    grid: RectilinearGrid,   // 已完成全部格线与三角拓扑校验。
) -> Result<Vec<u16>, String> {
    let [sample_width, sample_height] = spec.resolution; // 高度采样仍独立于真实 GLB 细分。
    let mut heights = Vec::with_capacity(sample_width as usize * sample_height as usize); // 只分配输出的固定尺寸缓存。
    let tolerance = (spec.height_scale / u16::MAX as f32 * 0.5).max(1.0e-4); // 与通用栅格化共用量化容差。
    for z in 0..sample_height {
        let world_z = sample_axis(spec.world_min[1], spec.world_max[1], z, sample_height); // 端点必须精确命中清单边界。
        let row = rectilinear_cell(&mesh.positions, world_z, grid.height, grid.width, 2); // 二分定位 Z 单元。
        for x in 0..sample_width {
            let world_x = sample_axis(spec.world_min[0], spec.world_max[0], x, sample_width); // 取得真实 X 采样位置。
            let column = rectilinear_cell(&mesh.positions, world_x, grid.width, 1, 0); // 二分定位 X 单元。
            let a = row * grid.width + column; // 北西角；其余角在完整共享拓扑中具有固定偏移。
            let b = a + 1; // 北东角。
            let c = a + grid.width; // 南西角。
            let d = c + 1; // 南东角。
            let first = vertical_intersection(
                world_x,
                world_z,
                [mesh.positions[a], mesh.positions[c], mesh.positions[b]],
            ); // 查询 NE/SW 对角一侧。
            let second = vertical_intersection(
                world_x,
                world_z,
                [mesh.positions[b], mesh.positions[c], mesh.positions[d]],
            ); // 查询对角另一侧。
            let y = match (first, second) {
                (Some(first), Some(second)) => Some(first.max(second)), // 共享边浮点误差按最高室外表面处理。
                (Some(y), None) | (None, Some(y)) => Some(y), // 单个三角命中足以给出真实表面。
                (None, None) => None,                         // 缺口仍需按通用路径的覆盖契约报告。
            };
            let Some(y) = y else {
                return Err(format!(
                    "动态地形在X={world_x:.3} Z={world_z:.3}整列没有任何可采样表面；请保留洞穴地板或室外地面覆盖"
                )); // 不用规则格网身份掩盖实际采样失败。
            };
            heights.push(quantize_height(y, spec, tolerance)?); // 保持与任意拓扑相同的 u16 最近量化。
        }
    }
    Ok(heights) // 输出供草、水和室外高度查询共享的完整高度缓存。
}

/// 用固定128×128上限空间桶限制每个高度采样点的候选三角形集合。
fn rasterize_heights(mesh: &TerrainMeshInput, spec: TerrainSampleSpec) -> Result<Vec<u16>, String> {
    let [width, height] = spec.resolution; // 顶点高度网格包含四条地图边。
    let bin_width = (width - 1).min(MAX_TERRAIN_RASTER_BINS_PER_AXIS).max(1); // 小图不建立空桶。
    let bin_height = (height - 1).min(MAX_TERRAIN_RASTER_BINS_PER_AXIS).max(1); // 非方形地图独立取Z桶数。
    let mut bins = vec![Vec::<u32>::new(); (bin_width * bin_height) as usize]; // 后台独占桶集合。
    let mut bin_references = 0usize; // 记录真实引用数并执行固定内存门。
    for (triangle_index, triangle) in mesh.indices.chunks_exact(3).enumerate() {
        let points = [
            mesh.positions[triangle[0] as usize], // 当前面首顶点世界位置。
            mesh.positions[triangle[1] as usize], // 当前面次顶点世界位置。
            mesh.positions[triangle[2] as usize], // 当前面末顶点世界位置。
        ]; // 完成当前面小型位置快照。
        let min_x = points
            .iter()
            .map(|point| point[0])
            .fold(f32::INFINITY, f32::min); // 投影包围盒X下界。
        let max_x = points
            .iter()
            .map(|point| point[0])
            .fold(f32::NEG_INFINITY, f32::max); // 投影包围盒X上界。
        let min_z = points
            .iter()
            .map(|point| point[2])
            .fold(f32::INFINITY, f32::min); // 投影包围盒Z下界。
        let max_z = points
            .iter()
            .map(|point| point[2])
            .fold(f32::NEG_INFINITY, f32::max); // 投影包围盒Z上界。
        let Some([first_x, last_x]) = projected_bin_range(
            min_x,
            max_x,
            spec.world_min[0],
            spec.world_max[0],
            bin_width,
        ) else {
            continue; // 完全位于声明X范围外的面不影响高度覆盖。
        }; // 完成X桶范围裁剪。
        let Some([first_z, last_z]) = projected_bin_range(
            min_z,
            max_z,
            spec.world_min[1],
            spec.world_max[1],
            bin_height,
        ) else {
            continue; // 完全位于声明Z范围外的面不影响高度覆盖。
        }; // 完成Z桶范围裁剪。
        for bin_z in first_z..=last_z {
            for bin_x in first_x..=last_x {
                bin_references = bin_references.saturating_add(1); // 饱和计数防御异常输入整数溢出。
                if bin_references > MAX_TERRAIN_BIN_REFERENCES {
                    return Err(format!(
                        "动态地形空间分桶引用超过{}预算；请减少跨越整图的大三角形或极端密集的洞穴层",
                        MAX_TERRAIN_BIN_REFERENCES
                    )); // 明确拒绝异常工作集而不是拖慢整个进程。
                } // 完成桶引用内存门。
                bins[(bin_z * bin_width + bin_x) as usize].push(triangle_index as u32); // 当前面只登记到投影相交桶。
            } // 完成当前Z桶行。
        } // 完成当前面的全部桶登记。
    } // 完成空间桶构建。
    let mut heights = Vec::with_capacity(width as usize * height as usize); // 输出在成功前仍只属于后台任务。
    let mut tested = 0usize; // 候选测试数量保持固定最坏停止门。
    let duplicate_tolerance = (spec.height_scale / u16::MAX as f32 * 0.5).max(1.0e-4); // 半量化级以内视作共享边同一点。
    for z in 0..height {
        let world_z = sample_axis(spec.world_min[1], spec.world_max[1], z, height); // 端点严格命中Z边界。
        let bin_z = point_bin(world_z, spec.world_min[1], spec.world_max[1], bin_height); // 取得当前采样Z桶。
        for x in 0..width {
            let world_x = sample_axis(spec.world_min[0], spec.world_max[0], x, width); // 端点严格命中X边界。
            let bin_x = point_bin(world_x, spec.world_min[0], spec.world_max[0], bin_width); // 取得当前采样X桶。
            let candidates = &bins[(bin_z * bin_width + bin_x) as usize]; // 只遍历局部投影候选。
            let mut highest = None::<f32>; // 当前X/Z允许多个真实交点，高度缓存只记录最高室外包络。
            for &triangle_index in candidates {
                tested = tested.saturating_add(1); // 记录本任务实际候选工作量。
                if tested > MAX_TERRAIN_RASTER_TESTS {
                    return Err(format!(
                        "动态地形高度栅格化候选测试超过{}预算；请减少同一区域的极端层数或细分异常大面",
                        MAX_TERRAIN_RASTER_TESTS
                    )); // 后台任务也必须有固定停止条件。
                } // 完成候选计算门。
                let base = triangle_index as usize * 3; // 定位当前候选三角的首索引。
                let triangle = [
                    mesh.positions[mesh.indices[base] as usize], // 读取候选首顶点。
                    mesh.positions[mesh.indices[base + 1] as usize], // 读取候选次顶点。
                    mesh.positions[mesh.indices[base + 2] as usize], // 读取候选末顶点。
                ]; // 完成候选位置读取。
                let Some(y) = vertical_intersection(world_x, world_z, triangle) else {
                    continue; // 投影包围盒相交但真实三角形不包含该采样点。
                }; // 完成候选垂直命中。
                highest = Some(highest.map_or(y, |previous| previous.max(y))); // 同高共享边自动合并，多层只更新最高室外表面。
            } // 完成当前采样点局部候选遍历。
            let Some(y) = highest else {
                return Err(format!(
                    "动态地形在X={world_x:.3} Z={world_z:.3}整列没有任何可采样表面；请保留洞穴地板或室外地面覆盖"
                )); // 洞穴入口可以多层或开口，但每条规则竖线仍需至少命中一块真实地形。
            }; // 完成覆盖门。
            heights.push(quantize_height(y, spec, duplicate_tolerance)?); // 使用最近u16量化并拒绝越界。
        } // 完成当前Z行全部X采样。
    } // 完成全部规则高度采样。
    Ok(heights) // 返回完整的室外最高包络CPU高度网格。
} // 结束空间分桶栅格化。

/// 把一个投影包围盒裁剪并映射到连续桶索引范围。
fn projected_bin_range(
    min_value: f32, // 三角投影轴下界。
    max_value: f32, // 三角投影轴上界。
    world_min: f32, // 地图声明轴下界。
    world_max: f32, // 地图声明轴上界。
    bins: u32,      // 当前轴桶数。
) -> Option<[u32; 2]> {
    if max_value < world_min || min_value > world_max {
        return None; // 投影完全位于声明范围外。
    } // 完成快速相离判断。
    let first = point_bin(min_value.max(world_min), world_min, world_max, bins); // 裁剪后取得首桶。
    let last = point_bin(max_value.min(world_max), world_min, world_max, bins); // 裁剪后取得末桶。
    Some([first, last]) // 返回闭区间供三角登记。
} // 结束投影桶范围映射。

/// 把一个范围内世界坐标稳定映射到零基桶索引。
fn point_bin(value: f32, world_min: f32, world_max: f32, bins: u32) -> u32 {
    let normalized = ((value - world_min) / (world_max - world_min)).clamp(0.0, 1.0); // 边界误差裁回合法区间。
    ((normalized * bins as f32).floor() as u32).min(bins - 1) // 右端点归入最后一桶。
} // 结束单点桶映射。

/// 按包含端点的规则网格取得一个轴的世界采样值。
fn sample_axis(min: f32, max: f32, index: u32, count: u32) -> f32 {
    if index + 1 == count {
        max // 末采样显式返回原上界，避免乘加舍入漏掉边缘。
    } else {
        min + (max - min) * index as f32 / (count - 1) as f32 // 其余采样按等距比例取得。
    } // 完成端点稳定采样。
} // 结束规则轴采样。

/// 求世界竖线与一个非投影退化三角形的交点高度；绕序不影响结果。
fn vertical_intersection(x: f32, z: f32, triangle: [[f32; 3]; 3]) -> Option<f32> {
    let [a, b, c] = triangle; // 拆出三个世界空间顶点。
    let denominator = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]); // XZ重心分母。
    if denominator.abs() <= HEIGHT_PROJECTION_EPSILON {
        return None; // 垂直洞壁没有单一高度，只参与渲染和Trimesh而不进入高度包络。
    } // 完成投影退化门。
    let first = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / denominator; // A权重。
    let second = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / denominator; // B权重。
    let third = 1.0 - first - second; // C权重由总和一取得。
    if first < -BARYCENTRIC_EPSILON
        || second < -BARYCENTRIC_EPSILON
        || third < -BARYCENTRIC_EPSILON
        || first > 1.0 + BARYCENTRIC_EPSILON
        || second > 1.0 + BARYCENTRIC_EPSILON
        || third > 1.0 + BARYCENTRIC_EPSILON
    {
        return None; // 当前点只在投影包围盒内，不在真实三角形内。
    } // 完成重心范围门。
    Some(first * a[1] + second * b[1] + third * c[1]) // 线性插值得到唯一世界Y。
} // 结束垂直命中计算。

/// 把世界Y严格量化为清单声明的u16范围。
fn quantize_height(
    y: f32,                  // 当前采样世界高度。
    spec: TerrainSampleSpec, // 提供编码下界与跨度。
    tolerance: f32,          // 只吸收共享边和端点浮点误差。
) -> Result<u16, String> {
    let maximum = spec.base_height + spec.height_scale; // 计算完整可表示上界。
    if !y.is_finite() || y < spec.base_height - tolerance || y > maximum + tolerance {
        return Err(format!(
            "动态地形高度{y:.4}超出清单可表示范围{:.4}..{maximum:.4}",
            spec.base_height
        )); // 不静默截断Blender的真实越界地形。
    } // 完成高度量程门。
    let normalized = ((y.clamp(spec.base_height, maximum) - spec.base_height) / spec.height_scale)
        .clamp(0.0, 1.0); // 仅把容差内误差收回端点。
    Ok((normalized * u16::MAX as f32).round() as u16) // 采用最近整数保持旧R16解码精度。
} // 结束高度量化。

#[cfg(test)]
mod tests {
    use std::time::Instant; // 性能规格测试记录三档生成耗时，不参与算法路径。

    use super::*; // 纯算法测试直接覆盖私有栅格辅助函数。

    fn flat_quad(y: f32) -> TerrainMeshInput {
        TerrainMeshInput {
            positions: vec![
                [-1.0, y, -1.0],
                [1.0, y, -1.0],
                [-1.0, y, 1.0],
                [1.0, y, 1.0],
            ], // 四个共享顶点。
            indices: vec![0, 2, 1, 1, 2, 3], // 与现有动态地形朝上绕序一致。
        } // 完成测试平面构造。
    } // 结束测试平面夹具。

    fn spec() -> TerrainSampleSpec {
        TerrainSampleSpec {
            resolution: [3, 3],      // 覆盖边界、中心和共享对角线。
            world_min: [-1.0, -1.0], // 与测试平面边界一致。
            world_max: [1.0, 1.0],   // 与测试平面边界一致。
            base_height: -10.0,      // 平面位于量程中点。
            height_scale: 20.0,      // 完整范围为负十至正十。
        } // 完成测试采样规格。
    } // 结束测试规格夹具。

    fn regular_mesh(resolution: u32) -> TerrainMeshInput {
        let spec = TerrainSampleSpec {
            resolution: [resolution, resolution],
            world_min: [-512.0, -512.0], // 最大规格仍保持2m单元，避免测试夹具自身形成亚毫米退化面。
            world_max: [512.0, 512.0],
            base_height: -1.0,
            height_scale: 2.0,
        };
        crate::build_flat_terrain_mesh(spec, 0.0).expect("合法规则性能夹具必须可构建")
    }

    #[test]
    fn rectangular_grid_with_sculpted_heights_matches_general_rasterization() {
        let xs = [0.0f32, 1.0, 2.0, 3.0]; // 等距 X 格线采用新地图生成器的实际取值。
        let zs = [0.0f32, 1.0, 2.0]; // 非方形地图的 Z 轴可与 X 轴独立细分。
        let mut positions = Vec::new(); // 构造完整共享顶点矩形地形。
        for z in zs {
            for x in xs {
                positions.push([x, x + z, z]); // 非平坦高度能检验两条三角插值路径。
            }
        }
        let mut indices = Vec::new(); // 保持新地图的 NE/SW 对角拓扑。
        for row in 0..zs.len() - 1 {
            for column in 0..xs.len() - 1 {
                let a = (row * xs.len() + column) as u32; // 当前单元北西角。
                let c = a + xs.len() as u32; // 当前单元南西角。
                indices.extend_from_slice(&[a, c, a + 1, a + 1, c, c + 1]); // 两个朝上三角。
            }
        }
        let mesh = TerrainMeshInput { positions, indices }; // 冻结规则但非等距、非平坦的真实拓扑。
        let spec = TerrainSampleSpec {
            resolution: [7, 5],    // 室外采样与 GLB 网格分辨率独立。
            world_min: [0.0, 0.0], // 采样包含矩形左上角。
            world_max: [3.0, 2.0], // 采样包含矩形右下角。
            base_height: -1.0,     // 保证所有真实高度进入可量化范围。
            height_scale: 10.0,    // 保证所有真实高度进入可量化范围。
        };
        let grid = infer_rectilinear_grid(&mesh).expect("完整规则拓扑应采用直接定位路径"); // 锁定大图路径的识别门。
        let direct = rasterize_rectilinear_grid(&mesh, spec, grid).expect("规则采样应完整覆盖"); // 每点只查局部两面。
        let general = rasterize_heights(&mesh, spec).expect("通用空间桶仍可处理此小网格"); // 对比既有高度语义。
        assert_eq!(direct, general); // 规则优化不能改变草、水与高度查询结果。
    }

    #[test]
    fn rectilinear_shortcut_requires_exact_topology_and_horizontal_grid() {
        let mesh = regular_mesh(4); // 完整新建网格可进入直接采样路径。
        assert!(infer_rectilinear_grid(&mesh).is_some()); // 完整规则网格进入直接采样。
        let mut missing = mesh.clone(); // 缺失真实三角面必须保持通用覆盖诊断。
        missing.indices.truncate(missing.indices.len() - 6); // 删除末格两面。
        assert!(infer_rectilinear_grid(&missing).is_none()); // 不把洞口自动补成地面。
        let mut moved = mesh.clone(); // 横向移动顶点后不再是矩形格线。
        moved.positions[5][0] += 1.0; // 中间行单点偏移。
        assert!(infer_rectilinear_grid(&moved).is_none()); // 任意拓扑交给原有空间桶。
        let mut rewound = mesh; // 倒转局部作者绕序同样不能冒充新地图规则拓扑。
        rewound.indices.swap(0, 1); // 真实三角形连接顺序已改变。
        assert!(infer_rectilinear_grid(&rewound).is_none()); // 洞穴与非规则拓扑继续保留通用路径。
        let mut irregular = regular_mesh(4); // 行列仍完整但格线并非新地图生成器的等距值。
        for row in 0..4 {
            irregular.positions[row * 4 + 1][0] += 1.0; // 同列横向移动，保证每行 X 一致但间距改变。
        }
        assert!(infer_rectilinear_grid(&irregular).is_none()); // 编辑器不能按错误步长直接定位非均匀网格。
    }

    #[test]
    fn glb_grid_above_height_resolution_derives_against_fixed_height_sampling() {
        let mesh_spec = TerrainSampleSpec {
            resolution: [516, 516],    // GLB每轴515格，明确跨过513²高度采样的旧上限。
            world_min: [0.0, 0.0],     // 一米间隔使边界和非整数高度采样点都受检。
            world_max: [515.0, 515.0], // 规则白模完整覆盖固定采样的世界范围。
            base_height: -1.0,         // 世界Y零位于高度编码量程中点。
            height_scale: 2.0,         // 量化后的预期值为32768。
        }; // 构造高于固定高度纹理分辨率的GLB输入。
        let mesh = crate::build_flat_terrain_mesh(mesh_spec, 0.0)
            .expect("GLB细分不应受513²高度采样门限制"); // 验证新建平面允许更密顶点。
        assert_eq!(mesh.positions.len(), 516 * 516); // 实际顶点数保持GLB请求而非被截为513²。
        assert_eq!(mesh.indices.len(), 515 * 515 * 6); // 每个规则单元必须保留两个三角形。
        let sample_spec = TerrainSampleSpec {
            resolution: [513, 513], // 运行时高度缓存继续使用原本的固定分辨率。
            ..mesh_spec             // 世界范围和编码量程必须与GLB一致。
        }; // 分离GLB真实拓扑与高度采样规格。
        let derived = derive_terrain(mesh, sample_spec)
            .expect("较密GLB地形必须能派生固定高度缓存且不超出候选预算"); // 覆盖空间桶和完整高度栅格化。
        assert_eq!(derived.heights.len(), 513 * 513); // 高度缓存不随GLB网格细分扩大。
        assert!(derived.heights.iter().all(|height| *height == 32_768)); // 平面所有采样点高度一致。
        assert_eq!(derived.height_cells_rgba16.len(), 512 * 512 * 8); // GPU高度单元仍为固定512²。
        assert!(validate_sample_spec(mesh_spec).is_err()); // 派生采样门自身仍拒绝516²规格。
    }

    #[test]
    fn maximum_flat_glb_grid_stays_within_fixed_raster_budgets() {
        let mesh_spec = TerrainSampleSpec {
            resolution: [1025, 1025],    // 达到平坦GLB独立的1024²单元上限。
            world_min: [0.0, 0.0],       // 与单元编号对齐，便于验证末端覆盖。
            world_max: [1024.0, 1024.0], // 每个GLB单元一米。
            base_height: -1.0,           // 平面Y零位于高度编码量程中点。
            height_scale: 2.0,           // 量化高度预期为32768。
        }; // 构造允许的最密规则新图白模。
        let started = Instant::now(); // 记录目标硬件上的最大规格完整派生耗时。
        let mesh = crate::build_flat_terrain_mesh(mesh_spec, 0.0)
            .expect("最大1025²顶点的平坦GLB网格应可构建"); // 验证完整单元预算的顶点和索引分配。
        let sample_spec = TerrainSampleSpec {
            resolution: [513, 513], // 运行时仍只生成固定高度缓存。
            ..mesh_spec             // 采样范围和量程沿用真实地形地图规格。
        }; // 将较密GLB和较稀高度缓存分开。
        let derived = derive_terrain(mesh, sample_spec)
            .expect("最大平坦GLB网格不应触发空间桶或候选测试硬预算"); // 覆盖最坏规格的完整派生路径。
        assert_eq!(derived.positions.len(), 1025 * 1025); // 确认未降低或丢弃真实GLB网格细分。
        assert_eq!(derived.heights.len(), 513 * 513); // 确认高度缓存维持现有上限。
        assert!(derived.heights.iter().all(|height| *height == 32_768)); // 每条规则采样竖线均由平面覆盖。
        eprintln!(
            "terrain-derive max-flat-grid 1025x1025 -> 513x513: {:?}",
            started.elapsed()
        ); // 提供目标机器性能基线。
    }

    #[test]
    fn flat_arbitrary_mesh_generates_height_grid_and_rgba_cells() {
        let derived = derive_terrain(flat_quad(0.0), spec()).expect("合法平面必须成功派生"); // 执行完整后台纯算法。
        assert_eq!(derived.heights, vec![32_768; 9]); // 中点按最近u16量化为32768。
        assert_eq!(derived.height_cells_rgba16.len(), 2 * 2 * 8); // 四个单元每个八字节。
        assert!(
            derived
                .normals
                .iter()
                .all(|normal| *normal == [0.0, 1.0, 0.0])
        ); // 平面法线保持世界上方向。
    } // 结束平面派生回归。

    #[test]
    fn channels_keep_north_west_north_east_south_west_south_east_order() {
        let bytes =
            pack_height_cells_rgba16(&[1, 2, 3, 4], [2, 2]).expect("2×2高度必须形成一个单元"); // 编码最小高度单元。
        let decoded = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(); // 还原四个小端通道。
        assert_eq!(decoded, vec![1, 2, 3, 4]); // 通道顺序严格保持NW、NE、SW、SE。
    } // 结束通道顺序回归。

    #[test]
    fn missing_columns_are_rejected_and_multiple_layers_choose_the_highest_surface() {
        let mut missing = flat_quad(0.0); // 从完整平面删除右下三角形制造缺口。
        missing.indices.truncate(3); // 只保留左上半面。
        missing.positions.truncate(3); // 删除未引用顶点，让测试精确到达覆盖校验而非孤立顶点门。
        assert!(
            derive_terrain(missing, spec())
                .unwrap_err()
                .contains("整列没有任何可采样表面")
        ); // 任何采样缺口都明确失败。
        let mut layered = flat_quad(0.0); // 保留完整底层。
        layered.positions.extend(flat_quad(1.0).positions); // 追加同范围上层顶点。
        layered.indices.extend([4, 6, 5, 5, 6, 7]); // 追加同范围上层三角形。
        let derived =
            derive_terrain(layered, spec()).expect("洞穴与悬空层必须允许同一X/Z存在多个高度");
        let expected =
            (((1.0 - spec().base_height) / spec().height_scale) * u16::MAX as f32).round() as u16; // 多层规则高度按最高世界Y量化。
        assert_eq!(derived.heights, vec![expected; 9]); // 室外水域和植被读取最高包络，不会落到洞顶下方。
    } // 结束缺口与多层最高表面回归。

    #[test]
    fn sub_sample_cave_details_do_not_invalidate_the_full_mesh() {
        let coordinates = [-1.0, 0.25, 0.5, 1.0];
        let positions = coordinates
            .iter()
            .flat_map(|&z| coordinates.iter().map(move |&x| [x, 0.0, z]))
            .collect::<Vec<_>>();
        let mut indices = Vec::new();
        for z in 0..3u32 {
            for x in 0..3u32 {
                if x == 1 && z == 1 {
                    continue; // 小洞完全位于3×3规则采样点之间。
                }
                let nw = z * 4 + x;
                indices.extend_from_slice(&[nw, nw + 4, nw + 1, nw + 1, nw + 4, nw + 5]);
            }
        }
        let hole = TerrainMeshInput { positions, indices };
        derive_terrain(hole, spec())
            .expect("规则高度采样点均有地面时，采样格内开口属于合法洞穴细节"); // 完整Mesh与Trimesh继续保留小开口。

        let mut hidden_layer = flat_quad(0.0);
        hidden_layer
            .positions
            .extend([[0.20, 1.0, 0.20], [0.20, 1.0, 0.40], [0.40, 1.0, 0.20]]); // 小悬空层同样完全避开3×3采样点。
        hidden_layer.indices.extend([4, 5, 6]);
        derive_terrain(hidden_layer, spec()).expect("采样点之间的洞顶细节不得使完整三维地形失效"); // 高度缓存无需表达低于其分辨率的全部三维细节。
    }

    #[test]
    fn cancelling_face_normals_reuse_the_first_real_face() {
        let mesh = TerrainMeshInput {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]], // 同三个顶点由两张相反绕序面共享。
            indices: vec![0, 2, 1, 0, 1, 2], // 两面面积权重正好相消，必须回退到作者首面。
        };
        let normals = derive_vertex_normals(&mesh).expect("非流形相反面仍可取得作者首面法线"); // 锁定压缩首面身份后的语义。
        assert!(normals.iter().all(|normal| normal[1] > 0.99)); // 首面朝上，不因完全抵消输出零法线。
    }

    #[test]
    fn arbitrary_winding_is_allowed_but_non_finite_and_out_of_range_heights_are_rejected() {
        let mut reversed = flat_quad(0.0); // 构造向下绕序。
        reversed.indices.swap(1, 2); // 翻转首个三角形。
        let reversed = derive_terrain(reversed, spec()).expect("向下面是洞穴顶板的合法组成部分");
        assert_eq!(reversed.indices, vec![0, 1, 2, 1, 2, 3]); // 派生器只允许作者绕序，不擅自翻转三角连接。
        let mut non_finite = flat_quad(0.0); // 构造损坏位置。
        non_finite.positions[0][1] = f32::NAN; // 注入非有限高度。
        assert!(
            derive_terrain(non_finite, spec())
                .unwrap_err()
                .contains("NaN")
        ); // 非有限值在分桶前失败。
        assert!(
            derive_terrain(flat_quad(11.0), spec())
                .unwrap_err()
                .contains("超出")
        ); // 不静默截断量程外地形。

        let outside_vertex = TerrainMeshInput {
            positions: vec![
                [-2.0, 11.0, -2.0], // 位于声明X/Z外且超出高度量程，规则采样本身仍可能全部合法。
                [2.0, 0.0, -2.0],
                [-2.0, 0.0, 2.0],
                [2.0, 0.0, 2.0],
            ],
            indices: vec![0, 2, 1, 1, 2, 3],
        };
        assert!(
            derive_terrain(outside_vertex, spec())
                .unwrap_err()
                .contains("顶点0高度")
        ); // 地形的范围外顶点也必须遵守同一清单高度契约。
    } // 结束非法输入回归。

    #[test]
    fn shuffled_vertices_boundary_samples_and_nearest_quantization_are_stable() {
        let mesh = TerrainMeshInput {
            positions: vec![
                [1.0, 10.0, 1.0],  // 原SE被故意放到索引0。
                [-1.0, 0.0, -1.0], // 原NW被放到索引1。
                [1.0, 5.0, -1.0],  // 原NE被放到索引2。
                [-1.0, 5.0, 1.0],  // 原SW被放到索引3。
            ],
            indices: vec![1, 3, 2, 2, 3, 0], // 保持朝上连接，但顶点表完全乱序。
        };
        let derived = derive_terrain(
            mesh,
            TerrainSampleSpec {
                resolution: [3, 3],
                world_min: [-1.0, -1.0],
                world_max: [1.0, 1.0],
                base_height: 0.0,
                height_scale: 10.0,
            },
        )
        .expect("线性斜面不应依赖顶点排序");
        assert_eq!(derived.heights[0], 0); // NW边界精确命中量程下界。
        assert_eq!(derived.heights[2], 32_768); // NE边界5m按最近整数取中点。
        assert_eq!(derived.heights[4], 32_768); // 中心点同样验证重心插值和最近量化。
        assert_eq!(derived.heights[6], 32_768); // SW边界不因桶映射丢失。
        assert_eq!(derived.heights[8], u16::MAX); // SE右下边界精确命中量程上界。
        assert_eq!(derived.positions[0], [1.0, 10.0, 1.0]); // 派生结果不重排Blender顶点。
        assert_eq!(derived.indices, vec![1, 3, 2, 2, 3, 0]); // 三角连接关系原样保留。
    }

    #[test]
    fn vertical_cave_walls_are_kept_out_of_height_rasterization() {
        let mut mesh = flat_quad(0.0); // 完整室外平面继续提供每条规则竖线的最高表面。
        mesh.positions.extend([
            [-0.5, -1.0, -0.5],
            [-0.5, 1.0, -0.5],
            [-0.5, -1.0, 0.5],
            [-0.5, 1.0, 0.5],
        ]); // 独立竖直面模拟洞壁，XZ投影退化但三维面积有效。
        mesh.indices.extend([4, 6, 5, 5, 6, 7]); // 洞壁三角形保留作者绕序。

        let derived =
            derive_terrain(mesh, spec()).expect("竖直洞壁必须参与渲染和Trimesh但不阻断高度派生");
        assert_eq!(derived.positions.len(), 8); // 完整洞壁顶点没有从可见或碰撞网格中删除。
        assert_eq!(derived.indices.len(), 12); // 两个竖直面三角形仍保留在原索引末尾。
        assert_eq!(derived.heights, vec![32_768; 9]); // 室外高度仍由水平顶层决定。
    }

    #[test]
    fn degenerate_and_empty_meshes_are_rejected_before_rasterization() {
        let degenerate = TerrainMeshInput {
            positions: vec![[-1.0, 0.0, -1.0], [0.0, 0.0, 0.0], [1.0, 0.0, 1.0]],
            indices: vec![0, 1, 2], // X/Z共线使投影面积为零。
        };
        assert!(
            derive_terrain(degenerate, spec())
                .unwrap_err()
                .contains("退化")
        );

        let empty = TerrainMeshInput {
            positions: Vec::new(), // 空POSITION仍不是可派生地形。
            indices: Vec::new(),   // 空索引与空顶点共同验证首个输入门。
        }; // 完成空网格夹具。
        assert!(
            derive_terrain(empty, spec())
                .unwrap_err()
                .contains("至少一个顶点")
        ); // 删除固定顶点上限后仍保留非空契约。
    }

    #[test] // 同时超过旧顶点和三角形门的合法拓扑不再因固定数量预算被拒绝。
    fn mesh_above_legacy_vertex_and_triangle_limits_is_accepted_by_mesh_validation() {
        const LEGACY_VERTEX_LIMIT: usize = 263_169; // 仅用于锁定已经删除的历史门，不再参与生产代码。
        const LEGACY_TRIANGLE_LIMIT: usize = 524_288; // 仅用于证明旧三角形门也已经删除。
        const COLUMNS: usize = 262_146; // 两行共524292个顶点，并形成524290个合法三角形。
        let mut positions = Vec::with_capacity(COLUMNS * 2); // 一次分配测试条带位置表。
        for row in 0..2usize {
            let z = if row == 0 { -1.0 } else { 1.0 }; // 两行形成具有正Z跨度的平面条带。
            positions.extend((0..COLUMNS).map(|x| [x as f32, 0.0, z])); // 每个顶点都将被相邻三角形引用。
        } // 完成超过旧上限的位置表。
        let mut indices = Vec::with_capacity((COLUMNS - 1) * 6); // 每个条带单元保持两个朝上三角形。
        for x in 0..COLUMNS - 1 {
            let north_west = x as u32; // 北侧当前列。
            let north_east = north_west + 1; // 北侧下一列。
            let south_west = (COLUMNS + x) as u32; // 南侧当前列。
            let south_east = south_west + 1; // 南侧下一列。
            indices.extend_from_slice(&[
                north_west, // 首三角北西角。
                south_west, // 首三角南西角，保持法线朝上。
                north_east, // 首三角北东角。
                north_east, // 次三角北东角。
                south_west, // 次三角南西角。
                south_east, // 次三角南东角。
            ]); // 完成当前条带单元连接。
        } // 完成全部合法三角形。
        let mesh = TerrainMeshInput { positions, indices }; // 冻结超过旧上限的合法共享拓扑。

        assert!(mesh.positions.len() > LEGACY_VERTEX_LIMIT); // 夹具必须确实覆盖被删除的限制。
        assert!(mesh.indices.len() / 3 > LEGACY_TRIANGLE_LIMIT); // 夹具必须同时越过已删除的旧三角形限制。
        validate_mesh(&mesh).expect("超过旧顶点和三角形数量上限不应再失败"); // 只验证轻量网格门，避免测试执行整幅连续投影。
    }

    #[test]
    fn generation_cost_is_recorded_for_supported_resolutions() {
        for resolution in [17, 129, 513] {
            let mesh = regular_mesh(resolution);
            let input_bytes = mesh.positions.len() * std::mem::size_of::<[f32; 3]>()
                + mesh.indices.len() * std::mem::size_of::<u32>();
            let started = Instant::now();
            let derived = derive_terrain(
                mesh,
                TerrainSampleSpec {
                    resolution: [resolution, resolution],
                    world_min: [-512.0, -512.0],
                    world_max: [512.0, 512.0],
                    base_height: -1.0,
                    height_scale: 2.0,
                },
            )
            .unwrap_or_else(|error| panic!("{resolution}²性能规格派生失败：{error}"));
            let output_bytes = derived.positions.len() * std::mem::size_of::<[f32; 3]>()
                + derived.normals.len() * std::mem::size_of::<[f32; 3]>()
                + derived.indices.len() * std::mem::size_of::<u32>()
                + derived.heights.len() * std::mem::size_of::<u16>()
                + derived.height_cells_rgba16.len();
            eprintln!(
                "terrain-derive {resolution}x{resolution}: {:?}, input={} KiB, output={} KiB",
                started.elapsed(),
                input_bytes / 1024,
                output_bytes / 1024
            ); // `--nocapture`时形成目标机器的可复现基线记录。
        }
    }
}
