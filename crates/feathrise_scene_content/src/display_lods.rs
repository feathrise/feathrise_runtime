//! Geometry Clipmap显示LOD的作者默认值与不依赖渲染器的纯配置校验。

use crate::{SceneTerrainDisplayLod, SparseHeightSpec}; // 编辑器和场景运行时共享同一作者类型与稀疏规格。

/// 旧Geometry Clipmap中心方格和每级外环使用的固定半边格数。
pub const LEGACY_DISPLAY_LOD_HALF_CELLS: u32 = 64; // 显式默认列表必须复现当前自动算法的画质起点。
/// 顶点Shader使用f32指数且旧实现最多十六级，因此作者倍率最高保留十五次翻倍。
pub const MAX_DISPLAY_LOD_MULTIPLIER: u32 = 1 << 15; // 最大合法倍率为32768倍作者格距。
/// 一份组合显示Mesh的位置、法线、两组UV和u32索引总载荷硬上限。
pub const MAX_DISPLAY_LOD_MESH_BYTES: u64 = 16 * 1024 * 1024; // 与运行时大资源上传事务使用同一静态预算。
/// 完整候选高度数组与紧凑RGBA32_UINT页表的单次上传硬上限。
pub const MAX_DISPLAY_LOD_PAGE_BYTES: u64 = 8 * 1024 * 1024; // 保存预检按范围内页全部非空计算。
/// 每个采样Mip在可见范围外额外驻留的格数，保持旧128格页窗口完全等价。
pub const DISPLAY_LOD_RESIDENCY_GUARD_CELLS: u64 = 64; // 只服务GPU显示，不进入碰撞规划。

/// 编译后一个只与相邻二倍倍率连接的世界固定方形环带。
#[derive(Clone, Debug, PartialEq)] // 场景任务身份按完整计划比较，迟到结果不能只比较作者列表长度。
pub struct CompiledDisplayLodBand {
    pub spacing_multiplier: u32,         // 相对作者基础格距的当前几何倍率。
    pub inner_half_cells: [u32; 2],      // 当前倍率格中的X/Z内边界；首层固定为零。
    pub outer_half_cells: [u32; 2],      // 当前倍率格中的X/Z外边界；两轴分别量化。
    pub source_lod_index: Option<usize>, // None表示为倍率跳级自动插入的内部过渡层。
} // 运行时仅消费该结构，不反写作者JSON。

/// 一个高度采样Mip需要覆盖的最大可见半径，不包含固定驻留保护带。
#[derive(Clone, Copy, Debug, PartialEq, Eq)] // 页身份可直接携带并确定排序。
pub struct CompiledDisplaySampleRange {
    pub mip_level: u8,        // 已钳制到当前稀疏格式持久Mip上限。
    pub half_cells: [u64; 2], // 当前采样Mip格中的可见X/Z半径，已包含法线相邻点。
} // 分页器随后再增加64格驻留保护带。

/// 作者显示列表经合并、量化和2:1补带后的不可变构建计划。
#[derive(Clone, Debug, PartialEq)] // 后台Mesh、分页和GPU候选共享同一值身份。
pub struct CompiledDisplayLodPlan {
    pub authored_lods: Vec<SceneTerrainDisplayLod>, // 保存配置快照，诊断区分已保存与已采用。
    pub bands: Vec<CompiledDisplayLodBand>,       // 从内向外的实际有效环带，倍率严格逐级翻倍。
    pub sample_ranges: Vec<CompiledDisplaySampleRange>, // 仅包含真实被几何或接缝采样的持久Mip。
    pub actual_half_extents_meters: Vec<[f32; 2]>, // 每个有效环带外边界的实际世界X/Z半径。
    pub vertex_count: u64,                        // 预算公式计算的真实组合顶点数。
    pub index_count: u64,                         // 预算公式计算的真实u32索引数。
    pub mesh_bytes: u64,                          // position、normal、UV0、UV1和索引的总上传字节。
} // 计划不拥有Mesh、Image或ECS实体。

/// 为新SparseClipmap按当前旧算法生成显式作者列表，末层固定覆盖场景边界。
pub fn default_display_lods_for_spec(
    spec: &SparseHeightSpec, // 已验证的世界范围与作者实际格距。
) -> Result<Vec<SceneTerrainDisplayLod>, String> {
    // 失败只可能来自无法由十六级覆盖的异常规格。
    let spacing = [
        spec.actual_spacing_meters[0] as f32, // X轴作者实际格距收窄到显示配置使用的f32。
        spec.actual_spacing_meters[1] as f32, // Z轴作者实际格距独立保留，避免假设方形格。
    ]; // 完成两轴显示格距快照。
    let extent = [
        spec.world_max[0] - spec.world_min[0], // 从场景任意边缘焦点覆盖对侧需要完整X长度。
        spec.world_max[1] - spec.world_min[1], // Z轴同样使用完整长度而不是半长。
    ]; // 完成最坏焦点覆盖范围。
    let mut multiplier = 1_u32; // 默认从作者最高精度开始。
    let mut lods = Vec::new(); // 层数最多十六，属于固定小向量。
    loop {
        let radius = [
            spacing[0] * LEGACY_DISPLAY_LOD_HALF_CELLS as f32 * multiplier as f32, // 当前倍率的X旧环半径。
            spacing[1] * LEGACY_DISPLAY_LOD_HALF_CELLS as f32 * multiplier as f32, // 当前倍率的Z旧环半径。
        ]; // 完成当前旧层真实两轴范围。
        let covers_scene = radius[0] >= extent[0] && radius[1] >= extent[1]; // 两轴同时覆盖才允许成为最终层。
        lods.push(SceneTerrainDisplayLod {
            outer_half_extent_meters: (!covers_scene).then_some(radius[0].max(radius[1])), // 有限层用不降低任一轴画质的标量方形半径。
            spacing_multiplier: multiplier, // 作者倍率保持相对基础格距，不逐层累乘。
        }); // 保存当前显式默认层。
        if covers_scene {
            break; // 最后一项已经以None表达到场景边界。
        } // 继续加入下一粗层。
        multiplier = multiplier
            .checked_mul(2)
            .filter(|value| *value <= MAX_DISPLAY_LOD_MULTIPLIER)
            .ok_or("显示LOD十六级上限不足以覆盖场景")?; // 不绕回或静默降低作者范围。
    } // 完成旧自动算法到显式列表的确定转换。
    validate_display_lod_shape(&lods)?; // 默认生成器也必须服从公开结构合同。
    Ok(lods) // 调用方决定保存为显式列表还是仅作为旧图回显。
} // 不构造Mesh或访问ECS资源。

