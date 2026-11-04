//! 固定预算三维河流、急流与瀑布带状网格纯构建器。
//!
//! 【功能设计】作者点按米制间距采样；河宽始终在水平XZ平面，只有瀑布平行传输横轴。
//! UV、切线、逐三角累计法线与互动足迹同源；运行时只登记完成数组，不在帧内重采样。

use bevy::prelude::{Vec2, Vec3}; // 纯构建阶段使用Bevy正式数学类型，不访问ECS或渲染世界。
use feathrise_map_content::{
    MapWaterRiver, MapWaterSegmentKind, MapWaterSplinePoint, MapWaterfall,
}; // 公共作者类型是河流、急流与瀑布的唯一数据来源。

use super::constants::RIBBON_STYLE_SENTINEL; // 样条与Blender导入整网格共用唯一UV1模式哨兵，避免Rust编码分叉。
use super::footprint::RibbonWaterFootprint; // 斜河互动足迹逐顶点复用实际渲染三角剖分。
use super::painted::{
    BuiltWaterSurface, PaintedWaterMeshData, WaterSurfaceDescriptor, WaterSurfaceKind,
    legacy_macro_waves, legacy_micro_normals,
}; // 带状完成值复用湖面相同的统一安装数据结构。

/// 旧预算常量保留API兼容与回归阈值，不再限制作者内容数量。
pub const MAX_RIVER_RIBBON_SECTIONS: usize = 2_048;

/// 旧预算常量保留API兼容与回归阈值，不再限制作者内容数量。
pub const MAX_WATERFALL_RIBBON_SECTIONS: usize = 512;

/// 旧预算常量保留API兼容与回归阈值，不再限制作者内容数量。
pub const MAX_MAP_SPLINE_WATER_TRIANGLES: usize = 131_072;

const LONGITUDINAL_SAMPLE_METERS: f32 = 2.0; // 两米纵向采样兼顾弯道连续度与最坏网格预算。
const LATERAL_SAMPLE_METERS: f32 = 4.0; // 四米横向目标尺寸避免宽河只有两条岸线顶点。
const MIN_LATERAL_CELLS: usize = 2; // 每个截面至少三个顶点，保留中心水面顶点和稳定法线。
const MIN_SEGMENT_LENGTH_METERS: f32 = 0.0001; // 公共入口独立拒绝无法产生切线的退化段。
const STYLE_RIVER: f32 = 0.0; // 哨兵后的零进入普通河流材质分支。
const STYLE_RAPID: f32 = 1.0; // 哨兵后的一增强急流波幅、频率和泡沫。
const STYLE_WATERFALL: f32 = 2.0; // 哨兵后的二切换为沿幕布法向位移的瀑布分支。

mod budget; // 整资产保存预检复用正式单体builder，并保持主网格模块低于维护行数上限。
mod validation; // 公开构建入口的控制点与PBR参数失败关闭规则独立维护。

pub use budget::validate_spline_water_build_budget; // 编辑器保存共享正式几何可构建性校验。
use validation::{validate_material, validate_points}; // 河流与瀑布入口复用相同纯校验边界。

/// 一个经过固定预算采样的三维中心截面。
#[derive(Clone, Copy, Debug)] // 小型纯值只在一次后台构建调用内连续保存。
struct SampledSection {
    center: Vec3,    // 当前截面的三维中心位置。
    width: f32,      // 当前完整水面宽度。
    depth: f32,      // 当前作者玩法水深。
    flow_speed: f32, // 当前局部流速，稍后写入UV1.x。
    style: f32,      // 当前河流、急流或瀑布解码样式，写入UV1.y时再叠加高位身份哨兵。
    arc_length: f32, // 从路径起点到当前截面的真实累计米数。
    tangent: Vec3,   // 从上游指向下游的单位局部流向。
    side: Vec3,      // 与切线垂直且连续的单位横向轴。
} // 结束后台采样截面定义。

