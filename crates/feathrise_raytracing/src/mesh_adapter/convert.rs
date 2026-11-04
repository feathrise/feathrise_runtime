//! 不访问ECS、Assets或GPU的纯CPU Solari Mesh转换。

use bevy::{
    asset::RenderAssetUsages, // 派生Mesh需要同时保留主世界和渲染世界数据。
    mesh::{Indices, PrimitiveTopology}, // Solari严格要求TriangleList与U32索引。
    prelude::*,               // Mesh与Vec3。
};

use super::{AdaptedMeshOutput, CapturedMeshInput}; // 后台线程输入与未登记输出。

/// 把完整捕获快照转换为只含 Position/Normal/UV0/Tangent 的 Solari 兼容 Mesh。
pub(super) fn adapt_captured_mesh(input: CapturedMeshInput) -> Result<AdaptedMeshOutput, String> {
    let vertex_count = input.positions.len(); // 所有顶点属性必须一一对应。
    if vertex_count == 0 {
        return Err("位置数组为空".to_string()); // 空BLAS没有P1意义且可能触发底层验证错误。
    }
    if input.normals.len() != vertex_count {
        return Err(format!(
            "法线数量{}与顶点数量{}不一致",
            input.normals.len(),
            vertex_count
        ));
    }
    if input.indices.is_empty() || !input.indices.len().is_multiple_of(3) {
        return Err(format!("索引数量{}不是非零三角列表", input.indices.len()));
    }
    if input
        .indices
        .iter()
        .any(|index| *index as usize >= vertex_count)
    {
        return Err("索引引用了顶点数组范围外元素".to_string());
    }
    if input
        .positions
        .iter()
        .flatten()
        .chain(input.normals.iter().flatten())
        .any(|value| !value.is_finite())
    {
        return Err("位置或法线包含NaN/Inf".to_string()); // 非有限数据不得进入BLAS构建。
    }

    let has_source_uv = input.uvs.is_some(); // 只有地形等真实UV路径调用MikkTSpace切线生成。
    let uvs = input.uvs.unwrap_or_else(|| vec![[0.0, 0.0]; vertex_count]); // 无UV村落与村路按计划填充零UV。
    if uvs.len() != vertex_count {
        return Err(format!(
            "UV数量{}与顶点数量{}不一致",
            uvs.len(),
            vertex_count
        ));
    }
    if uvs.iter().flatten().any(|value| !value.is_finite()) {
        return Err("UV包含NaN/Inf".to_string()); // 切线生成不得接收非有限纹理坐标。
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList, // Solari场景提取只支持三角形列表。
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD, // 回退清理前主世界仍需审计属性。
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, input.positions); // 第一项固定属性。
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, input.normals.clone()); // 第二项固定属性。
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs); // 第三项固定属性，始终存在。
    mesh.insert_indices(Indices::U32(input.indices)); // 索引统一为U32。

    if has_source_uv {
        mesh.generate_tangents()
            .map_err(|error| format!("原UV切线生成失败：{error:?}"))?; // 地形失败时整体阻断，不静默换零切线。
    } else {
        let tangents = input
            .normals
            .iter()
            .map(|normal| finite_orthogonal_tangent(Vec3::from_array(*normal)))
            .collect::<Result<Vec<_>, _>>()?; // 无UV内容使用有限正交基，不使用全零四维切线。
        mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents); // 第四项固定属性。
    }
    mesh.enable_raytracing = true; // 明确要求Bevy为派生Mesh建立BLAS。

    let attribute_count = mesh.attributes().count(); // 防止未来修改意外把额外顶点属性带入Solari。
    if attribute_count != 4
        || !mesh.contains_attribute(Mesh::ATTRIBUTE_POSITION)
        || !mesh.contains_attribute(Mesh::ATTRIBUTE_NORMAL)
        || !mesh.contains_attribute(Mesh::ATTRIBUTE_UV_0)
        || !mesh.contains_attribute(Mesh::ATTRIBUTE_TANGENT)
    {
        return Err(format!("派生Mesh属性契约错误：实际{attribute_count}项"));
    }

    let derived_bytes = vertex_count as u64 * (12 + 12 + 8 + 16)
        + mesh.indices().map_or(0, |indices| indices.len() as u64 * 4); // 四属性加U32索引的可审计载荷。
    Ok(AdaptedMeshOutput {
        identity: input.identity, // 原样交回任务启动身份。
        mesh,                     // 未登记Mesh由主线程按每帧一个采用。
        derived_bytes,            // 供20MiB单项上传门核对。
    })
}

