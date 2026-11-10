//! 景观动态代理的纯Mesh回归测试。
//! 【功能设计】测试只使用程序化小网格锁定既有预算和PBR属性，不启动窗口、AssetServer或RenderApp。

use bevy::asset::RenderAssetUsages; // 测试网格同时保留可读取的主世界属性。
use bevy::mesh::{Indices, PrimitiveTopology}; // 构造与正式代理相同的索引三角列表。
use bevy::prelude::*; // Mesh提供测试输入和输出检查接口。

use super::{build_rock_ultra_proxy, build_shrub_ultra_proxy}; // 直接验证私有代理算法和输出数据契约。

/// 构造四面体岩石输入。
fn tetrahedron_mesh() -> Mesh {
    let positions = vec![
        [-1.0, 0.0, -1.0], // 底面西北点。
        [1.0, 0.0, -1.0],  // 底面东北点。
        [0.0, 0.0, 1.0],   // 底面南点。
        [0.0, 2.0, 0.0],   // 顶点。
    ]; // 完成闭合四面体位置。
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList, // 使用正式代理相同拓扑。
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD, // 测试允许读取CPU属性。
    ); // 创建测试Mesh。
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions); // 写入凸包输入点。
    mesh.insert_indices(Indices::U32(vec![
        0, 2, 1, // 底面。
        0, 1, 3, // 北侧面。
        1, 2, 3, // 东南侧面。
        2, 0, 3, // 西南侧面。
    ])); // 完成测试索引。
    mesh // 返回最低岩石替身。
} // 完成测试岩石构造。

#[test] // 灌木代理必须守住三角预算并保留源UV。
fn shrub_proxy_keeps_source_attributes_with_bounded_triangles() {
    let mut source = Mesh::new(
        PrimitiveTopology::TriangleList, // 模拟叶片三角列表。
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD, // 允许算法读取。
    ); // 创建测试灌木。
    source.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [0.0, 0.0, 0.0], // 第一叶片A。
            [1.0, 0.0, 0.0], // 第一叶片B。
            [0.0, 1.0, 0.0], // 第一叶片C。
            [2.0, 0.0, 0.0], // 第二叶片A。
            [3.0, 0.0, 0.0], // 第二叶片B。
            [2.0, 1.0, 0.0], // 第二叶片C。
            [4.0, 0.0, 0.0], // 第三叶片A。
            [5.0, 0.0, 0.0], // 第三叶片B。
            [4.0, 1.0, 0.0], // 第三叶片C。
        ],
    ); // 写入三片分散叶面。
    source.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 9]); // 所有测试叶面朝正Z。
    source.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; 9]); // 提供AlphaMask所需UV属性。
    source.insert_indices(Indices::U32((0_u32..9).collect())); // 三片叶面按连续索引组成。
    let proxy = build_shrub_ultra_proxy(&source, 2).expect("测试灌木应成功生成两面代理"); // 限制为两个三角形。
    assert_eq!(proxy.indices().map(Indices::len), Some(6)); // 两个三角形严格六索引。
    assert!(proxy.count_vertices() <= 6); // 最坏每面不共享时仍最多六顶点。
    assert!(proxy.contains_attribute(Mesh::ATTRIBUTE_UV_0)); // 动态代理必须保留源UV。
} // 完成灌木代理预算测试。

#[test] // 岩石代理必须形成有法线、UV和切线的闭合凸包。
fn rock_proxy_builds_closed_pbr_mesh() {
    let proxy = build_rock_ultra_proxy(&tetrahedron_mesh(), 32).expect("四面体应生成有效岩石凸包"); // 使用小型确定性输入。
    assert!(proxy.count_vertices() >= 4); // 三维凸包至少四点。
    assert!(proxy.indices().is_some_and(|indices| indices.len() >= 12)); // 闭合四面体至少四个面。
    assert!(proxy.contains_attribute(Mesh::ATTRIBUTE_NORMAL)); // PBR光照必须有法线。
    assert!(proxy.contains_attribute(Mesh::ATTRIBUTE_UV_0)); // 复用最低岩石材质必须有UV。
    assert!(proxy.contains_attribute(Mesh::ATTRIBUTE_TANGENT)); // 法线贴图必须有切线。
} // 完成岩石代理属性测试。

// 结束动态代理纯逻辑测试。