/// 从一条作者河流构建统一水面完成值；调用方必须在有界后台任务中执行。
pub fn build_river_water_surface(river: &MapWaterRiver) -> Result<BuiltWaterSurface, String> {
    validate_material(
        &format!("河流{}", river.id), // 材质错误诊断携带稳定河流ID。
        river.base_color_srgba,       // 验证全部sRGBA分量处于零至一。
        river.perceptual_roughness,   // 验证PBR粗糙度处于零至一。
    )?;
    let sections = sample_path(
        &river.points,                // 河流控制点按作者上游至下游顺序采样。
        MAX_RIVER_RIBBON_SECTIONS,    // 旧阈值仅兼容参数，不再截断或拒绝采样。
        false,                        // 河段读取每个起点的普通河流或急流样式。
        &format!("河流{}", river.id), // 所有失败诊断携带稳定河流ID。
    )?;
    let flow_speed = average_flow_speed(&sections); // 描述保留平均值供缺失UV1时安全回退。
    let mesh = build_ribbon_mesh(&sections, true, 0.0)?; // 河流建立实际斜面互动足迹且不需要瀑布边缘泡沫。
    Ok(BuiltWaterSurface {
        descriptor: WaterSurfaceDescriptor {
            id: river.id.clone(),                             // 统一marker使用稳定河流ID。
            kind: WaterSurfaceKind::River, // 普通河段和急流段共享三维河流实体类别。
            base_color_srgba: river.base_color_srgba, // 复用当前河流作者PBR颜色。
            perceptual_roughness: river.perceptual_roughness, // 复用当前河流作者PBR粗糙度。
            reflectance: 0.5,              // 样条水暂沿用稳定非金属反射基准。
            trough_darkening: super::constants::PLAYER_RIPPLE_TROUGH_DARKENING,
            shore_wave_attenuation: 1.0,
            macro_waves: legacy_macro_waves(), // 河流的相位仍由流速分支推进。
            micro_normals: legacy_micro_normals(),
            micro_normal_textures: [None, None],
            flow_direction: Vec2::X, // 样条水从切线与UV1读取流向，uniform仅保留稳定回退。
            flow_speed,              // 样条UV缺失时仍使用有限平均流速。
            translation_y: 0.0,      // 三维顶点已经位于世界局部绝对高度，不再叠加水位平移。
            rest_water_level: 0.0,   // 河流互动由带状足迹返回实际局部Y。
            interaction_enabled: true, // 河流和急流继续允许玩家脚步与跳跃波纹。
        },
        mesh, // 移交完整连续数组与同源斜河互动足迹。
    })
}

/// 从一条作者瀑布构建近竖直幕布；瀑布明确不创建玩家互动足迹。
pub fn build_waterfall_water_surface(
    waterfall: &MapWaterfall,
) -> Result<BuiltWaterSurface, String> {
    validate_material(
        &format!("瀑布{}", waterfall.id), // 材质错误诊断携带稳定瀑布ID。
        waterfall.base_color_srgba,       // 验证全部sRGBA分量处于零至一。
        waterfall.perceptual_roughness,   // 验证PBR粗糙度处于零至一。
    )?;
    if !waterfall.foam_strength.is_finite() || !(0.0..=1.0).contains(&waterfall.foam_strength) {
        return Err(format!("瀑布{}包含无效泡沫强度", waterfall.id)); // 非有限泡沫值不得污染顶点乘色。
    }
    let sections = sample_path(
        &waterfall.points,                // 瀑布控制点按瀑布口至落水区顺序采样。
        MAX_WATERFALL_RIBBON_SECTIONS,    // 旧阈值仅兼容参数，不再截断或拒绝采样。
        true,                             // 全部瀑布截面强制使用独立幕布样式。
        &format!("瀑布{}", waterfall.id), // 所有失败诊断携带稳定瀑布ID。
    )?;
    let flow_speed = average_flow_speed(&sections); // 平均值仅用于材质缺省和诊断一致性。
    let mesh = build_ribbon_mesh(&sections, false, waterfall.foam_strength)?; // 瀑布只生成视觉幕布，不生成玩家足迹。
    Ok(BuiltWaterSurface {
        descriptor: WaterSurfaceDescriptor {
            id: waterfall.id.clone(),                     // 统一marker使用稳定瀑布ID。
            kind: WaterSurfaceKind::Waterfall,            // 幕布类别供查询和后续独立泡沫实体识别。
            base_color_srgba: waterfall.base_color_srgba, // 复用当前瀑布作者PBR颜色。
            perceptual_roughness: waterfall.perceptual_roughness, // 复用当前瀑布作者PBR粗糙度。
            reflectance: 0.5,
            trough_darkening: super::constants::PLAYER_RIPPLE_TROUGH_DARKENING,
            shore_wave_attenuation: 1.0,
            macro_waves: legacy_macro_waves(),
            micro_normals: legacy_micro_normals(),
            micro_normal_textures: [None, None],
            flow_direction: Vec2::X, // 瀑布逐顶点读取实际静止法线，uniform只保留有限回退值。
            flow_speed,              // UV1逐截面仍是正式局部瀑布流速来源。
            translation_y: 0.0,      // 三维幕布顶点直接保存绝对作者高度。
            rest_water_level: 0.0,   // Disabled互动分支不会读取标量静止水位。
            interaction_enabled: false, // 瀑布持续动画不能占用或淘汰八槽玩家波源。
        },
        mesh, // 移交完整幕布连续数组且不携带互动足迹。
    })
}