/// 校验显示LOD作者列表中不依赖场景尺寸和实际网格预算的结构约束。
pub fn validate_display_lod_shape(
    lods: &[SceneTerrainDisplayLod], // 清单或F12草稿中的完整作者顺序。
) -> Result<(), String> {
    // 返回可直接展示给作者的中文错误。
    if lods.is_empty() {
        return Err("自定义显示LOD至少需要一层".into()); // 显式空列表不能冒充旧自动模式。
    } // 完成非空检查。
    let mut previous_extent = 0.0_f32; // 第一项有限半径也必须严格为正。
    let mut previous_multiplier = 1_u32; // 向外不允许恢复更细网格。
    for (index, lod) in lods.iter().enumerate() {
        let is_last = index + 1 == lods.len(); // 只有最后一项可以没有有限范围。
        match lod.outer_half_extent_meters {
            Some(_extent) if is_last => {
                return Err("显示LOD最后一层必须延伸至场景边界".into()); // 避免场景外圈没有任何显示层。
            } // 末层有限值属于协议错误。
            Some(extent) => {
                if !extent.is_finite() || extent <= previous_extent {
                    return Err(format!(
                        "显示LOD {index} 的方形半径必须有限、为正且逐层递增"
                    )); // NaN、无穷、零、负数和重复距离统一拒绝。
                } // 完成当前有限边界验证。
                previous_extent = extent; // 下一作者层必须严格越过当前范围。
            } // 非末层保存有限累计半径。
            None if !is_last => {
                return Err(format!("显示LOD {index} 缺少有限方形半径")); // 中途全图层会吞掉后续作者条目。
            } // 非末层None属于协议错误。
            None => {} // 唯一末层使用None覆盖场景边界。
        } // 完成范围语义检查。
        if !lod.spacing_multiplier.is_power_of_two()
            || lod.spacing_multiplier < previous_multiplier
            || lod.spacing_multiplier > MAX_DISPLAY_LOD_MULTIPLIER
        {
            return Err(format!(
                "显示LOD {index} 的倍率必须为1、2、4、8等，向外不减且不超过{MAX_DISPLAY_LOD_MULTIPLIER}倍"
            )); // 不排序、不钳制也不替作者改变精度。
        } // 完成倍率约束检查。
        previous_multiplier = lod.spacing_multiplier; // 相同倍率允许由运行时合并为一个有效环带。
    } // 遍历全部作者条目，不设置五层等产品数量上限。
    Ok(()) // 场景相关量化、页数与Mesh预算由显示规划器继续校验。
} // 纯函数可被反序列化、编辑器和运行时共同调用。

/// 把米制作者边界向外量化到指定倍率的基础格整数倍。
fn quantized_base_cells(
    extent_meters: f32,  // 作者累计方形半径。
    spacing_meters: f64, // 当前轴真实作者格距。
    multiplier: u32,     // 边界外侧相邻较粗层倍率。
) -> Result<u64, String> {
    // 所有整数转换都显式检查，避免极端输入饱和成另一含义。
    let coarse_spacing = spacing_meters * f64::from(multiplier); // 当前量化粗格的世界宽度。
    let coarse_cells = (f64::from(extent_meters) / coarse_spacing).ceil(); // 只向外取整，不缩小作者范围。
    if !coarse_cells.is_finite() || coarse_cells <= 0.0 || coarse_cells > u64::MAX as f64 {
        return Err("显示LOD边界量化后超出可表达范围".into()); // 不进入后续乘法或分配。
    } // 完成有限整数范围检查。
    (coarse_cells as u64)
        .checked_mul(u64::from(multiplier))
        .ok_or_else(|| "显示LOD边界基础格数量溢出".to_owned()) // 返回可被所有轴复用的基础格坐标。
} // 量化误差严格小于一个对应粗格。

