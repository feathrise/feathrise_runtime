//! 静水透明边与样条水实际三角互动足迹回归。

use super::*;
use bevy::prelude::Vec3; // 样条足迹拆分后测试显式导入三维顶点类型。

#[test]
fn ribbon_contact_uses_every_actual_lateral_mesh_triangle() {
    let footprint = RibbonWaterFootprint::from_ribbon_vertices(
        vec![
            Vec3::new(0.0, 0.0, 0.0), // 上游第一侧顶点。
            Vec3::new(0.0, 0.0, 1.0), // 上游中间横向顶点。
            Vec3::new(0.0, 0.0, 2.0), // 上游第二侧顶点。
            Vec3::new(1.0, 0.0, 0.0), // 下游第一侧顶点。
            Vec3::new(1.0, 4.0, 1.0), // 下游中间顶点故意抬高，证明不能以两岸大四边形近似。
            Vec3::new(1.0, 0.0, 2.0), // 下游第二侧顶点。
        ],
        vec![2.0, 4.0], // 上下游截面使用可辨别的作者深度。
        3,              // 每个截面两个真实横向格。
    )
    .expect("合法横向细分带状足迹必须完成构建");
    let sample = footprint
        .sample(Vec2::new(2.0 / 3.0, 1.0 / 3.0))
        .expect("第一横向格首片三角形重心必须命中");

    assert!((sample.surface_local_y - 4.0 / 3.0).abs() < 0.0001); // 实际中间顶点Y必须参与同源三角插值。
    assert!((sample.author_depth - 10.0 / 3.0).abs() < 0.0001); // 深度采用同一片小三角的截面权重。
}

#[test]
fn painted_water_contact_follows_visible_triangle_alpha() {
    let footprint = PaintedWaterFootprint::from_visible_mesh(
        [2, 1],                             // 两个横向单元可以同时覆盖可见浅滩和透明保护边。
        [0.0, 0.0],                         // 测试网格局部最小端点。
        [2.0, 1.0],                         // 测试网格局部最大端点。
        vec![1.0, 0.5, 0.0, 1.0, 0.5, 0.0], // 左侧有水，右边界按实际Mesh透明度归零。
    );
    assert!(painted_water_footprint_contains(
        &footprint,
        Vec2::new(0.5, 0.5),
    )); // 不透明核心必须属于玩法水面。
    assert!(painted_water_footprint_contains(
        &footprint,
        Vec2::new(1.5, 0.5),
    )); // 透明度尚未归零的浅滩三角同样必须允许互动。
    assert!(!painted_water_footprint_contains(
        &footprint,
        Vec2::new(2.0, 0.5),
    )); // 完全透明的保护边不得授权波源。
    assert!(!painted_water_footprint_contains(
        &footprint,
        Vec2::new(-0.1, 0.5),
    )); // 活跃网格范围外的低地同样不得授权波源。
}

#[test]
fn painted_water_contact_uses_the_same_mesh_diagonal() {
    let footprint = PaintedWaterFootprint::from_visible_mesh(
        [1, 1],                   // 单个网格单元正好包含Mesh使用的两片三角形。
        [0.0, 0.0],               // 测试单元局部最小端点。
        [1.0, 1.0],               // 测试单元局部最大端点。
        vec![0.0, 1.0, 0.0, 0.0], // 只有右上顶点可见，因此只有右上三角拥有非零插值。
    );
    assert!(painted_water_footprint_contains(
        &footprint,
        Vec2::new(0.75, 0.25),
    )); // Mesh右上三角内的位置必须命中可见水面。
    assert!(!painted_water_footprint_contains(
        &footprint,
        Vec2::new(0.25, 0.75),
    )); // 左下透明三角不能被同单元另一角的Alpha错误授权。
}
