//! 景观动态代理共享的源网格属性读取与派生属性工具。
//! 【功能设计】本模块只处理内容无关的Mesh格式校验、边界、法线和切线，不决定灌木或岩石代理算法。

use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues}; // 统一读取glTF网格的拓扑、索引和顶点属性。
use bevy::prelude::*; // Mesh与Vec3提供共享网格运算所需基础类型。

/// 要求源网格使用三角列表。
pub(super) fn require_triangle_list(source: &Mesh, label: &str) -> Result<(), String> {
    if source.primitive_topology() != PrimitiveTopology::TriangleList {
        return Err(format!(
            "{label}最低档网格不是TriangleList，无法生成第四级代理"
        )); // 拒绝猜测条带或线段拓扑。
    } // 完成源拓扑校验。
    Ok(()) // 当前glTF primitive满足动态代理输入契约。
} // 完成拓扑校验helper。

/// 读取必需的Float32x3网格属性。
pub(super) fn mesh_float3_attribute<'a>(
    source: &'a Mesh,                           // 读取主世界仍保留CPU数据的源网格。
    attribute: bevy::mesh::MeshVertexAttribute, // 使用Bevy稳定内建属性身份。
    label: &str,                                // 错误消息说明缺失的具体资产字段。
) -> Result<&'a [[f32; 3]], String> {
    match source.attribute(attribute) {
        Some(VertexAttributeValues::Float32x3(values)) => Ok(values), // glTF位置和法线的标准解码格式。
        Some(_) => Err(format!("{label}不是Float32x3格式")),          // 不执行有损隐式转换。
        None => Err(format!("{label}属性缺失")),                      // 缺位置时无法形成任何代理。
    } // 完成属性格式分派。
} // 完成Float32x3属性读取。

/// 读取可选的Float32x3网格属性。
pub(super) fn optional_float3_attribute<'a>(
    source: &'a Mesh,                           // 读取源网格。
    attribute: bevy::mesh::MeshVertexAttribute, // 当前可选属性身份。
    label: &str,                                // 异常格式的诊断名。
) -> Result<Option<&'a [[f32; 3]]>, String> {
    match source.attribute(attribute) {
        Some(VertexAttributeValues::Float32x3(values)) => Ok(Some(values)), // 保留现有法线等属性。
        Some(_) => Err(format!("{label}不是Float32x3格式")), // 存在却格式不符时拒绝静默丢失。
        None => Ok(None), // 缺少可选属性时由代理构建器生成安全替代。
    } // 完成可选属性读取。
} // 完成可选Float32x3读取。

/// 读取可选的Float32x2网格属性。
pub(super) fn optional_float2_attribute<'a>(
    source: &'a Mesh,                           // 读取源网格。
    attribute: bevy::mesh::MeshVertexAttribute, // 当前可选属性身份。
    label: &str,                                // 异常格式的诊断名。
) -> Result<Option<&'a [[f32; 2]]>, String> {
    match source.attribute(attribute) {
        Some(VertexAttributeValues::Float32x2(values)) => Ok(Some(values)), // 保留最低档现有UV。
        Some(_) => Err(format!("{label}不是Float32x2格式")), // UV格式不符时禁止错位复制。
        None => Ok(None),                                    // 无纹理网格允许缺省。
    } // 完成可选属性读取。
} // 完成可选Float32x2读取。

/// 读取可选的Float32x4网格属性。
pub(super) fn optional_float4_attribute<'a>(
    source: &'a Mesh,                           // 读取源网格。
    attribute: bevy::mesh::MeshVertexAttribute, // 当前可选属性身份。
    label: &str,                                // 异常格式的诊断名。
) -> Result<Option<&'a [[f32; 4]]>, String> {
    match source.attribute(attribute) {
        Some(VertexAttributeValues::Float32x4(values)) => Ok(Some(values)), // 保留最低档现有切线。
        Some(_) => Err(format!("{label}不是Float32x4格式")), // 切线格式不符时禁止构建错误顶点布局。
        None => Ok(None), // 无切线时后续可从法线生成近似正交方向。
    } // 完成可选属性读取。
} // 完成可选Float32x4读取。

