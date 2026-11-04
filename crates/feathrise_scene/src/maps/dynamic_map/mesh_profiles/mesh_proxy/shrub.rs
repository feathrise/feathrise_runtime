//! 灌木动态超低模共享代理生成。
//! 【功能设计】从最低档源网格确定性选择空间分散的真实叶片三角形，并保留源PBR顶点属性。

use std::collections::HashMap; // 灌木紧凑重索引只在地图加载阶段创建短寿命映射。

use bevy::asset::RenderAssetUsages; // 动态代理同时保留主世界诊断数据和渲染世界上传数据。
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues}; // 读取现有GLB顶点并创建索引三角网格。
use bevy::prelude::*; // Mesh与Vec3提供灌木代理构建所需基础类型。

use super::attributes::{
    mesh_float3_attribute, mesh_triangle_indices, optional_float2_attribute,
    optional_float3_attribute, optional_float4_attribute, position_bounds, require_triangle_list,
    smooth_normals, tangent_from_normal,
}; // 复用统一Mesh格式校验和派生属性算法，避免两类代理重复实现。

/// 一份候选叶片三角形的加载期选择数据。
#[derive(Clone, Copy, Debug)] // 三角形数量很小，按值保存便于确定性筛选。
struct ShrubTriangleCandidate {
    source_indices: [u32; 3],  // 保留原网格索引顺序、UV缝和正反面朝向。
    normalized_centroid: Vec3, // 归一化中心用于从整株各区域均匀选择叶片。
    doubled_area: f32,         // 叉积长度等于两倍面积，首个样本优先保留显著叶面。
} // 完成灌木候选三角形定义。

/// 从最低档灌木网格选择空间分散的真实叶片三角形。
///
/// 【功能设计】植物薄片不能转换成封闭凸包；本函数保留源顶点、法线、UV和切线，只降低叶片三角形数量，
/// 因而继续复用最低档 AlphaMask 纹理，并避免运行时自绘与原资产无关的实心绿色几何。
pub(in crate::maps::dynamic_map::mesh_profiles) fn build_shrub_ultra_proxy(
    source: &Mesh,           // 已递归加载完成的最低档灌木Mesh16。
    target_triangles: usize, // 地图常量锁定的超低档最大三角形数量。
) -> Result<Mesh, String> {
    require_triangle_list(source, "灌木")?; // 只处理glTF当前使用的索引三角列表。
    let positions = mesh_float3_attribute(source, Mesh::ATTRIBUTE_POSITION, "灌木位置")?; // 位置决定空间覆盖与输出顶点。
    let source_indices = mesh_triangle_indices(source, positions.len(), "灌木")?; // 统一U16、U32或非索引输入。
    let (minimum, extent) = position_bounds(positions)?; // 把非均匀模型尺度转换到统一选择空间。
    let candidates = source_indices
        .chunks_exact(3) // glTF primitive按每三个索引形成一片三角形。
        .filter_map(|triangle| {
            let indices = [triangle[0], triangle[1], triangle[2]]; // 冻结当前原始绕序。
            let [a, b, c] = indices.map(|index| positions[index as usize]); // 前置索引校验保证转换安全。
            let a = Vec3::from_array(a); // 转为Bevy向量执行面积和中心计算。
            let b = Vec3::from_array(b); // 转为Bevy向量执行面积和中心计算。
            let c = Vec3::from_array(c); // 转为Bevy向量执行面积和中心计算。
            let doubled_area = (b - a).cross(c - a).length(); // 剔除无法形成叶片轮廓的退化三角形。
            if !doubled_area.is_finite() || doubled_area <= 1.0e-8 {
                return None; // 无面积或异常面不进入动态代理。
            } // 完成三角形有效性保护。
            let centroid = (a + b + c) / 3.0; // 中心用于覆盖整株不同高度和朝向区域。
            Some(ShrubTriangleCandidate {
                source_indices: indices, // 输出继续使用源三角形绕序。
                normalized_centroid: (centroid - minimum) / extent, // 三轴归一化避免最长轴垄断距离。
                doubled_area,                                       // 保存可比较的叶面权重。
            }) // 返回当前有效叶片候选。
        })
        .collect::<Vec<_>>(); // 加载阶段允许为数百个三角形建立短寿命候选表。
    if candidates.is_empty() {
        return Err("最低档灌木网格没有可用于超低代理的有效三角形".to_string()); // 资产契约损坏时拒绝空Mesh。
    } // 完成候选非空保护。
    let selected = select_spread_triangles(&candidates, target_triangles.max(1)); // 以确定性最远点策略保留整株轮廓。
    build_subset_mesh(source, positions, &candidates, &selected) // 重用源顶点属性构建当前会话共享代理。
} // 完成灌木超低代理生成。