/// 根据矩形环带闭式公式累计真实顶点与索引数量。
fn band_geometry_counts(
    inner: [u32; 2], // 当前倍率格中的内矩形半径。
    outer: [u32; 2], // 当前倍率格中的外矩形半径。
) -> Result<(u128, u128), String> {
    // u128确保任何作者极值都先失败而不是绕回。
    let [inner_x, inner_z] = inner.map(u128::from); // 提升后再执行面积乘法。
    let [outer_x, outer_z] = outer.map(u128::from); // 两轴允许因实际格距不同而不相等。
    if outer_x == 0
        || outer_z == 0
        || inner_x > outer_x
        || inner_z > outer_z
        || (inner_x == outer_x && inner_z == outer_z)
    {
        return Err("显示LOD量化后产生空环或退化轴".into()); // 矩形场景允许短轴已到边界后仅沿另一轴继续扩展。
    } // 完成拓扑前置条件检查。
    let outer_cells = 4_u128
        .checked_mul(outer_x)
        .and_then(|value| value.checked_mul(outer_z))
        .ok_or("显示LOD外环单元数量溢出")?; // [-x,x]乘[-z,z]共有4xz个单元。
    if inner_x == 0 && inner_z == 0 {
        let vertices = (2 * outer_x + 1)
            .checked_mul(2 * outer_z + 1)
            .ok_or("显示LOD中心顶点数量溢出")?; // 首层完整矩形共享整数顶点。
        let indices = outer_cells
            .checked_mul(6)
            .ok_or("显示LOD中心索引数量溢出")?; // 每格固定两个三角形。
        return Ok((vertices, indices)); // 中心层没有接缝额外点。
    } // 后续层按空心矩形计算。
    if inner_x == 0 || inner_z == 0 {
        return Err("显示LOD内环不得仅单轴退化".into()); // 不支持线状洞口的歧义接缝。
    } // 两轴内半径必须同步非零。
    let inner_cells = 4_u128
        .checked_mul(inner_x)
        .and_then(|value| value.checked_mul(inner_z))
        .ok_or("显示LOD内环单元数量溢出")?; // 洞口完整单元面积。
    let ring_cells = outer_cells
        .checked_sub(inner_cells)
        .ok_or("显示LOD环带单元数量下溢")?; // 前置条件保证正常大于零。
    let outer_grid_vertices = (2 * outer_x + 1)
        .checked_mul(2 * outer_z + 1)
        .ok_or("显示LOD外环顶点数量溢出")?; // 完整外矩形格点数。
    let removed_inner_vertices = (2 * inner_x - 1)
        .checked_mul(2 * inner_z - 1)
        .ok_or("显示LOD洞内顶点数量溢出")?; // 严格位于洞内的整数点不生成。
    let unused_vertical_vertices = if outer_x == inner_x {
        2_u128
            .checked_mul(2 * inner_z - 1)
            .ok_or("显示LOD侧向环竖边顶点数量溢出")?
    } else {
        0
    }; // 没有左右外侧格时，两条竖直内边不属于本层。
    let unused_horizontal_vertices = if outer_z == inner_z {
        2_u128
            .checked_mul(2 * inner_x - 1)
            .ok_or("显示LOD侧向环横边顶点数量溢出")?
    } else {
        0
    }; // 没有上下外侧格时，两条水平内边不属于本层。
    let unused_boundary_vertices = unused_vertical_vertices
        .checked_add(unused_horizontal_vertices)
        .ok_or("显示LOD侧向环边界顶点数量溢出")?; // 两轴都相等已在前置条件作为真正空环拒绝。
    let transition_half_length = (outer_x > inner_x)
        .then_some(inner_z)
        .unwrap_or(0)
        .checked_add((outer_z > inner_z).then_some(inner_x).unwrap_or(0))
        .ok_or("显示LOD接缝单元数量溢出")?; // 只有外侧真实存在粗格的内边才需要过渡。
    let transition_cells = 4_u128
        .checked_mul(transition_half_length)
        .ok_or("显示LOD接缝单元数量溢出")?; // 两侧各为两倍半边长；单轴侧向环不伪造场景外接缝。
    let vertices = outer_grid_vertices
        .checked_sub(removed_inner_vertices)
        .and_then(|value| value.checked_sub(unused_boundary_vertices))
        .and_then(|value| value.checked_add(transition_cells * 2))
        .ok_or("显示LOD环带顶点数量溢出")?; // 每个过渡格增加边中点和单元中心。
    let indices = ring_cells
        .checked_mul(6)
        .and_then(|value| value.checked_add(transition_cells * 9))
        .ok_or("显示LOD环带索引数量溢出")?; // 五三角过渡格比普通格多九个索引。
    Ok((vertices, indices)) // 与运行时实际生成器逐项一致。
} // 不进行任何堆分配。

/// 将作者列表编译为Mesh与分页共同消费的有效环带计划。
pub fn compile_display_lod_plan(
    spec: &SparseHeightSpec, // 已验证的场景边界、基础格距和持久Mip上限。
    authored_lods: &[SceneTerrainDisplayLod], // 显式列表或旧自动模式的等价临时列表。
) -> Result<CompiledDisplayLodPlan, String> {
    compile_display_lod_plan_with_coverage(spec, authored_lods, spec.cells)
}

