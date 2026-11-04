//! 完整格径向LOD回归：面积、逐位共享边、固定过渡模板及预算，不以调试画线代替几何验收。
use super::{
    geometry::build_chunk_grid,
    planning::{MAX_CHUNKS, plan},
}; // 生产入口。
use bevy::prelude::Vec2; // 玩家中心。
use feathrise_map_content::{MapCollisionLod, sparse_height::SparseHeightSpec}; // 作者格。
use std::collections::BTreeMap; // 真实网格边计数。
fn spec() -> SparseHeightSpec {
    SparseHeightSpec::new([-4096.0; 2], [4096.0; 2], 0.125, -128.0, 256.0).unwrap()
} // 不分配整图高度。
fn lod(d: f32, m: u32) -> MapCollisionLod {
    MapCollisionLod {
        max_distance_meters: d,
        spacing_multiplier: m,
    }
} // 半径与倍率。
fn audit(lods: &[MapCollisionLod], center: Vec2) {
    // 同时检验覆盖和模板。
    let spec = spec(); // 作者规格。
    let planned = plan(&spec, lods, center).unwrap(); // 正式规划。
    let rows = planned.supported_rows.as_ref().unwrap(); // 同版真实覆盖。
    assert!(!planned.chunks.is_empty() && planned.chunks.len() <= MAX_CHUNKS); // 完整有限。
    let mut edges = BTreeMap::<[[u32; 3]; 2], usize>::new(); // 含高度逐位边键。
    let mut area = 0.0; // 世界投影面积。
    for chunk in planned.chunks.values() {
        // 每个物理管理块。
        let cells = chunk.cells.as_ref().unwrap(); // 完整单元列表。
        let expected_faces: usize = cells
            .iter()
            .map(|cell| {
                if cell.finer_edges == 0 {
                    2
                } else {
                    4 + cell.finer_edges.count_ones() as usize
                }
            })
            .sum(); // 普通两三角，过渡格固定中心扇形。
        let mesh = build_chunk_grid(&spec, chunk, |p| {
            Some(((p[0] * 0.03).sin() + (p[1] * 0.02).cos()) as f32)
        })
        .unwrap(); // 非平源检验高度共享。
        assert_eq!(mesh.triangles.len(), expected_faces); // 无额外自由剖分三角形。
        let step = (1_u64 << chunk.key.level) as f64; // 本层格距。
        for p in &mesh.base_grid_points {
            // 每点只可能是格角或过渡半格。
            assert_eq!((p[0] * 2.0 / step).fract(), 0.0); // 无任意圆交点。
            assert_eq!((p[1] * 2.0 / step).fract(), 0.0); // 无自由剖分点。
        } // 模板顶点位置明确。
        for t in &mesh.triangles {
            // 逐面覆盖。
            let [a, b, c] = t.map(|i| mesh.base_grid_points[i as usize]); // 作者精确坐标。
            let twice = (b[1] - a[1]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[1] - a[1]); // 向上绕序。
            assert!(twice > 0.0); // 不允许退化或反面。
            area += twice * 0.5 * 0.125 * 0.125; // 世界面积。
            for i in 0..3 {
                // 全部真实三角边。
                let bits = |index: u32| {
                    let v = mesh.vertices[index as usize];
                    [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
                }; // 高度必须逐位相同。
                let mut edge = [bits(t[i]), bits(t[(i + 1) % 3])]; // 无向边。
                edge.sort(); // 唯一表示。
                *edges.entry(edge).or_default() += 1; // 内部两面共享。
            } // 当前面。
        } // 当前块。
    } // 全部几何。
    let expected: f64 = rows
        .rows
        .values()
        .map(|r| (r[1] - r[0]) as f64 * rows.spacing[0] * rows.spacing[1])
        .sum(); // 最粗完整格union面积。
    assert!((area - expected).abs() < expected * 1e-9); // 无重叠、无缺面积。
    for (edge, count) in edges {
        // 单边只允许出现在真正外轮廓。
        assert!(count == 1 || count == 2); // 无重叠多面。
        if count == 1 {
            // 外边。
            let [a, b] = edge.map(|p| Vec2::new(f32::from_bits(p[0]), f32::from_bits(p[2]))); // 实际世界端点。
            assert!(a.x == b.x || a.y == b.y); // 外缘沿完整格，绝不再做凸包斜切。
            let mid = (a + b) * 0.5; // 边中点。
            let normal = Vec2::new(-(b - a).y, (b - a).x).normalize() * 0.001; // 两侧采样。
            assert_ne!(
                rows.contains(mid + normal),
                rows.contains(mid - normal),
                "内部漏缝 {a:?} {b:?}"
            ); // 内部T接缝不能以总面积掩盖。
        } // 内边已双面共享。
    } // 真实拓扑审计完成。
} // 不运行游戏。

#[test] // 用户1米近层到50米远层不能错误耗尽块预算。
fn small_near_lod_does_not_fragment_large_far_lod() {
    let lods = [lod(1.0, 1), lod(50.0, 4)]; // 实际保存案例。
    let planned = plan(&spec(), &lods, Vec2::new(0.13, 0.27)).unwrap(); // 非格点中心。
    assert!(planned.chunks.len() < 100); // 各层独立管理分组。
    audit(&lods, Vec2::new(0.13, 0.27)); // 拓扑仍正确。
    let same = plan(&spec(), &[lod(1.0, 4), lod(50.0, 4)], Vec2::new(0.13, 0.27)).unwrap(); // 相同倍率不能反查成第一层的小块。
    assert!(same.chunks.len() < 100); // 近远管理粒度仍各自独立。
} // 不靠降低精度。
#[test] // 用户近细远粗及跳倍率都必须固定模板连接。
fn full_cells_and_fixed_transition_seams_are_watertight() {
    for center in [Vec2::ZERO, Vec2::new(0.3, -0.7), Vec2::new(0.49, 0.49)] {
        // 任意格相位。
        audit(&[lod(1.0, 1), lod(2.0, 2)], center); // 最新用户案例。
        audit(&[lod(1.0, 4), lod(4.0, 8)], center); // 原截图案例。
        audit(&[lod(2.0, 1), lod(8.0, 8)], center); // 必要中间过渡级。
        audit(&[lod(1.0, 1), lod(8.0, 32)], center); // 大跨度不能直接长扇形连接。
    } // 非平面逐位接缝。
} // 不使用CDT。
#[test] // 薄层与同倍率不产生重叠格。
fn coincident_grid_edges_and_thin_bands_are_watertight() {
    audit(
        &[lod(1.01, 4), lod(1.02, 4), lod(4.0, 8)],
        Vec2::new(0.13, -0.27),
    ); // 同倍率薄层。
    audit(&[lod(1.0, 4), lod(1.01, 8), lod(4.0, 16)], Vec2::ZERO); // 跳级薄层。
} // 用户列表原样保留。
#[test] // 层数不固定。
fn configurable_layer_counts_fit_budget() {
    for count in [1, 2, 5, 6] {
        // 不设五层上限。
        let layers: Vec<_> = (0..count)
            .map(|i| lod(2.0 * (1_u32 << i) as f32, 1 << i))
            .collect(); // 自定义层数。
        audit(&layers, Vec2::new(7.3, -19.8)); // 正式几何。
    } // 全部面积和接缝检验。
} // 无图形窗口。
#[test] // 参数及预算硬门。
fn invalid_parameters_and_excessive_budget_fail() {
    for layers in [
        vec![],
        vec![lod(f32::NAN, 1)],
        vec![lod(2.0, 3)],
        vec![lod(2.0, 4), lod(4.0, 2)],
        vec![lod(4.0, 1), lod(4.0, 2)],
        vec![lod(1000.0, 1)],
    ] {
        // 非法输入。
        assert!(plan(&spec(), &layers, Vec2::ZERO).is_err()); // 不生成部分候选。
    } // 不偷偷降精度。
    assert!(plan(&spec(), &[lod(16.0, 1)], Vec2::splat(f32::NAN)).is_err()); // 无效中心。
} // 保留旧地面由运行时覆盖。
#[test] // 固定格复用及地图裁边。
fn movement_reuses_interior_and_map_edge_clips() {
    let layers = [lod(16.0, 1), lod(64.0, 4)]; // 常用两层。
    let a = plan(&spec(), &layers, Vec2::ZERO).unwrap(); // 初始。
    let b = plan(&spec(), &layers, Vec2::new(0.1, 0.1)).unwrap(); // 小移动。
    assert!(a.chunks.iter().any(|(key, p)| b.chunks.get(key) == Some(p))); // 内部稳定复用。
    assert!(a.chunks.iter().any(|(key, p)| b.chunks.get(key) != Some(p))); // 边界更新。
    let spec = SparseHeightSpec::new([0.0; 2], [501.0, 503.0], 0.125, -128.0, 256.0).unwrap(); // 不整除地图。
    let tail = plan(&spec, &[lod(3.0, 4), lod(9.0, 16)], Vec2::new(500.0, 502.0)).unwrap(); // 尾角。
    for chunk in tail.chunks.values() {
        // 真实物理网格。
        let mesh = build_chunk_grid(&spec, chunk, |_| Some(0.0)).unwrap(); // 完整格地图裁边。
        assert!(
            mesh.vertices
                .iter()
                .all(|v| v.x >= 0.0 && v.x <= 501.0 && v.z >= 0.0 && v.z <= 503.0)
        ); // 无越界。
        assert!(build_chunk_grid(&spec, chunk, |_| None).is_err()); // 不用零值掩盖缺页。
    } // 尾格完成。
    assert!(
        plan(&spec, &layers, Vec2::splat(9000.0))
            .unwrap()
            .chunks
            .is_empty()
    ); // 图外无地面。
} // 不改作者数据。

#[test] // 可选导出真实碰撞三角边，仅用于离线验收，不修改F1或游戏渲染。
fn export_fixed_transition_preview_when_requested() {
    // 默认测试不写任何文件。
    let Ok(path) = std::env::var("COLLISION_TEST_PREVIEW_PATH") else {
        return;
    }; // 仅显式请求导出。
    let spec = spec(); // 标准作者格。
    let center = Vec2::new(0.13, 0.27); // 非格点玩家中心。
    let planned = plan(&spec, &[lod(1.0, 4), lod(4.0, 8)], center).unwrap(); // 用户最初截图配置。
    let mut image = image::RgbImage::from_pixel(960, 960, image::Rgb([24, 26, 28])); // 诊断图，不伪装游戏截图。
    for chunk in planned.chunks.values() {
        // 使用正式生成的物理面索引。
        let mesh = build_chunk_grid(&spec, chunk, |_| Some(0.0)).unwrap(); // 只移除高度，保持全部拓扑。
        let color = if chunk.key.level == 2 {
            image::Rgb([255, 211, 76])
        } else {
            image::Rgb([93, 184, 241])
        }; // 颜色仅说明采样格距。
        for triangle in &mesh.triangles {
            for edge in 0..3 {
                // 每条真实三角边。
                let point = |index: u32| {
                    let v = mesh.vertices[index as usize];
                    ((Vec2::new(v.x, v.z) - center) * 85.0 + Vec2::splat(480.0)).as_ivec2()
                }; // 俯视世界X/Z。
                let a = point(triangle[edge]); // 起点。
                let b = point(triangle[(edge + 1) % 3]); // 终点。
                let count = (b - a).abs().max_element().max(1); // 像素级有界插值。
                for i in 0..=count {
                    // 只绘制既有边，不添加轮廓。
                    let p = a
                        .as_vec2()
                        .lerp(b.as_vec2(), i as f32 / count as f32)
                        .round()
                        .as_ivec2(); // 最近像素。
                    if p.x >= 0 && p.y >= 0 && p.x < 960 && p.y < 960 {
                        image.put_pixel(p.x as u32, p.y as u32, color);
                    } // 图片边界。
                } // 当前边。
            }
        } // 当前块。
    } // 全部正式碰撞边。
    image.save(path).unwrap(); // 仅用户/测试调用指定的位置。
} // 离线拓扑预览不等于实机视觉验收。