/// 用确定性最远点策略选择整株空间分散的叶片三角形。
fn select_spread_triangles(
    candidates: &[ShrubTriangleCandidate], // 全部有效源叶片候选。
    target_triangles: usize,               // 第四档允许的最大三角形数。
) -> Vec<usize> {
    let target = target_triangles.min(candidates.len()).max(1); // 至少保留一个有效叶面且不超过源数量。
    if target == candidates.len() {
        return (0..candidates.len()).collect(); // 小网格无需执行二次筛选。
    } // 完成全保留快速路径。
    let first = candidates
        .iter() // 首个样本保留面积最大的显著叶面。
        .enumerate()
        .max_by(|left, right| left.1.doubled_area.total_cmp(&right.1.doubled_area))
        .map(|(index, _)| index)
        .unwrap_or(0); // 前置非空门保证存在，仍保留确定性回退。
    let mut selected_flags = vec![false; candidates.len()]; // 防止同一三角形重复进入代理。
    selected_flags[first] = true; // 标记首个最大叶面。
    let mut selected = vec![first]; // 输出保存候选表索引而非源顶点索引。
    let mut minimum_distance_squared = candidates
        .iter() // 每个候选保存到已选集合的最短归一化距离。
        .map(|candidate| {
            candidate
                .normalized_centroid
                .distance_squared(candidates[first].normalized_centroid)
        })
        .collect::<Vec<_>>(); // 初始化为到首个叶面的距离。
    while selected.len() < target {
        let next = (0..candidates.len())
            .filter(|index| !selected_flags[*index]) // 排除已选叶片。
            .max_by(|left, right| {
                let left_score =
                    minimum_distance_squared[*left] * (1.0 + candidates[*left].doubled_area.sqrt()); // 空间覆盖优先并轻度偏向显著叶面。
                let right_score = minimum_distance_squared[*right]
                    * (1.0 + candidates[*right].doubled_area.sqrt()); // 使用相同稳定评分。
                left_score.total_cmp(&right_score)
            })
            .unwrap_or(first); // 未选集合理论非空，回退值只防御未来逻辑变化。
        selected_flags[next] = true; // 当前最分散叶面进入代理。
        selected.push(next); // 保存本轮选择结果。
        for (index, candidate) in candidates.iter().enumerate() {
            let distance_squared = candidate
                .normalized_centroid
                .distance_squared(candidates[next].normalized_centroid); // 计算到新样本距离。
            minimum_distance_squared[index] = minimum_distance_squared[index].min(distance_squared); // 更新到完整已选集合的最近距离。
        } // 完成所有候选覆盖距离更新。
    } // 达到固定三角预算后停止。
    selected.sort_unstable(); // 恢复源三角顺序，使输出索引和视觉结果跨平台稳定。
    selected // 返回被选候选表索引。
} // 完成灌木叶片选择。

