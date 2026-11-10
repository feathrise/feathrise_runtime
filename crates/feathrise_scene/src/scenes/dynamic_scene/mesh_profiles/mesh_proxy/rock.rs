//! 岩石动态超低模共享代理生成。
//! 【功能设计】对最低档源网格的有限唯一点做确定性分散采样，再生成带完整PBR属性的封闭凸包。

use std::collections::HashSet; // 岩石输入点去重只发生在场景加载阶段。

use avian3d::parry::math::Vector; // 复用Avian公开的同版本Parry身份，避免直接依赖产生版本漂移。
use avian3d::parry::transformation::try_convex_hull;
use bevy::asset::RenderAssetUsages; // 动态代理同时保留主世界诊断数据和渲染世界上传数据。
use bevy::mesh::{Indices, PrimitiveTopology}; // 创建标准索引三角凸包网格。
use bevy::prelude::*; // Mesh与Vec3提供岩石代理构建所需基础类型。 // 岩石代理继续使用可返回错误的封闭凸包入口。

use super::attributes::{
    mesh_float3_attribute, position_bounds, require_triangle_list, smooth_normals,
    tangent_from_normal,
}; // 复用统一Mesh格式校验和派生属性算法，避免两类代理重复实现。

/// 从最低档岩石网格生成封闭凸包代理。
///
/// 【功能设计】现有岩石三份GLB的Mesh2几何完全相同；第四档先均匀采样最低档表面点，再用QuickHull生成
/// 闭合外轮廓，避免简单删三角形产生从任意方向都能看到的破洞。UV和切线只服务超远PBR近似。
pub(in crate::scenes::dynamic_scene::mesh_profiles) fn build_rock_ultra_proxy(
    source: &Mesh,            // 已递归加载完成的最低纹理档岩石Mesh2。
    max_sample_points: usize, // 限制QuickHull输入规模并形成稳定几何上界。
) -> Result<Mesh, String> {
    require_triangle_list(source, "岩石")?; // 当前技术代理只接受三角列表。
    let positions = mesh_float3_attribute(source, Mesh::ATTRIBUTE_POSITION, "岩石位置")?; // 读取完整表面点云。
    let unique_points = unique_finite_points(positions); // 去除重复和异常点，防止凸包退化。
    if unique_points.len() < 4 {
        return Err("最低档岩石网格不足四个唯一有限点，无法生成三维凸包".to_string()); // 三维闭合体至少需要四点。
    } // 完成凸包最低输入保护。
    let sampled_points = farthest_point_sample(&unique_points, max_sample_points.max(4)); // 均匀覆盖岩石外轮廓而非按源顶点顺序截断。
    let parry_points = sampled_points
        .iter() // Parry输入只在本次场景加载期间短暂存在。
        .map(|point| Vector::new(point.x, point.y, point.z)) // Bevy Vec3转换为Parry同精度向量。
        .collect::<Vec<_>>(); // QuickHull需要连续点切片。
    let (hull_points, hull_faces) = try_convex_hull(&parry_points)
        .map_err(|error| format!("最低档岩石动态凸包生成失败：{error:?}"))?; // 资产异常返回可诊断错误而不panic。
    if hull_points.len() < 4 || hull_faces.len() < 4 {
        return Err("最低档岩石动态凸包结果不是有效闭合三维体".to_string()); // 防止退化面进入渲染。
    } // 完成凸包结果保护。
    let proxy_positions = hull_points
        .iter() // Parry输出点已经是采样输入的封闭外壳子集。
        .map(|point| [point.x, point.y, point.z]) // 转回Bevy Float32x3顶点属性。
        .collect::<Vec<_>>(); // 动态代理只保存实际凸包顶点。
    let proxy_center = proxy_positions
        .iter() // 中心只用于检查并修正所有三角形朝外绕序。
        .map(|point| Vec3::from_array(*point)) // 数组转换为Bevy向量。
        .fold(Vec3::ZERO, |sum, point| sum + point)
        / proxy_positions.len() as f32; // 凸包顶点平均值必定位于凸包内部。
    let mut proxy_indices = Vec::with_capacity(hull_faces.len() * 3); // 每个凸包面严格贡献三个U32索引。
    for face in hull_faces {
        let mut oriented = face; // Parry声明CCW，仍执行中心校验抵御坐标约定差异。
        let a = Vec3::from_array(proxy_positions[oriented[0] as usize]); // 取得当前面第一点。
        let b = Vec3::from_array(proxy_positions[oriented[1] as usize]); // 取得当前面第二点。
        let c = Vec3::from_array(proxy_positions[oriented[2] as usize]); // 取得当前面第三点。
        let face_normal = (b - a).cross(c - a); // 原始绕序的面积法线。
        let face_center = (a + b + c) / 3.0; // 面中心到体中心方向判定内外。
        if face_normal.dot(face_center - proxy_center) < 0.0 {
            oriented.swap(1, 2); // 内向面交换后两个顶点恢复外向背面剔除契约。
        } // 完成当前面绕序保护。
        proxy_indices.extend_from_slice(&oriented); // 写入闭合凸包当前三角形。
    } // 完成全部岩石代理面装配。
    let proxy_normals = smooth_normals(&proxy_positions, &proxy_indices); // 面积加权法线保留远景岩石圆润光照。
    let (minimum, extent) = position_bounds(&proxy_positions)?; // 归一化范围用于生成稳定近似UV。
    let proxy_uvs = proxy_positions
        .iter() // 当前超远档不追求源UV逐像素一致，只需避免无UV法线贴图管线失败。
        .map(|position| {
            let point = Vec3::from_array(*position); // 读取当前凸包点。
            let relative = (point - minimum) / extent; // 三轴归一化到零至一。
            [
                point.z.atan2(point.x) / std::f32::consts::TAU + 0.5, // 环绕Y轴提供连续主体纹理方向。
                relative.y.clamp(0.0, 1.0),                           // 高度映射提供稳定第二维。
            ] // 返回当前顶点UV。
        })
        .collect::<Vec<_>>(); // 完成凸包近似UV。
    let proxy_tangents = proxy_normals
        .iter() // StandardMaterial法线贴图需要切线属性。
        .map(|normal| tangent_from_normal(Vec3::from_array(*normal))) // 为每个平滑法线构造稳定正交切线。
        .collect::<Vec<_>>(); // 完成凸包切线。
    let mut proxy = Mesh::new(
        PrimitiveTopology::TriangleList, // 凸包全部面使用标准索引三角列表。
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD, // F3可读且渲染可上传。
    ); // 创建不依赖外部GLB的新会话Mesh。
    proxy.insert_attribute(Mesh::ATTRIBUTE_POSITION, proxy_positions); // 写入闭合凸包位置。
    proxy.insert_attribute(Mesh::ATTRIBUTE_NORMAL, proxy_normals); // 写入面积加权平滑法线。
    proxy.insert_attribute(Mesh::ATTRIBUTE_UV_0, proxy_uvs); // 写入最低岩石材质可用的近似UV。
    proxy.insert_attribute(Mesh::ATTRIBUTE_TANGENT, proxy_tangents); // 写入法线贴图切线空间。
    proxy.insert_indices(Indices::U32(proxy_indices)); // U32避免未来提高代理预算后产生索引格式分支。
    Ok(proxy) // 返回只在当前场景会话登记一次的共享岩石超低模。
} // 完成岩石超低代理生成。

