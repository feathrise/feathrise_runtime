//! 动态云远景载体：低模二十面球只输出世界位置，云形完全不依赖球面 UV。

use bevy::prelude::{Mesh, Meshable, Sphere}; // 使用 Bevy 稳定球体生成器建立相机内侧远景几何。

/// 相机内侧动态云载体半径。
pub(super) const CLOUD_DOME_RADIUS: f32 = 750.0; // 保持在项目常用千米远裁面内，并完整覆盖透视视锥。

/// 构造无纹理坐标依赖的低模远景球。
pub(super) fn build_cloud_dome_mesh() -> Mesh {
    let mut mesh = Sphere::new(CLOUD_DOME_RADIUS) // 固定米制半径让透明云始终位于普通场景几何之后。
        .mesh() // 取得可选择二十面球细分级别的网格构造器。
        .ico(4) // 四级细分足以承载平滑视线插值，且不会产生经纬极点三角扇。
        .expect("动态云远景球细分级别必须有效"); // 固定合法级别若因上游 API 漂移失败则明确中断。
    mesh.remove_attribute(Mesh::ATTRIBUTE_UV_0); // Bevy 二十面球默认生成经纬 UV；显式删除后云形只能来自世界视线投影。
    mesh // 返回只在首次可见云层时登记一次的共享网格。
}

#[cfg(test)]
mod tests {
    use bevy::mesh::VertexAttributeValues; // 读取实际顶点位置验证固定半径。
    use bevy::prelude::Mesh; // 检查运行时网格没有球面 UV 契约。

    use super::*; // 直接测试正式网格构造入口与固定半径。

    #[test]
    fn cloud_dome_has_no_spherical_uv_dependency() {
        let mesh = build_cloud_dome_mesh(); // 构造运行时使用的同一份二十面球。
        assert!(mesh.attribute(Mesh::ATTRIBUTE_UV_0).is_none()); // 禁止重新引入极点或经线纹理坐标。
        let positions = match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
            Some(VertexAttributeValues::Float32x3(values)) => values, // 二十面球位置使用标准三分量格式。
            _ => panic!("动态云远景球缺少 Float32x3 位置"), // Shader 必须能够由世界位置重建视线。
        };
        assert!(positions.iter().all(|position| {
            let length =
                (position[0] * position[0] + position[1] * position[1] + position[2] * position[2])
                    .sqrt(); // 每个顶点都应位于同一固定半径球面上。
            (length - CLOUD_DOME_RADIUS).abs() < 0.01 // 仅容忍生成器浮点归一化舍入误差。
        }));
    }
}
