//! 布局源字段采样、所有权口径与异步共享快照回归。

use std::sync::Arc; // 核对后台快照只共享大数组与解析块。

use bevy::prelude::{IVec2, Rect, Vec2}; // 构造端点式字段与固定测试块。

use crate::VegetationClaimSampling; // 锁定点采样与源单元所有权采样的正交差异。

use super::*; // 私有最近采样与精度函数需要锁定GPU对等口径。

#[test]
fn async_snapshots_share_source_pixels_and_resolved_tiles() {
    let bounds = Rect::from_corners(Vec2::ZERO, Vec2::splat(64.0)); // 使用一块正式世界范围。
    let source = VegetationSourceField::new(bounds, 65, 65, vec![0.5; 65 * 65])
        .expect("shared source fixture should be valid"); // 创建足以暴露深拷贝的大数组。
    let cloned_source = source.clone(); // 后台输入快照会执行相同Clone。
    assert!(Arc::ptr_eq(&source.values, &cloned_source.values)); // Clone只能增加强引用，不能复制源像素。
    let tile_id = VegetationDirtyTile(IVec2::ZERO); // 固定活动块身份。
    let tile = Arc::new(VegetationResolvedTile {
        coordinate: tile_id,  // 保存块坐标。
        world_bounds: bounds, // 保存同一世界范围。
        layers: vec![VegetationResolvedLayer {
            layer: VegetationLayerId(1),                     // 任意稳定层ID。
            density: vec![1.0; VEGETATION_TILE_TEXEL_COUNT], // 模拟正式4096项解析密度。
        }],
    });
    let mut layout = ActiveVegetationLayout::default(); // 构造活动布局共享容器。
    layout.tiles.insert(tile_id, Arc::clone(&tile)); // 活动布局取得解析块强引用。
    let cloned_layout = layout.clone(); // GPU候选后台快照会执行相同Clone。
    assert!(Arc::ptr_eq(
        layout
            .tiles
            .get(&tile_id)
            .expect("original tile must exist"),
        cloned_layout
            .tiles
            .get(&tile_id)
            .expect("cloned tile must exist"),
    )); // Clone不能复制解析块中的完整多层密度。
}

#[test]
fn source_field_matches_gpu_endpoint_round_sampling() {
    let bounds = Rect::from_corners(Vec2::new(-10.0, -1.0), Vec2::new(10.0, 1.0));
    let field = VegetationSourceField::new(
        bounds,
        3,
        1,
        vec![0.0, 0.5, 1.0], // 世界-10、0、10分别对应三个端点样本。
    )
    .unwrap();
    assert_eq!(field.sample(Vec2::new(-10.0, 0.0)), 0.0);
    assert_eq!(field.sample(Vec2::new(-5.0, 0.0)), 0.5); // round(0.25 × 2)=1，与WGSL相同。
    assert_eq!(field.sample(Vec2::new(0.0, 0.0)), 0.5);
    assert_eq!(field.sample(Vec2::new(10.0, 0.0)), 1.0);
    assert_eq!(field.sample(Vec2::new(10.1, 0.0)), 0.0); // 世界范围外不钳回边缘纹素。
    assert_eq!(field.finest_texel_size(), 10.0); // 单行常量轴不应覆盖X轴十米端点间距。
}

#[test]
fn source_cell_overlap_claims_neighbor_without_expanding_point_density() {
    let bounds = Rect::from_corners(Vec2::ZERO, Vec2::splat(2.0)); // 三乘三端点样本的单元间距为一米。
    let field = VegetationSourceField::new(
        bounds,
        3,
        3,
        vec![
            0.0, 0.0, 0.0, // 第一行不声明所有权。
            0.0, 1.0, 0.0, // 中央源样本是唯一非零声明单元。
            0.0, 0.0, 0.0, // 第三行不声明所有权。
        ],
    )
    .unwrap();
    let adjacent_center = Vec2::new(1.75, 1.0); // 最近邻点已经落到右侧零样本。
    assert_eq!(field.sample(adjacent_center), 0.0); // 实际渲染密度仍保持点采样，不向外膨胀。
    assert_eq!(
        field.sample_claim(
            adjacent_center,
            VegetationClaimSampling::SourceCellOverlap,
            0.5,
        ),
        1.0
    ); // 半米输出单元与中央一米源单元相交，因此仍声明空间所有权。
    assert_eq!(
        field.sample_claim(adjacent_center, VegetationClaimSampling::Point, 0.5),
        0.0
    ); // 默认模式逐字节保持既有点采样语义。
}