/// 按两米目标间距采样Catmull-Rom中心线，并在预算采用前构建连续局部标架。
fn sample_path(
    points: &[MapWaterSplinePoint], // 经过公共资产校验或独立调用传入的作者控制点。
    _legacy_max_sections: usize,    // 旧API参数仅兼容；路径按实际长度完整采样。
    waterfall: bool,                // true时忽略outgoing并强制瀑布材质样式。
    owner: &str,                    // 稳定诊断前缀。
) -> Result<Vec<SampledSection>, String> {
    validate_points(points, owner)?; // 公共纯入口不能依赖调用方一定先执行完整资产校验。
    let mut segment_steps = Vec::with_capacity(points.len() - 1); // 先规划全部段步数，超预算时不分配大顶点数组。
    let mut section_count = 1usize; // 路径起点始终占一个截面。
    for pair in points.windows(2) {
        let first = Vec3::from_array(pair[0].position); // 当前段上游三维位置。
        let second = Vec3::from_array(pair[1].position); // 当前段下游三维位置。
        let steps = (first.distance(second) / LONGITUDINAL_SAMPLE_METERS)
            .ceil()
            .max(1.0) as usize; // 每段至少一格且按真实三维弧段长度增加采样。
        section_count = section_count
            .checked_add(steps)
            .ok_or_else(|| format!("{owner}采样截面数量溢出"))?; // 损坏输入不能绕过固定预算。
        segment_steps.push(steps); // 保存受检段步数供第二遍确定采样。
    }
    let mut raw = Vec::new();
    raw.try_reserve_exact(section_count)
        .map_err(|e| format!("{owner}截面内存分配失败：{e}"))?; // 只由实际分配能力决定容量。
    raw.push(raw_section(points, 0, 0.0, waterfall)); // 显式保留第一个作者控制点。
    for (segment_index, steps) in segment_steps.into_iter().enumerate() {
        for step in 1..=steps {
            let interpolation = step as f32 / steps as f32; // 当前段零到一确定采样参数。
            raw.push(raw_section(points, segment_index, interpolation, waterfall)); // 相邻段不重复共享端点。
        }
    }
    finalize_frames(raw, waterfall, owner) // 计算累计米、切线与连续侧向标架后发布。
}