/// 用源顶点属性和被选三角形构建紧凑重索引网格。
fn build_subset_mesh(
    source: &Mesh,                         // 读取最低档现有法线、UV与切线。
    positions: &[[f32; 3]],                // 已校验位置属性。
    candidates: &[ShrubTriangleCandidate], // 候选保存原始绕序。
    selected: &[usize],                    // 只保留空间分散叶面。
) -> Result<Mesh, String> {
    let normals = optional_float3_attribute(source, Mesh::ATTRIBUTE_NORMAL, "灌木法线")?; // 优先保持源光照方向。
    let uvs = optional_float2_attribute(source, Mesh::ATTRIBUTE_UV_0, "灌木UV0")?; // AlphaMask必须继续读取源纹理区域。
    let tangents = optional_float4_attribute(source, Mesh::ATTRIBUTE_TANGENT, "灌木切线")?; // 法线贴图优先保持源切线空间。
    let mut remap = HashMap::<u32, u32>::with_capacity(selected.len() * 3); // 只为实际使用的源顶点建立新编号。
    let mut source_vertex_order = Vec::<usize>::with_capacity(selected.len() * 3); // 输出属性全部按首次使用顺序复制。
    let mut proxy_indices = Vec::<u32>::with_capacity(selected.len() * 3); // 被选每面贡献三个紧凑索引。
    for selected_index in selected {
        for source_index in candidates[*selected_index].source_indices {
            let next_index = u32::try_from(source_vertex_order.len())
                .map_err(|_| "灌木超低代理顶点数超过U32范围".to_string())?; // 理论仅百余顶点，仍保留显式保护。
            let proxy_index = *remap.entry(source_index).or_insert_with(|| {
                source_vertex_order.push(source_index as usize); // 首次使用时登记源顶点复制顺序。
                next_index // 返回当前紧凑编号。
            }); // 复用相邻三角共享顶点和UV缝。
            proxy_indices.push(proxy_index); // 保持源三角形绕序。
        } // 完成当前三角形三个顶点。
    } // 完成全部被选叶片重索引。
    let proxy_positions = source_vertex_order
        .iter() // 按紧凑编号复制位置。
        .map(|index| positions[*index])
        .collect::<Vec<_>>(); // 输出位置长度决定代理顶点数。
    let proxy_normals = normals.map(|values| {
        source_vertex_order
            .iter() // 保持源每顶点法线。
            .map(|index| values[*index])
            .collect::<Vec<_>>()
    }); // 完成可选法线复制。
    let proxy_uvs = uvs.map(|values| {
        source_vertex_order
            .iter() // 保持源AlphaMask UV。
            .map(|index| values[*index])
            .collect::<Vec<_>>()
    }); // 完成可选UV复制。
    let proxy_tangents = tangents.map(|values| {
        source_vertex_order
            .iter() // 保持源法线贴图切线。
            .map(|index| values[*index])
            .collect::<Vec<_>>()
    }); // 完成可选切线复制。
    let mut proxy = Mesh::new(
        PrimitiveTopology::TriangleList, // 被选源叶片仍为标准索引三角列表。
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD, // F3与渲染共同访问。
    ); // 创建当前地图会话共享灌木代理。
    proxy.insert_attribute(Mesh::ATTRIBUTE_POSITION, proxy_positions); // 写入必需位置属性。
    if let Some(normals) = proxy_normals {
        proxy.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals); // 源法线存在时原样复用。
    } else {
        let generated = flat_vertex_normals(&source_vertex_order, positions, &proxy_indices); // 极端缺法线资产生成安全替代。
        proxy.insert_attribute(Mesh::ATTRIBUTE_NORMAL, generated); // StandardMaterial始终取得合法法线。
    } // 完成灌木法线分支。
    if let Some(uvs) = proxy_uvs {
        proxy.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs); // 保留最低档现有AlphaMask映射。
    } // 无UV源材质只能按无纹理路径运行，不伪造图集映射。
    if let Some(tangents) = proxy_tangents {
        proxy.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents); // 保留最低档现有切线。
    } else if let Some(VertexAttributeValues::Float32x3(normals)) =
        proxy.attribute(Mesh::ATTRIBUTE_NORMAL)
    {
        let generated = normals
            .iter() // 缺切线时从已安装法线生成稳定正交方向。
            .map(|normal| tangent_from_normal(Vec3::from_array(*normal)))
            .collect::<Vec<_>>();
        proxy.insert_attribute(Mesh::ATTRIBUTE_TANGENT, generated); // 保证法线贴图材质管线存在切线属性。
    } // 完成灌木切线分支。
    proxy.insert_indices(Indices::U32(proxy_indices)); // 当前代理统一使用U32紧凑索引。
    Ok(proxy) // 返回只登记一次且供全部灌木共享的第四档Mesh。
} // 完成灌木子集网格构建。

/// 为缺少源法线的灌木代理生成按面累加的顶点法线。
fn flat_vertex_normals(
    source_vertex_order: &[usize], // 紧凑顶点对应的源位置编号。
    source_positions: &[[f32; 3]], // 原始位置属性。
    proxy_indices: &[u32],         // 已重索引三角列表。
) -> Vec<[f32; 3]> {
    let positions = source_vertex_order
        .iter() // 先取得紧凑位置便于复用通用平滑法线实现。
        .map(|index| source_positions[*index])
        .collect::<Vec<_>>(); // 仅加载阶段分配百余个数组。
    smooth_normals(&positions, proxy_indices) // 面积加权结果比固定朝向更适合分散叶片。
} // 完成灌木安全法线生成。
