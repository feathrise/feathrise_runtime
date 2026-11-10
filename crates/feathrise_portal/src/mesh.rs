//! 传送点共享程序网格；边缘直光幕在启动阶段生成并由正式运行时与F12作者预览共同复用。

use bevy::asset::RenderAssetUsages; // 程序网格同时保留主世界测试数据并上传渲染世界。
use bevy::mesh::{Indices, PrimitiveTopology}; // 无顶盖环形筒壁使用显式三角索引。
use bevy::prelude::*; // Mesh与Vec2等几何基础类型。

const EDGE_BEAM_SEGMENTS: u32 = 32; // 固定三十二段兼顾圆形轮廓与透明面片数量。
const EDGE_BEAM_RINGS: [(f32, f32); 4] = [
    (0.0, 1.0),   // 地面边缘保持完整亮度，确保高草缝隙仍能定位范围。
    (0.58, 1.0),  // 下方约六成高度维持直光，不让整根从地面开始变淡。
    (0.82, 0.38), // 上段增加中间采样，使淡出不是从底到顶的单一线性透明。
    (1.0, 0.0),   // 顶部透明度归零，消除突兀的水平截断边。
];

/// 构造固定半径、中心留空且顶部渐隐的无盖环形直光幕。
pub fn build_portal_edge_beam_mesh(radius: f32, height: f32) -> Mesh {
    assert!(radius.is_finite() && radius > 0.0); // 公共调用不得产生中心退化或非有限顶点。
    assert!(height.is_finite() && height > 0.0); // 高度必须能形成从地面向上的有效光幕。

    let vertices_per_ring = EDGE_BEAM_SEGMENTS + 1; // 复制首尾点形成连续UV接缝。
    let vertex_capacity = vertices_per_ring as usize * EDGE_BEAM_RINGS.len(); // 四圈顶点均为固定小预算。
    let index_capacity = EDGE_BEAM_SEGMENTS as usize * (EDGE_BEAM_RINGS.len() - 1) * 6; // 每个竖向格写两个三角形。
    let mut positions = Vec::with_capacity(vertex_capacity); // 局部坐标以原点为高度中心。
    let mut normals = Vec::with_capacity(vertex_capacity); // 法线沿固定半径向外，双面材质负责内视角。
    let mut uvs = Vec::with_capacity(vertex_capacity); // UV保留环向与高度比例，便于未来材质扩展。
    let mut colors = Vec::with_capacity(vertex_capacity); // 顶点色Alpha承载下实上虚的确定性渐隐。
    let mut indices = Vec::with_capacity(index_capacity); // 索引只连接相邻高度圈，不生成上下盖。

    for (height_fraction, alpha) in EDGE_BEAM_RINGS {
        // 固定四圈建立下段直光和上段渐隐。
        let y = (height_fraction - 0.5) * height; // Mesh居中后由实体平移半高使底端落地。
        for segment in 0..=EDGE_BEAM_SEGMENTS {
            // 包含重复接缝点以避免UV跨一回零插值。
            let u = segment as f32 / EDGE_BEAM_SEGMENTS as f32; // 环向归一化坐标。
            let angle = u * std::f32::consts::TAU; // 完整一周保持半径不随高度改变。
            let (sin, cos) = angle.sin_cos(); // 一次三角函数同时得到XZ方向。
            let x = cos * radius; // X始终位于指定传送边缘半径。
            let z = sin * radius; // Z与X共同形成中心留空的圆周。
            positions.push([x, y, z]); // 仅写筒壁顶点，不写中心或盖面顶点。
            normals.push([cos, 0.0, sin]); // 外法线保持竖直筒壁朝向一致。
            uvs.push([u, height_fraction]); // 高度渐变身份同时保留在UV纵轴。
            colors.push([1.0, 1.0, 1.0, alpha]); // 白色不改变作者色，只用Alpha调制叠加能量。
        }
    }

    for ring in 0..(EDGE_BEAM_RINGS.len() as u32 - 1) {
        // 只连接相邻圈，顶部和底部继续保持无盖。
        let lower_start = ring * vertices_per_ring; // 当前高度带下圈首顶点。
        let upper_start = (ring + 1) * vertices_per_ring; // 当前高度带上圈首顶点。
        for segment in 0..EDGE_BEAM_SEGMENTS {
            // 每个圆周段形成一个竖直四边形。
            let lower = lower_start + segment; // 四边形左下角。
            let lower_next = lower + 1; // 四边形右下角。
            let upper = upper_start + segment; // 四边形左上角。
            let upper_next = upper + 1; // 四边形右上角。
            indices.extend_from_slice(&[lower, lower_next, upper_next]); // 第一三角形连接右上角。
            indices.extend_from_slice(&[lower, upper_next, upper]); // 第二三角形闭合当前竖向格。
        }
    }

    let mut mesh = Mesh::new(
        // 创建只包含三角面的共享程序网格。
        PrimitiveTopology::TriangleList, // StandardMaterial按普通三角列表渲染筒壁。
        RenderAssetUsages::default(),    // 测试读取顶点后仍可上传渲染世界。
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions); // 位置证明固定半径与完整高度。
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals); // 虽使用unlit材质，仍保持标准PBR网格完整性。
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs); // 为标准材质管线提供连续UV布局。
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors); // StandardMaterial把顶点Alpha乘入最终透明度。
    mesh.insert_indices(Indices::U32(indices)); // 四圈三带均使用明确U32索引。
    mesh // 返回后由启动资源登记一次并跨场景复用。
}