/// 把U16、U32或非索引三角网格统一为U32索引。
pub(super) fn mesh_triangle_indices(
    source: &Mesh,       // 读取源索引缓冲。
    vertex_count: usize, // 非索引网格按顶点连续顺序生成索引。
    label: &str,         // 错误消息中的资产类别。
) -> Result<Vec<u32>, String> {
    let indices = match source.indices() {
        Some(Indices::U16(indices)) => indices.iter().map(|index| u32::from(*index)).collect(), // U16无损扩展。
        Some(Indices::U32(indices)) => indices.clone(), // U32直接复制到短寿命选择表。
        None => (0..u32::try_from(vertex_count)
            .map_err(|_| format!("{label}最低档网格顶点数超过U32索引范围"))?)
            .collect(), // 非索引三角列表按连续顶点构造。
    }; // 完成索引格式统一。
    if indices.len() % 3 != 0 {
        return Err(format!("{label}最低档索引数不是三的倍数")); // 禁止忽略残缺面。
    } // 完成三角列表长度校验。
    if indices.iter().any(|index| *index as usize >= vertex_count) {
        return Err(format!("{label}最低档索引越过位置属性长度")); // 防止资产损坏导致数组越界。
    } // 完成索引范围校验。
    Ok(indices) // 返回可由两个代理算法共享的统一索引。
} // 完成索引读取helper。

/// 计算有限位置的最小值和安全三轴跨度。
pub(super) fn position_bounds(positions: &[[f32; 3]]) -> Result<(Vec3, Vec3), String> {
    let mut minimum = Vec3::splat(f32::INFINITY); // 从正无穷向真实最小值收敛。
    let mut maximum = Vec3::splat(f32::NEG_INFINITY); // 从负无穷向真实最大值收敛。
    for position in positions {
        let point = Vec3::from_array(*position); // 数组转换为向量。
        if !point.is_finite() {
            return Err("最低档网格包含非有限位置".to_string()); // 代理生成不传播NaN或Inf。
        } // 完成位置有限性保护。
        minimum = minimum.min(point); // 更新三轴最小值。
        maximum = maximum.max(point); // 更新三轴最大值。
    } // 完成全部位置扫描。
    if positions.is_empty() {
        return Err("最低档网格位置属性为空".to_string()); // 空网格不能形成代理。
    } // 完成位置非空保护。
    let extent = (maximum - minimum).max(Vec3::splat(1.0e-5)); // 扁平轴保留安全除数。
    Ok((minimum, extent)) // 返回选择和UV生成共用边界。
} // 完成位置边界helper。

/// 为闭合索引网格生成面积加权平滑法线。
pub(super) fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut accumulated = vec![Vec3::ZERO; positions.len()]; // 每个顶点累计相邻面面积法线。
    for triangle in indices.chunks_exact(3) {
        let a = Vec3::from_array(positions[triangle[0] as usize]); // 读取第一点。
        let b = Vec3::from_array(positions[triangle[1] as usize]); // 读取第二点。
        let c = Vec3::from_array(positions[triangle[2] as usize]); // 读取第三点。
        let area_normal = (b - a).cross(c - a); // 面积权重随叉积长度自然保留。
        accumulated[triangle[0] as usize] += area_normal; // 第一顶点累计当前面。
        accumulated[triangle[1] as usize] += area_normal; // 第二顶点累计当前面。
        accumulated[triangle[2] as usize] += area_normal; // 第三顶点累计当前面。
    } // 完成全部相邻面累加。
    accumulated
        .into_iter() // 每顶点归一化为PBR所需单位方向。
        .map(|normal| normal.normalize_or(Vec3::Y).to_array()) // 极端抵消时安全回退向上。
        .collect() // 返回与位置等长法线属性。
} // 完成平滑法线生成。

/// 从单位法线生成稳定正交切线。
pub(super) fn tangent_from_normal(normal: Vec3) -> [f32; 4] {
    let axis = if normal.y.abs() < 0.9 {
        Vec3::Y // 大多数表面以世界上轴作为切线参考。
    } else {
        Vec3::X // 接近水平面的法线改用X轴避免叉积退化。
    }; // 完成参考轴选择。
    let tangent = axis.cross(normal).normalize_or(Vec3::X); // 生成与法线正交的稳定单位方向。
    [tangent.x, tangent.y, tangent.z, 1.0] // w正一声明右手切线空间。
} // 完成切线生成。