/// 生成一个尚未计算局部标架和累计米的样条截面。
fn raw_section(
    points: &[MapWaterSplinePoint], // 完整作者路径用于读取相邻Catmull-Rom控制点。
    segment_index: usize,           // 当前采样所属的上游控制点下标。
    interpolation: f32,             // 当前段内零到一参数。
    waterfall: bool,                // true时强制UV样式为瀑布。
) -> SampledSection {
    let first_index = segment_index.saturating_sub(1); // 首段重复首点形成稳定端点切线。
    let second_index = segment_index; // 当前段起点。
    let third_index = (segment_index + 1).min(points.len() - 1); // 当前段终点。
    let fourth_index = (segment_index + 2).min(points.len() - 1); // 末段重复末点形成稳定端点切线。
    let first = Vec3::from_array(points[first_index].position); // Catmull-Rom前邻点。
    let second = Vec3::from_array(points[second_index].position); // Catmull-Rom当前段起点。
    let third = Vec3::from_array(points[third_index].position); // Catmull-Rom当前段终点。
    let fourth = Vec3::from_array(points[fourth_index].position); // Catmull-Rom后邻点。
    let mut center = catmull_rom(first, second, third, fourth, interpolation); // XZ和Y共同形成平滑三维中心线。
    center.y = center.y.clamp(second.y.min(third.y), second.y.max(third.y)); // 防止样条Y过冲制造逆流或越过端点水位。
    let width = scalar_lerp(
        points[second_index].width, // 当前段起点完整宽度。
        points[third_index].width,  // 当前段终点完整宽度。
        interpolation,              // 与中心线相同的段内参数。
    ); // 截面宽度在线段内稳定插值且不出现样条过冲。
    let depth = scalar_lerp(
        points[second_index].depth, // 当前段起点作者深度。
        points[third_index].depth,  // 当前段终点作者深度。
        interpolation,              // 与中心线相同的段内参数。
    ); // 作者玩法深度与实际中心线使用同一段参数。
    let flow_speed = scalar_lerp(
        points[second_index].flow_speed, // 当前段起点作者流速。
        points[third_index].flow_speed,  // 当前段终点作者流速。
        interpolation,                   // 与中心线相同的段内参数。
    ); // UV1局部流速在控制点间连续。
    let style = if waterfall {
        STYLE_WATERFALL // 瀑布所有段保持独立幕布分支。
    } else {
        match points[second_index].outgoing {
            MapWaterSegmentKind::River => STYLE_RIVER, // 普通河段保持基础动画。
            MapWaterSegmentKind::Rapid => STYLE_RAPID, // 急流段增强频率、振幅和纵向速度。
        }
    };
    SampledSection {
        center,              // 暂存三维中心点。
        width,               // 暂存插值宽度。
        depth,               // 暂存插值作者深度。
        flow_speed,          // 暂存插值局部流速。
        style,               // 暂存离散段样式。
        arc_length: 0.0,     // 第二阶段根据实际相邻中心点累计。
        tangent: Vec3::ZERO, // 第二阶段根据相邻采样点计算。
        side: Vec3::ZERO,    // 第二阶段为河流建立水平横轴，或为瀑布执行平行传输。
    }
}

/// 使用标准均匀Catmull-Rom公式平滑作者中心线。
fn catmull_rom(first: Vec3, second: Vec3, third: Vec3, fourth: Vec3, t: f32) -> Vec3 {
    let squared = t * t; // 二次项只计算一次。
    let cubed = squared * t; // 三次项只计算一次。
    (second * 2.0
        + (third - first) * t
        + (first * 2.0 - second * 5.0 + third * 4.0 - fourth) * squared
        + (-first + second * 3.0 - third * 3.0 + fourth) * cubed)
        * 0.5 // 标准均匀Catmull-Rom基函数系数。
}

/// 在稳定Rust浮点API上执行确定的线性插值，不依赖额外扩展trait进入作用域。
fn scalar_lerp(start: f32, end: f32, t: f32) -> f32 {
    start + (end - start) * t // 受检有限端点和零至一参数保证结果有限且位于两端之间。
}

/// 为全部采样点计算实际累计米、中心差分切线和按水体类别约束的横向轴。
fn finalize_frames(
    mut sections: Vec<SampledSection>, // 已完成位置和作者标量采样的连续截面。
    waterfall: bool,                   // 河流宽度保持水平，瀑布横轴沿幕布平行传输。
    owner: &str,                       // 退化完成值的稳定诊断前缀。
) -> Result<Vec<SampledSection>, String> {
    for index in 1..sections.len() {
        let distance = sections[index - 1].center.distance(sections[index].center); // 使用实际平滑中心线段长累计UV米数。
        if !distance.is_finite() || distance <= MIN_SEGMENT_LENGTH_METERS {
            return Err(format!("{owner}采样后产生退化中心线段")); // Catmull回折或坏输入不能形成零切线网格。
        }
        sections[index].arc_length = sections[index - 1].arc_length + distance; // UV0.x严格单调累计真实米数。
    }
    let mut previous_side = Vec3::ZERO; // 用于保持河流横轴符号或连续传输瀑布横轴。
    let last_index = sections.len() - 1; // 有效路径至少有两个截面。
    for index in 0..sections.len() {
        let previous = sections[index.saturating_sub(1)].center; // 首端重复自身供前向差分。
        let next = sections[(index + 1).min(last_index)].center; // 末端重复自身供后向差分。
        let tangent = (next - previous).normalize_or_zero(); // 中间点采用中心差分减少折线法线跳变。
        if tangent == Vec3::ZERO {
            return Err(format!("{owner}无法建立有限局部流向")); // 防止零切线进入Mesh切线属性和Shader。
        }
        let reference_side = if !waterfall {
            let horizontal_side = Vec3::Y.cross(tangent); // 河流每个截面独立以Y×流向建立严格水平横轴。
            if horizontal_side.length_squared() <= 0.000001 {
                return Err(format!("{owner}包含近竖直河段，应改用瀑布水体")); // 竖直切线无法同时满足河宽水平和垂直于流向。
            }
            horizontal_side.normalize() // 非竖直河流的完整宽度始终位于同一世界Y。
        } else if index == 0 {
            initial_waterfall_side(tangent) // 第一片水幕建立确定且不退化的横向轴。
        } else {
            let transported = previous_side - tangent * previous_side.dot(tangent); // 把上一横向轴投影到当前法平面完成平行传输。
            if transported.length_squared() <= 0.000001 {
                initial_waterfall_side(tangent) // 极端一百八十度折返安全重建确定幕布横向轴。
            } else {
                transported.normalize() // 非退化投影保持连续横向轴。
            }
        };
        let mut side = reference_side; // 河流保留Y×流向的朝上绕序；瀑布才按历史横轴修正符号。
        if waterfall && index > 0 && side.dot(previous_side) < 0.0 {
            side = -side; // 防止瀑布平行传输数值重建造成幕布截面突然翻面。
        }
        sections[index].tangent = tangent; // 发布局部下游切线。
        sections[index].side = side; // 发布连续横向轴。
        previous_side = side; // 下一截面从当前横向轴继续平行传输。
    }
    Ok(sections) // 返回具备完整Mesh属性的受检截面。
}