/// 根据有限法线构造稳定、有限且与法线正交的四维切线。
fn finite_orthogonal_tangent(normal: Vec3) -> Result<[f32; 4], String> {
    let length_squared = normal.length_squared(); // 避免normalize零向量产生NaN。
    if !length_squared.is_finite() || length_squared <= 1.0e-12 {
        return Err("无UV网格包含零长度或非有限法线".to_string());
    }
    let normal = normal / length_squared.sqrt(); // 只对受检有限正值开方。
    let helper = if normal.y.abs() < 0.999 {
        Vec3::Y // 大多数法线与Y叉乘即可获得稳定切向方向。
    } else {
        Vec3::X // 接近竖直时换X轴避免叉积退化。
    };
    let tangent = normal.cross(helper).normalize(); // helper选择保证叉积具有有限非零长度。
    if !tangent.is_finite() {
        return Err("无UV正交切线生成了非有限值".to_string());
    }
    Ok([tangent.x, tangent.y, tangent.z, 1.0]) // handedness固定为正，零UV不承载实际法线贴图方向。
}

#[cfg(test)]
mod tests {
    use bevy::asset::AssetId; // 构造不依赖Assets的任务身份。

    use super::*; // 直接测试正式纯CPU转换函数。
    use crate::types::AdaptationIdentity;

    fn identity() -> AdaptationIdentity {
        AdaptationIdentity {
            generation: 2,
            root: Entity::PLACEHOLDER,
            source_entity: Entity::PLACEHOLDER,
            source_mesh: AssetId::default(),
            source_revision: 3,
            ticket: 4,
            contract_index: 0,
        }
    }

    fn triangle_input(uvs: Option<Vec<[f32; 2]>>) -> CapturedMeshInput {
        CapturedMeshInput {
            identity: identity(),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            normals: vec![[0.0, 1.0, 0.0]; 3],
            uvs,
            indices: vec![0, 1, 2],
        }
    }

    #[test]
    fn uv_mesh_converts_to_exact_solari_format() {
        let output = adapt_captured_mesh(triangle_input(Some(vec![
            [0.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0],
        ])))
        .expect("合法UV三角形应生成切线");
        assert_eq!(
            output.mesh.primitive_topology(),
            PrimitiveTopology::TriangleList
        );
        assert!(matches!(output.mesh.indices(), Some(Indices::U32(_))));
        assert_eq!(output.mesh.attributes().count(), 4);
        assert!(output.mesh.enable_raytracing);
    }

    #[test]
    fn mesh_without_uv_gets_zero_uv_and_finite_orthogonal_tangents() {
        let output = adapt_captured_mesh(triangle_input(None)).expect("无UV村落应使用有限正交切线");
        let tangents = output
            .mesh
            .attribute(Mesh::ATTRIBUTE_TANGENT)
            .and_then(|values| match values {
                bevy::mesh::VertexAttributeValues::Float32x4(values) => Some(values.as_slice()),
                _ => None,
            })
            .expect("派生Mesh必须含Float32x4切线");
        assert!(tangents.iter().flatten().all(|value| value.is_finite()));
        assert!(tangents.iter().all(|tangent| tangent[..3] != [0.0; 3]));
    }

    #[test]
    fn malformed_uv_input_fails_before_tangent_generation() {
        let Err(error) = adapt_captured_mesh(triangle_input(Some(vec![[0.0, 0.0]]))) else {
            panic!("UV数量不匹配必须整体失败");
        };
        assert!(error.contains("UV数量"));
    }

    #[test]
    fn invalid_normal_cannot_generate_non_finite_fallback_tangent() {
        let mut input = triangle_input(None);
        input.normals[0] = [0.0, 0.0, 0.0];
        let Err(error) = adapt_captured_mesh(input) else {
            panic!("零法线必须阻断而不是生成NaN");
        };
        assert!(error.contains("零长度"));
    }
}