/// 运行时允许观察中心在网格外；覆盖半径只改变显示计划，不伪造作者规格或持久高度Mip。
pub fn compile_display_lod_plan_with_coverage(
    spec: &SparseHeightSpec,
    authored_lods: &[SceneTerrainDisplayLod],
    coverage_base_cells: [u64; 2],
) -> Result<CompiledDisplayLodPlan, String> {
    // 保存预检和运行时构建必须调用同一入口。
    validate_display_lod_shape(authored_lods)?; // 先拒绝空列表、坏末层和非法倍率。
    let mut merged = Vec::<(usize, SceneTerrainDisplayLod)>::new(); // 同倍率作者行只保留最外边界。
    for (index, lod) in authored_lods.iter().cloned().enumerate() {
        if let Some((source_index, previous)) = merged.last_mut()
            && previous.spacing_multiplier == lod.spacing_multiplier
        {
            *source_index = index; // 诊断把合并层归到最后一个实际决定边界的作者行。
            *previous = lod; // 相同倍率中间边界不产生重复几何环带。
        } else {
            merged.push((index, lod)); // 新倍率开始一个真实作者环带。
        } // 完成当前作者行合并。
    } // 最终None会随同倍率末行正确接管场景边界。
    let spacing = spec.actual_spacing_meters; // 两轴精确f64用于量化整数基础格。
    if spacing
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return Err("显示LOD使用的作者实际格距无效".into()); // 防御绕过SparseHeightSpec构造的损坏值。
    } // 完成基础格距检查。
    let mut bands = Vec::new(); // 最多作者层加十五个内部过渡层。
    let mut outer_base_cells = [0_u64; 2]; // 上一有效层世界边界，以基础格整数表达。
    for (merged_index, (source_index, lod)) in merged.iter().enumerate() {
        let multiplier = lod.spacing_multiplier; // 当前真实作者倍率。
        if outer_base_cells
            .iter()
            .any(|value| value % u64::from(multiplier) != 0)
        {
            return Err("显示LOD内部边界未对齐当前倍率格".into()); // 编译器自身不变量失效时禁止继续。
        } // 当前内边界可无损换算为本层格数。
        let inner = [
            u32::try_from(outer_base_cells[0] / u64::from(multiplier))
                .map_err(|_| "显示LOD X内边界格数超过u32")?,
            u32::try_from(outer_base_cells[1] / u64::from(multiplier))
                .map_err(|_| "显示LOD Z内边界格数超过u32")?,
        ]; // 完成本层内矩形。
        let next_multiplier = merged
            .get(merged_index + 1)
            .map(|(_, next)| next.spacing_multiplier); // None代表当前作者行是全图末层。
        let quantize_multiplier = next_multiplier
            .map(|next| multiplier.saturating_mul(2).min(next))
            .unwrap_or(multiplier); // 跳级先连接当前倍率的下一相邻二倍层。
        let next_outer_base = if let Some(extent) = lod.outer_half_extent_meters {
            [
                quantized_base_cells(extent, spacing[0], quantize_multiplier)?,
                quantized_base_cells(extent, spacing[1], quantize_multiplier)?,
            ] // 作者标量距离分别按X/Z真实粗格向外量化。
        } else {
            let scene_outer = [
                coverage_base_cells[0].div_ceil(u64::from(multiplier)).checked_mul(u64::from(multiplier))
                    .ok_or("显示LOD X覆盖范围溢出")?,
                coverage_base_cells[1].div_ceil(u64::from(multiplier)).checked_mul(u64::from(multiplier))
                    .ok_or("显示LOD Z覆盖范围溢出")?,
            ]; // 从任意合法焦点覆盖场景所需的两轴完整长度。
            [
                scene_outer[0].max(outer_base_cells[0]),
                scene_outer[1].max(outer_base_cells[1]),
            ] // 已被内层方形覆盖的短轴不能向内收缩；另一轴仍精确扩至完整场景长度。
        }; // 完成本作者层实际外边界。
        let outer = [
            u32::try_from(next_outer_base[0] / u64::from(multiplier))
                .map_err(|_| "显示LOD X外边界格数超过u32")?,
            u32::try_from(next_outer_base[1] / u64::from(multiplier))
                .map_err(|_| "显示LOD Z外边界格数超过u32")?,
        ]; // 量化倍数保证除法无余数。
        band_geometry_counts(inner, outer)?; // 量化后空环在加入计划前立即拒绝。
        bands.push(CompiledDisplayLodBand {
            spacing_multiplier: multiplier,        // 当前作者倍率原样保留。
            inner_half_cells: inner,               // 当前有效内边界。
            outer_half_cells: outer,               // 当前有效外边界。
            source_lod_index: Some(*source_index), // 指回决定此层的作者行。
        }); // 加入一个实际作者环带。
        outer_base_cells = next_outer_base; // 后续相邻层从同一世界边界开始。
        if let Some(next_multiplier) = next_multiplier {
            let mut inserted_multiplier =
                multiplier.checked_mul(2).ok_or("显示LOD内部过渡倍率溢出")?; // 当前作者层外只允许逐次二倍连接。
            while inserted_multiplier < next_multiplier {
                let inserted_inner = [
                    u32::try_from(outer_base_cells[0] / u64::from(inserted_multiplier))
                        .map_err(|_| "显示LOD内部过渡X格数超过u32")?,
                    u32::try_from(outer_base_cells[1] / u64::from(inserted_multiplier))
                        .map_err(|_| "显示LOD内部过渡Z格数超过u32")?,
                ]; // 前一边界已按当前内部倍率对齐。
                let following_multiplier = inserted_multiplier
                    .checked_mul(2)
                    .ok_or("显示LOD内部过渡倍率溢出")?; // 下一环继续维持2:1。
                let inserted_outer_base = [
                    (outer_base_cells[0] + u64::from(inserted_multiplier))
                        .div_ceil(u64::from(following_multiplier))
                        * u64::from(following_multiplier),
                    (outer_base_cells[1] + u64::from(inserted_multiplier))
                        .div_ceil(u64::from(following_multiplier))
                        * u64::from(following_multiplier),
                ]; // 至少留一当前粗格，并向外对齐下一层边界。
                let inserted_outer = [
                    u32::try_from(inserted_outer_base[0] / u64::from(inserted_multiplier))
                        .map_err(|_| "显示LOD内部过渡X外边界超过u32")?,
                    u32::try_from(inserted_outer_base[1] / u64::from(inserted_multiplier))
                        .map_err(|_| "显示LOD内部过渡Z外边界超过u32")?,
                ]; // 当前过渡环通常一至两格宽，接缝三角仍只占粗侧首格。
                band_geometry_counts(inserted_inner, inserted_outer)?; // 防御任何整数边界退化。
                bands.push(CompiledDisplayLodBand {
                    spacing_multiplier: inserted_multiplier, // 自动补齐的相邻倍率。
                    inner_half_cells: inserted_inner,        // 与前一层外边界完全同世界坐标。
                    outer_half_cells: inserted_outer,        // 与后一层格点重新对齐。
                    source_lod_index: None,                  // 不修改或伪造作者列表行。
                }); // 完成一个跳级内部过渡环。
                outer_base_cells = inserted_outer_base; // 下一倍率从新的外边界继续。
                inserted_multiplier = following_multiplier; // 严格逐级翻倍直到作者目标倍率。
            } // 完成当前作者边界后的全部内部层。
        } // 相邻倍率无需插入额外环。
    } // 完成全部合并作者层。
    finalize_display_lod_plan(spec, authored_lods, bands)
} // 作者米制半径只在此首次编译时量化。