/// 为近竖直或弯折瀑布建立确定且不退化的第一幕布横向轴。
fn initial_waterfall_side(tangent: Vec3) -> Vec3 {
    let horizontal_side = Vec3::Y.cross(tangent); // 非竖直瀑布优先让幕布宽度位于水平XZ平面。
    if horizontal_side.length_squared() > 0.000001 {
        return horizontal_side.normalize(); // 非竖直路径直接采用稳定水平横向轴。
    }
    let projected_x = Vec3::X - tangent * Vec3::X.dot(tangent); // 竖直瀑布优先沿世界X展开幕布宽度。
    if projected_x.length_squared() > 0.000001 {
        projected_x.normalize() // 非退化世界X投影形成稳定幕布横向轴。
    } else {
        Vec3::Z // 理论兜底仅用于切线与世界X完全共线的异常数值路径。
    }
}

/// 把完整截面网格化，并按策略选择斜河互动足迹或禁用瀑布互动。
fn build_ribbon_mesh(
    sections: &[SampledSection], // 位置、标架、累计米和作者标量均已完成的截面。
    build_interaction: bool,     // 只有河流与急流建立XZ查询足迹。
    foam_strength: f32,          // 瀑布顶底亮度提示使用受检作者泡沫强度。
) -> Result<PaintedWaterMeshData, String> {
    let maximum_width = sections
        .iter()
        .map(|section| section.width)
        .fold(0.0_f32, f32::max); // 全路径使用同一横向格数，保证纵向索引连续。
    let lateral_cells = ((maximum_width / LATERAL_SAMPLE_METERS).ceil() as usize)
        .max(MIN_LATERAL_CELLS); // 宽河仍保留相同米制精度，按实际宽度完整细分。
    let vertex_width = lateral_cells + 1; // 每截面顶点数比横向单元多一。
    let vertex_count = sections
        .len()
        .checked_mul(vertex_width)
        .ok_or_else(|| "带状水面顶点数量溢出".to_owned())?; // 固定预算前仍显式保护usize乘法。
    u32::try_from(vertex_count).map_err(|_| "带状水面顶点无法使用U32网格索引表示".to_owned())?;
    let triangle_count = (sections.len() - 1)
        .checked_mul(lateral_cells)
        .and_then(|cells| cells.checked_mul(2))
        .ok_or_else(|| "带状水面三角形数量溢出".to_owned())?; // 每个四边形固定两片三角形。
    let mut positions = Vec::with_capacity(vertex_count); // 一次分配完整三维顶点数组。
    let mut tangents = Vec::with_capacity(vertex_count); // 一次分配完整局部流向切线数组。
    let mut uvs = Vec::with_capacity(vertex_count); // 一次分配累计米与横向UV0数组。
    let mut uvs_b = Vec::with_capacity(vertex_count); // 一次分配局部流速与带身份哨兵样式的UV1数组。
    let mut colors = Vec::with_capacity(vertex_count); // 一次分配PBR乘色和幕布边缘Alpha数组。
    let mut footprint_vertices = build_interaction.then(|| Vec::with_capacity(vertex_count)); // 河流逐顶点保存实际Mesh几何，瀑布不分配互动数据。
    let mut footprint_depths = build_interaction.then(|| Vec::with_capacity(sections.len())); // 河流逐纵向截面保存作者深度。
    let mut footprint_waves = build_interaction.then(|| Vec::with_capacity(sections.len())); // 每截面只增12字节同源UV/流速/样式，不复制整份Mesh属性。
    for section in sections {
        if let Some(waves) = &mut footprint_waves {
            waves.push(Vec3::new(
                section.arc_length,
                section.flow_speed,
                section.style,
            )); // 同一截面值同时进入Mesh UV和水花高度索引。
        }
        if let Some(footprint_depths) = &mut footprint_depths {
            footprint_depths.push(section.depth); // 每个实际Mesh截面只保存一次作者深度。
        }
        for lateral_index in 0..=lateral_cells {
            let lateral_ratio = lateral_index as f32 / lateral_cells as f32; // 横向UV稳定覆盖零至一。
            let lateral_offset = (lateral_ratio - 0.5) * section.width; // 每个截面按自身宽度收放而不中断纵向拓扑。
            let position = section.center + section.side * lateral_offset; // 三维侧向轴支持倾斜河面与近竖直幕布。
            positions.push(position.to_array()); // 发布静止局部顶点。
            if let Some(footprint_vertices) = &mut footprint_vertices {
                footprint_vertices.push(position); // 河流足迹逐顶点复制实际静止Mesh位置，不再用两岸大四边形近似。
            }
            tangents.push([
                section.tangent.x, // tangent.x编码局部下游流向。
                section.tangent.y, // tangent.y保留斜坡和瀑布垂直分量。
                section.tangent.z, // tangent.z编码局部下游流向。
                1.0,               // 标准PBR切线手性保持正一。
            ]);
            uvs.push([section.arc_length, lateral_ratio]); // UV0.x累计米，UV0.y横向零至一。
            uvs_b.push([
                section.flow_speed,                    // UV1.x继续保存可插值的局部作者流速。
                RIBBON_STYLE_SENTINEL + section.style, // UV1.y用高位哨兵加离散样式，旧GLB普通第二套UV不会被误判为样条。
            ]); // 完成当前样条顶点专用UV1编码。
            let edge_alpha = if section.style >= STYLE_WATERFALL {
                (lateral_ratio.min(1.0 - lateral_ratio) * 4.0).clamp(0.0, 1.0) // 瀑布两侧一个四分之一宽度内平滑淡出。
            } else {
                1.0 // 河流岸边仍由水材质bank fade抑制位移，不裁掉实际水面颜色。
            };
            let path_ratio = section.arc_length
                / sections
                    .last()
                    .map_or(1.0, |last| last.arc_length.max(0.001)); // 瀑布顶底泡沫提示使用零至一路径比例。
            let foam_band = ((1.0 - path_ratio) * 4.0)
                .clamp(0.0, 1.0)
                .max(((path_ratio - 0.75) * 4.0).clamp(0.0, 1.0)); // 顶部和落水区形成有限泡沫亮带。
            let brightness = if section.style >= STYLE_WATERFALL {
                1.0 + foam_strength.clamp(0.0, 1.0) * foam_band * 0.35 // 作者泡沫强度克制提升幕布顶底亮度。
            } else {
                1.0 // 河流顶点颜色不修改作者基础水色。
            };
            colors.push([brightness, brightness, brightness, edge_alpha]); // 顶点RGB乘色和Alpha均保持有限。
        }
    }
    let mut indices = Vec::with_capacity(triangle_count * 3); // 精确预分配固定三角索引容量。
    for section_index in 0..sections.len() - 1 {
        for lateral_index in 0..lateral_cells {
            let first = (section_index * vertex_width + lateral_index) as u32; // 当前截面横向起点。
            let second = first + 1; // 当前截面下一横向顶点。
            let downstream_first = first + vertex_width as u32; // 下一截面同横向起点。
            let downstream_second = downstream_first + 1; // 下一截面下一横向顶点。
            indices.extend_from_slice(&[
                first,
                downstream_first,
                downstream_second, // 第一片三角形与互动足迹左半片共用对角线。
                first,
                downstream_second,
                second, // 第二片三角形与互动足迹右半片共用对角线。
            ]);
        }
    }
    let normals = smooth_vertex_normals(&positions, &indices)?; // 按实际全部小三角面积加权累计静止顶点法线。
    let ribbon_footprint = if let Some(footprint_vertices) = footprint_vertices {
        let footprint_depths =
            footprint_depths.ok_or_else(|| "带状水面互动深度完成值缺失".to_owned())?; // 两个互动数组必须原子发布。
        Some(
            RibbonWaterFootprint::from_ribbon_vertices(
                footprint_vertices, // 与Mesh位置逐顶点相同的静止河面。
                footprint_depths,   // 与Mesh纵向截面逐项相同的作者深度。
                vertex_width,       // 与上方实际索引构建完全相同的横向行宽。
            )?
            .with_wave_sections(
                footprint_waves.ok_or_else(|| "带状水面波形截面完成值缺失".to_owned())?,
            )?,
        ) // 静止几何和波形截面必须共同成功。
    } else {
        None // 瀑布不建立任何玩家互动足迹。
    };
    Ok(PaintedWaterMeshData::from_ribbon(
        positions,        // 完整三维静止位置。
        normals,          // 完整三维静止法线。
        tangents,         // 完整局部流向切线。
        uvs,              // 完整累计米和横向UV0。
        uvs_b,            // 完整局部流速和带身份哨兵样式的UV1。
        colors,           // 完整顶点乘色与Alpha。
        indices,          // 完整固定三角列表。
        ribbon_footprint, // 河流Some，瀑布None。
    ))
}

