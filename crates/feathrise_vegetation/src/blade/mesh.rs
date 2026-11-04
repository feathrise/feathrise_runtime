//! 通用叶片网格准备工具，不包含任何宿主项目资产路径或几何预算。

use bevy::{
    mesh::VertexAttributeValues, // 读取标准三维位置属性并按纵向范围派生根尖 UV。
    prelude::Mesh,               // 修改消费者已经加载或程序生成的叶片网格资产。
};

/// 为缺少 UV0 的叶片网格按真实纵向范围生成统一根尖坐标。
///
/// 【功能设计】已有 UV0 时原样保留；只有旧叶片 primitive 缺少 UV0 时才生成
/// `U=0.5`、`V=1根/0尖`的兼容坐标，公共运行时不推断任何草种或资产来源。
pub fn ensure_vertical_blade_uv(mesh: &mut Mesh) -> Result<bool, String> {
    if mesh.contains_attribute(Mesh::ATTRIBUTE_UV_0) {
        return Ok(false); // 消费者提供的正式 UV 始终优先，通用工具不得覆盖其图集映射。
    }
    let positions = match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(values)) => values, // glTF 和程序网格的标准位置属性可直接读取。
        Some(_) => return Err("叶片POSITION不是Float32x3，无法派生根尖UV".to_string()), // 非标准位置格式无法安全解释。
        None => return Err("叶片网格缺少POSITION，无法派生根尖UV".to_string()), // 没有位置时不存在可计算的纵向范围。
    };
    let (minimum_y, maximum_y) = positions.iter().fold(
        (f32::INFINITY, f32::NEG_INFINITY), // 从空范围开始聚合全部有限纵向坐标。
        |(minimum, maximum), position| {
            if position[1].is_finite() {
                (minimum.min(position[1]), maximum.max(position[1])) // 合法顶点扩展叶片纵向范围。
            } else {
                (minimum, maximum) // 单个异常顶点不得把整个纵向范围污染为 NaN。
            }
        },
    );
    let height = maximum_y - minimum_y; // 根尖 UV 只需要叶片的有限纵向跨度。
    if !height.is_finite() || height <= 0.000_1 {
        return Err("叶片网格纵向范围无效，无法派生根尖UV".to_string()); // 平面或异常范围不能稳定归一化。
    }
    let uvs = positions
        .iter() // 为每个位置生成与顶点数量严格一致的 UV0。
        .map(|position| {
            let vertical = ((position[1] - minimum_y) / height).clamp(0.0, 1.0); // 把最低点到最高点映射为零至一。
            [0.5_f32, 1.0 - vertical] // 绘制 shader 沿用 V=1 根、V=0 尖的叶片契约。
        })
        .collect::<Vec<_>>(); // Bevy 接受与位置数组等长的 Float32x2 顶点属性。
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs); // 只补入缺失的 UV0，不修改位置、索引或法线。
    Ok(true) // 返回 true 让调用方知道本次确实修改了网格资产。
}

#[cfg(test)] // 通用工具的行为不依赖 GPU，可用最小程序网格锁定。
mod tests {
    use bevy::{
        asset::RenderAssetUsages, // 构造保留主世界和渲染世界用途的测试网格。
        mesh::PrimitiveTopology,  // 叶片测试只需三角列表拓扑。
    };

    use super::*; // 直接验证公开 UV 补齐入口。

    #[test]
    fn vertical_uv_generation_is_idempotent() {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList, // 单个三角叶片足以覆盖根尖映射。
            RenderAssetUsages::default(),    // 测试不依赖实际 GPU 上传。
        );
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION, // 提供最低根部和最高叶尖的标准位置属性。
            vec![[0.0_f32, 0.0, 0.0], [0.1, 1.0, 0.0], [-0.1, 0.0, 0.0]], // 三个有限顶点形成一米高叶片。
        );
        assert_eq!(ensure_vertical_blade_uv(&mut mesh), Ok(true)); // 首次调用必须补齐 UV0。
        assert_eq!(ensure_vertical_blade_uv(&mut mesh), Ok(false)); // 第二次调用必须保留已有 UV0。
    }
}