/// 运行时剔除不可能触及来源的细层，保留已受检的逐轴边界和原作者行身份。
/// 【功能设计】矩形源两轴实际格距可以不同，禁止把已量化X/Z半径取max后再次标量量化。
/// 这里只改变首层内洞和末层覆盖；内部2:1过渡层仍是原编译结果，不冒充新的作者档。
pub fn compile_display_lod_plan_suffix(
    spec: &SparseHeightSpec,
    authored: &CompiledDisplayLodPlan,
    minimum_multiplier: u32,
    coverage_base_cells: [u64; 2],
) -> Result<CompiledDisplayLodPlan, String> {
    let first = authored.bands.iter()
        .position(|band| band.spacing_multiplier == minimum_multiplier)
        .ok_or("观察LOD倍率不属于当前配置，保留上一完整显示")?;
    let mut bands = authored.bands[first..].to_vec(); // 最多十六个有效倍率，不读取高度或生成Mesh。
    bands[0].inner_half_cells = [0, 0]; // 被剔除前缀不再占用内洞，由当前首档完整覆盖。
    let last = bands.last_mut().expect("已从存在的起始倍率取得非空后缀");
    let multiplier = u64::from(last.spacing_multiplier);
    if multiplier == 0 { return Err("显示LOD末档倍率无效".into()); }
    for axis in 0..2 {
        let outer_base = coverage_base_cells[axis].div_ceil(multiplier)
            .checked_mul(multiplier).ok_or("显示LOD覆盖范围溢出")?;
        last.outer_half_cells[axis] = u32::try_from(outer_base / multiplier)
            .map_err(|_| "显示LOD覆盖格数超过u32")?.max(last.inner_half_cells[axis]);
    } // 只量化来源覆盖包络，有限层的原逐轴outer一项不改。
    finalize_display_lod_plan(spec, &authored.authored_lods, bands)
}

/// 首次作者编译和运行时后缀共用统计与预算门，不为显示派生维护第二套计数口径。
fn finalize_display_lod_plan(
    spec: &SparseHeightSpec,
    authored_lods: &[SceneTerrainDisplayLod],
    bands: Vec<CompiledDisplayLodBand>,
) -> Result<CompiledDisplayLodPlan, String> {
    let spacing = spec.actual_spacing_meters;
    if spacing.iter().any(|value| !value.is_finite() || *value <= 0.0) {
        return Err("显示LOD使用的作者实际格距无效".into());
    }
    if bands.is_empty() {
        return Err("显示LOD编译后没有有效环带".into()); // 理论上已由非空校验阻止，保留防御诊断。
    } // 后续统计可以安全读取首末层。
    let mut vertex_count = 0_u128; // 所有计数先在u128中求和。
    let mut index_count = 0_u128; // 索引与顶点分别诊断。
    let mut actual_half_extents_meters = Vec::with_capacity(bands.len()); // 固定小列表，不按场景格点分配。
    let mut sample_ranges = std::collections::BTreeMap::<u8, [u64; 2]>::new(); // Mip排序稳定。
    for (index, band) in bands.iter().enumerate() {
        if !band.spacing_multiplier.is_power_of_two()
            || band.spacing_multiplier > MAX_DISPLAY_LOD_MULTIPLIER
            || band.source_lod_index.is_some_and(|source| source >= authored_lods.len())
            || (index == 0 && band.inner_half_cells != [0, 0]) {
            return Err("显示LOD已编译环带身份无效".into());
        }
        if index > 0 {
            let previous = &bands[index - 1];
            if previous.spacing_multiplier.checked_mul(2) != Some(band.spacing_multiplier)
                || (0..2).any(|axis| u64::from(previous.outer_half_cells[axis]) * u64::from(previous.spacing_multiplier)
                    != u64::from(band.inner_half_cells[axis]) * u64::from(band.spacing_multiplier)) {
                return Err("显示LOD已编译环带不满足2:1共同边界".into());
            }
        } // 只检查固定小环带；公开后缀接口不能让损坏计划越过几何/页预算合同。
        let (vertices, indices) =
            band_geometry_counts(band.inner_half_cells, band.outer_half_cells)?;
        vertex_count = vertex_count
            .checked_add(vertices)
            .ok_or("显示LOD组合顶点数量溢出")?; // 累计真实顶点数。
        index_count = index_count
            .checked_add(indices)
            .ok_or("显示LOD组合索引数量溢出")?; // 累计真实索引数。
        let multiplier = u64::from(band.spacing_multiplier); // 当前几何层到基础格的整数比例。
        let outer_base = [
            u64::from(band.outer_half_cells[0]) * multiplier,
            u64::from(band.outer_half_cells[1]) * multiplier,
        ]; // 当前外边界基础格半径。
        actual_half_extents_meters.push([
            (outer_base[0] as f64 * spacing[0]) as f32,
            (outer_base[1] as f64 * spacing[1]) as f32,
        ]); // 公开诊断显示量化后的两轴真实米数。
        let geometry_level = band.spacing_multiplier.trailing_zeros() as u8; // 二次幂倍率的显式几何Mip。
        let sample_level = geometry_level.min(spec.max_mip_level()); // Shader与分页共同钳制到持久高度层级。
        let sample_multiplier = 1_u64 << sample_level; // 当前持久Mip相对基础格的倍率。
        let range = sample_ranges.entry(sample_level).or_insert([0, 0]); // 同一钳制Mip可能服务多个远层。
        for axis in 0..2 {
            range[axis] = range[axis].max(outer_base[axis].div_ceil(sample_multiplier)); // 保护带内另含法线邻点，默认64+64仍等价旧128格窗口。
        } // 完成当前几何层的高度采样外包络。
        if band.inner_half_cells != [0, 0] {
            let seam_level = geometry_level.saturating_sub(1).min(spec.max_mip_level()); // 粗侧边中点使用上一细层。
            let seam_multiplier = 1_u64 << seam_level; // 接缝细Mip基础格倍率。
            let seam_inner_base = [
                u64::from(band.inner_half_cells[0]) * multiplier,
                u64::from(band.inner_half_cells[1]) * multiplier,
            ]; // 当前粗层内边界的世界基础格半径。
            let seam_range = sample_ranges.entry(seam_level).or_insert([0, 0]); // 合并其他同Mip几何范围。
            for axis in 0..2 {
                seam_range[axis] =
                    seam_range[axis].max(seam_inner_base[axis].div_ceil(seam_multiplier)); // 接缝细边范围与同Mip几何包络合并。
            } // 完成细边高度依赖包络。
        } // 首层没有上一细Mip接缝。
    } // 完成几何计数、诊断和分页范围派生。
    let mesh_bytes = vertex_count
        .checked_mul(40)
        .and_then(|bytes| {
            index_count
                .checked_mul(4)
                .and_then(|indices| bytes.checked_add(indices))
        })
        .ok_or("显示LOD组合Mesh字节数溢出")?; // position12+normal12+UV0/UV1各8以及u32索引。
    if mesh_bytes > u128::from(MAX_DISPLAY_LOD_MESH_BYTES) {
        return Err(format!(
            "显示LOD组合Mesh预计{mesh_bytes}字节，超过{}字节预算",
            MAX_DISPLAY_LOD_MESH_BYTES
        )); // 不在运行时偷偷减少层数或精度。
    } // 完成静态网格预算门。
    let vertex_count = u64::try_from(vertex_count).map_err(|_| "显示LOD顶点计数超过u64")?;
    let index_count = u64::try_from(index_count).map_err(|_| "显示LOD索引计数超过u64")?;
    let mesh_bytes = u64::try_from(mesh_bytes).map_err(|_| "显示LOD网格字节数超过u64")?;
    let plan = CompiledDisplayLodPlan {
        authored_lods: authored_lods.to_vec(), // 复制固定小作者列表作为完整任务身份。
        bands,                                 // 发布只含相邻2:1连接的有效环带。
        sample_ranges: sample_ranges
            .into_iter()
            .map(|(mip_level, half_cells)| CompiledDisplaySampleRange {
                mip_level,
                half_cells,
            })
            .collect(), // 保持Mip升序，页身份确定且可比较。
        actual_half_extents_meters,            // 公开实际量化边界。
        vertex_count,                          // 公开诊断与运行时断言共用。
        index_count,                           // 三角形数量为该值除三。
        mesh_bytes,                            // GPU登记预算使用该精确载荷。
    }; // 完成不可变计划值。
    let page_bytes = worst_case_display_page_upload_bytes(spec, &plan)?; // 保存前即按全部非空页执行同一硬预算。
    if page_bytes > MAX_DISPLAY_LOD_PAGE_BYTES {
        return Err(format!(
            "显示LOD最坏高度页预计{page_bytes}字节，超过{}字节预算",
            MAX_DISPLAY_LOD_PAGE_BYTES
        )); // 不在运行时按实际平地碰巧通过超预算配置。
    } // 完成页上传预算门。
    Ok(plan) // 完整结果不修改作者输入。
} // 编译器不访问ECS、文件或GPU。