#[cfg(test)]
mod tests {
    use bevy::mesh::VertexAttributeValues; // Bevy 0.19只为Float32x3提供快捷访问，颜色需匹配枚举。

    use super::*; // 直接验证提交给正式与作者预览的同一共享构造器。

    #[test]
    fn edge_beam_stays_on_perimeter_and_fades_only_near_the_top() {
        let radius = 2.5; // 使用正式传送范围半径验证中心留空。
        let height = 14.0; // 使用提高后的正式光幕高度验证上下边界。
        let mesh = build_portal_edge_beam_mesh(radius, height); // 构造最终真实程序网格。
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION) // 读取每一圈局部空间位置。
            .and_then(|values| values.as_float3()) // 构造器必须提供标准Float32x3布局。
            .expect("边缘直光幕必须提供Float32x3位置顶点");
        let Some(VertexAttributeValues::Float32x4(colors)) = mesh.attribute(Mesh::ATTRIBUTE_COLOR)
        else {
            panic!("边缘直光幕必须提供Float32x4顶点颜色"); // 每个顶点都需要独立Alpha。
        };
        assert_eq!(positions.len(), colors.len()); // 位置和渐隐颜色必须逐顶点一一对应。

        let mut min_y = f32::INFINITY; // 收集局部底端高度。
        let mut max_y = f32::NEG_INFINITY; // 收集局部顶端高度。
        let mut top_alpha = 1.0_f32; // 顶圈最终必须完全透明。
        for ([x, y, z], color) in positions.iter().copied().zip(colors.iter()) {
            // 联合验证半径、高度和Alpha。
            let vertex_radius = Vec2::new(x, z).length(); // 中心留空要求全部顶点位于同一外圈。
            assert!((vertex_radius - radius).abs() < 0.0001); // 任一高度都不得向内成光芯或向外扩散。
            min_y = min_y.min(y); // 更新局部最低点。
            max_y = max_y.max(y); // 更新局部最高点。
            if (y - height * 0.5).abs() < 0.0001 {
                // 只聚合最顶部圆环的透明度。
                top_alpha = top_alpha.min(color[3]); // 所有顶圈顶点都应透明归零。
            }
        }

        assert!((min_y + height * 0.5).abs() < 0.0001); // 实体平移半高后底端正好落地。
        assert!((max_y - height * 0.5).abs() < 0.0001); // 提高后的顶端完整达到十四米。
        assert_eq!(top_alpha, 0.0); // 顶端没有水平硬截断亮边。
        let solid_ring_start = (EDGE_BEAM_SEGMENTS + 1) as usize; // 第二圈对应58%高度的淡出起点。
        assert_eq!(colors[solid_ring_start][3], 1.0); // 下方六成必须保持完整亮度后才开始淡出。
    }
}