/// 去重最低岩石网格中的有限位置。
fn unique_finite_points(positions: &[[f32; 3]]) -> Vec<Vec3> {
    let mut identities = HashSet::<(u32, u32, u32)>::with_capacity(positions.len()); // 浮点位模式提供确定性精确去重。
    let mut unique = Vec::with_capacity(positions.len()); // 最坏全部顶点唯一。
    for position in positions {
        let point = Vec3::from_array(*position); // 数组转换为Bevy向量。
        if !point.is_finite() {
            continue; // 异常点不允许进入QuickHull，最终不足四点时统一报错。
        } // 完成位置有限性过滤。
        let identity = (point.x.to_bits(), point.y.to_bits(), point.z.to_bits()); // 不引入距离容差和平台差异。
        if identities.insert(identity) {
            unique.push(point); // 首次出现的真实表面点进入采样池。
        } // 完成重复点过滤。
    } // 完成全部源位置扫描。
    unique // 返回保持源顺序的唯一有限点集。
} // 完成岩石点去重。

/// 用三轴归一化最远点采样限制QuickHull输入规模。
fn farthest_point_sample(points: &[Vec3], max_points: usize) -> Vec<Vec3> {
    if points.len() <= max_points {
        return points.to_vec(); // 小网格不需要再采样。
    } // 完成全保留快速路径。
    let arrays = points
        .iter()
        .map(|point| point.to_array())
        .collect::<Vec<_>>(); // 复用统一边界helper。
    let (minimum, extent) = position_bounds(&arrays).unwrap_or((Vec3::ZERO, Vec3::ONE)); // 有限非空输入理论成功。
    let normalized = points
        .iter() // 归一化防止岩石最长轴垄断样本。
        .map(|point| (*point - minimum) / extent)
        .collect::<Vec<_>>(); // 短寿命采样空间。
    let first = normalized
        .iter() // 首点选取离包围盒中心最远的角点。
        .enumerate()
        .max_by(|left, right| {
            left.1
                .distance_squared(Vec3::splat(0.5))
                .total_cmp(&right.1.distance_squared(Vec3::splat(0.5)))
        })
        .map(|(index, _)| index)
        .unwrap_or(0); // 非空输入保证存在。
    let mut selected_flags = vec![false; points.len()]; // 防止重复采样。
    selected_flags[first] = true; // 标记首个外轮廓点。
    let mut selected = vec![first]; // 保存源点索引。
    let mut minimum_distance_squared = normalized
        .iter() // 每点记录到已选集合的最短距离。
        .map(|point| point.distance_squared(normalized[first]))
        .collect::<Vec<_>>(); // 初始化到首点。
    while selected.len() < max_points {
        let next = (0..points.len())
            .filter(|index| !selected_flags[*index]) // 排除已选点。
            .max_by(|left, right| {
                minimum_distance_squared[*left].total_cmp(&minimum_distance_squared[*right])
            })
            .unwrap_or(first); // 未选集合理论非空。
        if minimum_distance_squared[next] <= 1.0e-12 {
            break; // 剩余点在归一化空间重合时提前结束。
        } // 完成重复近点保护。
        selected_flags[next] = true; // 当前最远点进入QuickHull输入。
        selected.push(next); // 保存稳定源索引。
        for (index, point) in normalized.iter().enumerate() {
            minimum_distance_squared[index] =
                minimum_distance_squared[index].min(point.distance_squared(normalized[next])); // 更新到新样本的最近距离。
        } // 完成全部点覆盖半径更新。
    } // 达到固定样本预算后停止。
    selected.sort_unstable(); // 恢复源顺序，保证QuickHull输入跨平台稳定。
    selected.into_iter().map(|index| points[index]).collect() // 返回实际Bevy点集。
} // 完成岩石最远点采样。