/// 估算任意焦点格相位下全部可见页均非空时的高度数组与紧凑页表字节。
pub fn worst_case_display_page_upload_bytes(
    spec: &SparseHeightSpec,       // 当前场景真实页层级与矩形页数。
    plan: &CompiledDisplayLodPlan, // 已编译的实际采样Mip范围。
) -> Result<u64, String> {
    // 编辑器诊断和运行时断言复用同一公式。
    const TILE_CELLS: u64 = 256; // 与稀疏作者页固定单元数一致。
    const PAGE_POINTS: u64 = TILE_CELLS + 1; // 右、上共享边使每页包含257²格点。
    let mut resident_pages = 0_u128; // 所有采样Mip最坏选页数。
    for range in &plan.sample_ranges {
        let counts = spec
            .mip_tile_counts(range.mip_level)
            .ok_or("显示LOD分页范围包含无效持久Mip")?; // 计划与规格不允许拆版。
        let mut axis_pages = [0_u128; 2]; // X/Z独立考虑矩形场景与焦点格相位。
        for axis in 0..2 {
            let radius = range.half_cells[axis]
                .checked_add(DISPLAY_LOD_RESIDENCY_GUARD_CELLS)
                .ok_or("显示LOD驻留保护带范围溢出")?; // 可见范围外固定增加64采样格。
            let point_span = radius
                .checked_mul(2)
                .and_then(|value| value.checked_add(1))
                .ok_or("显示LOD页窗口格点跨度溢出")?; // 两端均可能被顶点或法线读取。
            let phase_worst_pages = point_span
                .checked_add(TILE_CELLS - 1)
                .ok_or("显示LOD页窗口相位预算溢出")?
                .div_ceil(TILE_CELLS); // 任意起点相位最多额外跨过一个页边界。
            axis_pages[axis] = u128::from(u64::from(counts[axis]).min(phase_worst_pages)); // 有限场景页数是自然上限。
        } // 完成当前Mip两轴选页预算。
        resident_pages = resident_pages
            .checked_add(axis_pages[0] * axis_pages[1])
            .ok_or("显示LOD驻留页总数溢出")?; // 每个矩形窗口按笛卡尔积上传。
    } // 完成全部真实采样Mip。
    let resident_layers = resident_pages.max(2); // GPU array绑定至少保留两层合法回退。
    let height_bytes = resident_layers
        .checked_mul(u128::from(PAGE_POINTS * PAGE_POINTS * 2))
        .ok_or("显示LOD高度数组字节数溢出")?; // 每页R16_UNORM固定257²。
    let table_width = u128::from(spec.tile_counts()[0]); // 页表宽度使用基础层真实X页数。
    let mut table_rows = 0_u128; // 各持久Mip真实Z页数紧凑纵向堆叠。
    for level in 0..=spec.max_mip_level() {
        table_rows = table_rows
            .checked_add(u128::from(
                spec.mip_tile_counts(level)
                    .ok_or("显示LOD页表包含无效持久Mip")?[1],
            ))
            .ok_or("显示LOD页表行数溢出")?; // 矩形场景不按X宽复制空行。
    } // 完成紧凑页表高度。
    let page_table_bytes = table_width
        .checked_mul(table_rows)
        .and_then(|texels| texels.checked_mul(16))
        .ok_or("显示LOD页表字节数溢出")?; // RGBA32_UINT每texel十六字节。
    let total = height_bytes
        .checked_add(page_table_bytes)
        .ok_or("显示LOD页上传总字节数溢出")?; // 完整候选两张Image同属一个事务。
    u64::try_from(total).map_err(|_| "显示LOD页上传总字节数超过u64".to_owned())
} // 不读取清单实际非空页，严格采用最坏情况。

#[cfg(test)] // 作者协议回归不依赖Bevy App或GPU。
mod tests {
    use super::*; // 直接验证公开默认与结构校验。