/// 按实际索引三角形的面积向量累计并平滑静止顶点法线。
fn smooth_vertex_normals(
    positions: &[[f32; 3]], // 已完成的实际带状静止顶点。
    indices: &[u32],        // 与最终Mesh完全相同的三角列表。
) -> Result<Vec<[f32; 3]>, String> {
    if indices.len() % 3 != 0 {
        return Err("带状水面三角索引数量无效".to_owned()); // 不完整三角不能产生可发布PBR法线。
    }
    let mut accumulated = vec![Vec3::ZERO; positions.len()]; // 面积向量自然为较大三角提供更高平滑权重。
    for triangle in indices.chunks_exact(3) {
        let first_index = triangle[0] as usize; // 当前三角第一角顶点下标。
        let second_index = triangle[1] as usize; // 当前三角第二角顶点下标。
        let third_index = triangle[2] as usize; // 当前三角第三角顶点下标。
        let first = Vec3::from_array(
            *positions
                .get(first_index)
                .ok_or_else(|| "带状水面三角索引越界".to_owned())?,
        );
        let second = Vec3::from_array(
            *positions
                .get(second_index)
                .ok_or_else(|| "带状水面三角索引越界".to_owned())?,
        );
        let third = Vec3::from_array(
            *positions
                .get(third_index)
                .ok_or_else(|| "带状水面三角索引越界".to_owned())?,
        );
        let face_normal = (second - first).cross(third - first); // 保留两倍三角面积作为平滑权重。
        if !face_normal.is_finite() || face_normal.length_squared() <= 0.000000000001 {
            return Err("带状水面采样后产生退化三角形".to_owned()); // 退化面不能静默把零或NaN法线送入GPU。
        }
        accumulated[first_index] += face_normal; // 第一角累计当前实际三角面积向量。
        accumulated[second_index] += face_normal; // 第二角累计当前实际三角面积向量。
        accumulated[third_index] += face_normal; // 第三角累计当前实际三角面积向量。
    }
    accumulated
        .into_iter()
        .map(|normal| {
            let normal = normal.normalize_or_zero(); // 归一化相邻实际三角的面积加权和。
            if normal == Vec3::ZERO || !normal.is_finite() {
                Err("带状水面顶点无法建立有限平滑法线".to_owned()) // 孤立或相互抵消的顶点不能进入PBR。
            } else {
                Ok(normal.to_array()) // 发布逐顶点静止基准法线供主/预/Deferred路径共用。
            }
        })
        .collect()
}

/// 计算受检样条截面的有限平均流速作为uniform回退值。
fn average_flow_speed(sections: &[SampledSection]) -> f32 {
    sections
        .iter()
        .map(|section| section.flow_speed)
        .sum::<f32>()
        / sections.len() as f32 // 非空受检截面保证分母有效。
}

#[cfg(test)]
mod tests; // 斜河高度/深度、瀑布无互动和截面硬预算回归拆到独立测试模块。