    #[test] // 默认列表必须延续旧64半格翻倍行为并以场景边界收尾。
    fn explicit_default_matches_legacy_ranges() {
        let spec = SparseHeightSpec::new(
            [0.0, 0.0],       // 场景最小角。
            [1024.0, 1024.0], // 从边缘焦点需要1024米覆盖。
            0.125,            // 旧最细层半径为8米。
            -80.0,            // 高度编码基线与显示LOD无关。
            160.0,            // 高度量程与显示LOD无关。
        )
        .unwrap(); // 测试规格合法。
        let lods = default_display_lods_for_spec(&spec).unwrap(); // 生成新图显式默认。
        assert_eq!(lods.first().unwrap().outer_half_extent_meters, Some(8.0)); // 最细层保留旧8米范围。
        assert_eq!(lods.last().unwrap().outer_half_extent_meters, None); // 最粗层明确覆盖场景边界。
        assert_eq!(lods.last().unwrap().spacing_multiplier, 128); // 8米乘128恰好覆盖完整1024米。
    } // 默认不改变现有Clipmap画质级数。

    #[test] // 矩形场景短轴到边界后，末层只沿长轴扩展而不能形成倒置或空环。
    fn rectangular_default_keeps_a_valid_side_only_outer_band() {
        let spec = SparseHeightSpec::new([0.0, 0.0], [256.0, 128.0], 1.0, -8.0, 16.0).unwrap();
        let authored = default_display_lods_for_spec(&spec).unwrap();
        assert_eq!(
            authored
                .iter()
                .map(|lod| (lod.outer_half_extent_meters, lod.spacing_multiplier))
                .collect::<Vec<_>>(),
            vec![(Some(64.0), 1), (Some(128.0), 2), (None, 4)]
        ); // 完全复现旧64半格方环层级。
        let plan = compile_display_lod_plan(&spec, &authored).unwrap();
        assert_eq!(
            plan.actual_half_extents_meters.last(),
            Some(&[256.0, 128.0])
        );
        let outer = plan.bands.last().unwrap();
        assert!(outer.outer_half_cells[0] > outer.inner_half_cells[0]); // 长轴继续覆盖远端。
        assert_eq!(outer.outer_half_cells[1], outer.inner_half_cells[1]); // 短轴已覆盖，不在场景外伪造另一圈。
    }

    #[test] // 显式列表必须区分有效末层和各种损坏结构。
    fn shape_validation_rejects_ambiguous_lists() {
        assert!(validate_display_lod_shape(&[]).is_err()); // 空列表不能替代字段缺失。
        let valid = vec![
            SceneTerrainDisplayLod {
                outer_half_extent_meters: Some(16.0), // 第一层有限方形范围。
                spacing_multiplier: 1,                // 作者最高精度。
            },
            SceneTerrainDisplayLod {
                outer_half_extent_meters: Some(64.0), // 相同倍率的连续作者范围允许合并。
                spacing_multiplier: 1,                // 不强迫无意义的倍率变化。
            },
            SceneTerrainDisplayLod {
                outer_half_extent_meters: None, // 末层覆盖场景边界。
                spacing_multiplier: 8,          // 跳级交给内部2:1过渡带。
            },
        ]; // 完成合法多层夹具。
        assert!(validate_display_lod_shape(&valid).is_ok()); // 合法列表通过。
        let mut invalid = valid.clone(); // 每项变体独立验证。
        invalid[0].outer_half_extent_meters = None; // 非末层不得无限延伸。
        assert!(validate_display_lod_shape(&invalid).is_err()); // 中途None被拒绝。
        let mut invalid = valid.clone(); // 恢复合法基线。
        invalid[2].outer_half_extent_meters = Some(128.0); // 末层有限会留下场景空洞。
        assert!(validate_display_lod_shape(&invalid).is_err()); // 末层必须None。
        let mut invalid = valid.clone(); // 恢复合法基线。
        invalid[1].spacing_multiplier = 3; // 非二次幂不可进入Shader指数。
        assert!(validate_display_lod_shape(&invalid).is_err()); // 非二次幂被拒绝。
        let mut invalid = valid; // 使用最后一份合法基线。
        invalid[1].outer_half_extent_meters = Some(16.0); // 有限范围不再严格增加。
        assert!(validate_display_lod_shape(&invalid).is_err()); // 重复范围被拒绝。
    } // 不静默修复任何作者输入。

    #[test] // 旧自动列表编译后必须仍是64半格、逐级二倍且页窗口半径为128格。
    fn compiled_default_is_legacy_equivalent() {
        let spec = SparseHeightSpec::new(
            [0.0, 0.0],       // 最大规格从场景角验证全图覆盖。
            [8192.0, 8192.0], // 两轴完整长度相同。
            0.125,            // 作者最细格距。
            -1024.0,          // 高度编码与显示计划无关。
            2048.0,           // 高度量程与显示计划无关。
        )
        .unwrap(); // 合法最大场景。
        let authored = default_display_lods_for_spec(&spec).unwrap(); // 旧64半格算法的显式等价列表。
        let plan = compile_display_lod_plan(&spec, &authored).unwrap(); // 编译真实环带和分页范围。
        assert_eq!(plan.bands.len(), authored.len()); // 连续翻倍无需插入内部层。
        for band in &plan.bands {
            assert_eq!(band.outer_half_cells, [64, 64]); // 每一级外边界仍固定64个当前格。
            if band.inner_half_cells != [0, 0] {
                assert_eq!(band.inner_half_cells, [32, 32]); // 相邻级内洞仍为半径一半。
            } // 首层是完整中心方格。
        } // 完成全部旧环带比较。
        for range in &plan.sample_ranges {
            if range.mip_level < spec.max_mip_level() {
                assert_eq!(range.half_cells[0] + DISPLAY_LOD_RESIDENCY_GUARD_CELLS, 128); // 未钳制层仍是可见64格加保护64格。
            } else {
                assert_eq!(spec.mip_tile_counts(range.mip_level), Some([1, 1])); // 更远几何钳制到最高Mip时整图本就只有一页。
            } // 两种情况选择的实际页集合都与旧固定窗口相同。
        } // 完成全部默认采样层比较。
        assert!(plan.mesh_bytes <= MAX_DISPLAY_LOD_MESH_BYTES); // 最大旧图继续通过16MiB预算。
        assert!(
            worst_case_display_page_upload_bytes(&spec, &plan).unwrap()
                <= MAX_DISPLAY_LOD_PAGE_BYTES
        ); // 默认全非空页继续通过8MiB预算。
    } // 旧场景缺字段不会发生画质或资源规模迁移。

    #[test] // 相同倍率合并，跳级只在运行时补齐相邻2:1环且不改作者列表。
    fn compiler_merges_same_multiplier_and_inserts_jump_bands() {
        let spec = SparseHeightSpec::new(
            [0.0, 0.0],     // 简单原点便于检查整数半径。
            [512.0, 256.0], // 矩形场景覆盖末层两轴不同边界。
            1.0,            // 基础格一米。
            -32.0,          // 高度编码与本测试无关。
            64.0,           // 高度量程与本测试无关。
        )
        .unwrap(); // 合法矩形规格。
        let authored = vec![
            SceneTerrainDisplayLod {
                outer_half_extent_meters: Some(8.0), // 会被后一同倍率范围合并。
                spacing_multiplier: 1,               // 最细作者层。
            },
            SceneTerrainDisplayLod {
                outer_half_extent_meters: Some(16.0), // 真正决定×1外边界。
                spacing_multiplier: 1,                // 相同倍率不生成空重复环。
            },
            SceneTerrainDisplayLod {
                outer_half_extent_meters: None, // ×8最终覆盖场景边界。
                spacing_multiplier: 8,          // 运行时需补×2和×4。
            },
        ]; // 作者列表保持三行。
        let plan = compile_display_lod_plan(&spec, &authored).unwrap(); // 编译有效环带。
        assert_eq!(plan.authored_lods, authored); // 编译不能改写作者列表。
        assert_eq!(
            plan.bands
                .iter()
                .map(|band| band.spacing_multiplier)
                .collect::<Vec<_>>(),
            vec![1, 2, 4, 8]
        ); // 跳级被补为严格相邻倍率。
        assert_eq!(plan.bands[0].source_lod_index, Some(1)); // 同倍率归到最后范围行。
        assert_eq!(plan.bands[1].source_lod_index, None); // ×2仅是内部过渡。
        assert_eq!(plan.bands[2].source_lod_index, None); // ×4仅是内部过渡。
        assert_eq!(plan.bands[3].source_lod_index, Some(2)); // 最终×8仍指向作者末层。
        for pair in plan.bands.windows(2) {
            assert_eq!(pair[1].spacing_multiplier, pair[0].spacing_multiplier * 2); // 任意相邻真实环带都只做2:1连接。
            for axis in 0..2 {
                let fine_outer =
                    pair[0].outer_half_cells[axis] as u64 * u64::from(pair[0].spacing_multiplier);
                let coarse_inner =
                    pair[1].inner_half_cells[axis] as u64 * u64::from(pair[1].spacing_multiplier);
                assert_eq!(fine_outer, coarse_inner); // 世界边界逐轴完全一致。
            } // 完成X/Z边界对照。
        } // 完成全部相邻环带检查。
    } // 内部补带不进入序列化数据。

    #[test] // 作者有限边界量化后若不能形成两轴非空环必须明确拒绝。
    fn compiler_rejects_quantized_empty_ring() {
        let spec = SparseHeightSpec::new(
            [0.0, 0.0],   // 测试原点。
            [64.0, 64.0], // 有限小图。
            1.0,          // 一米基础格。
            -8.0,         // 高度基线。
            16.0,         // 高度量程。
        )
        .unwrap(); // 合法规格。
        let authored = vec![
            SceneTerrainDisplayLod {
                outer_half_extent_meters: Some(16.0), // 首层量化为16米。
                spacing_multiplier: 1,                // 最细层。
            },
            SceneTerrainDisplayLod {
                outer_half_extent_meters: Some(16.1), // 按×4粗格量化仍可能只到20米。
                spacing_multiplier: 2,                // 中层只留下两米宽，仍合法。
            },
            SceneTerrainDisplayLod {
                outer_half_extent_meters: None, // 全图末层。
                spacing_multiplier: 4,          // 相邻倍率。
            },
        ]; // 先确认薄环仍可表达。
        assert!(compile_display_lod_plan(&spec, &authored).is_ok()); // 至少一格的薄环不得误拒绝。
        let oversized = vec![
            SceneTerrainDisplayLod {
                outer_half_extent_meters: Some(64.0), // 已经覆盖完整场景。
                spacing_multiplier: 1,                // 首层外边界超过末层可用范围。
            },
            SceneTerrainDisplayLod {
                outer_half_extent_meters: None, // 最终层没有剩余环带。
                spacing_multiplier: 2,          // 量化后内边界不小于全图外边界。
            },
        ]; // 完成退化配置。
        assert!(compile_display_lod_plan(&spec, &oversized).is_err()); // 不静默删除空末环。
    } // 保存预检与运行时使用相同错误。
    #[test]
    fn external_camera_coverage_does_not_invent_persistent_height_mips() {
        let spec = SparseHeightSpec::new([-32.0; 2], [32.0; 2], 1.0, -8.0, 16.0).unwrap();
        let lods = [SceneTerrainDisplayLod { spacing_multiplier: 32, outer_half_extent_meters: None }];
        let plan = compile_display_lod_plan_with_coverage(&spec, &lods, [512, 1024]).unwrap();
        assert_eq!(plan.actual_half_extents_meters, [[512.0, 1024.0]]);
        assert!(plan.sample_ranges.iter().all(|range| range.mip_level <= spec.max_mip_level()));
        assert!(compile_display_lod_plan_with_coverage(&spec, &lods, [u64::MAX; 2]).is_err());
    }
} // 结束显示LOD纯数据测试。

#[cfg(test)]
#[path = "display_lods_suffix_tests.rs"]
mod suffix_tests; // 运行时后缀保留矩形轴边界、内部过渡和既有上传预算。
