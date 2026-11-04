//! 通用Mesh公告板的变化驱动诊断聚合。
//! 【功能设计】顶点朝向已经迁入GPU，本系统只在节点拓扑变化后扫描一次并发布材质就绪状态。

use bevy::ecs::system::Local; // 本地游标保存上一已发布拓扑代次，不污染公共资源。
use bevy::prelude::{Query, Res, ResMut}; // 系统只读不可变节点并原子替换诊断快照。

use super::super::{
    components::VegetationMeshBillboardNode, // 节点保存模式与GPU材质就绪证据。
    resources::{
        VegetationMeshBillboardPhase, VegetationMeshBillboardStatus,
        VegetationMeshBillboardTopology,
    }, // 私有代次避免稳定帧扫描，公开状态供F3只读。
}; // 完成公告板状态系统依赖导入。

/// 只在公告板节点集合变化后重新聚合一次完整诊断快照。
pub(crate) fn synchronize_vegetation_mesh_billboard_status(
    topology: Res<VegetationMeshBillboardTopology>, // 组件钩子以O(1)推进的结构代次。
    nodes: Query<&VegetationMeshBillboardNode>,     // 变化帧才遍历固定与动态公告板节点。
    mut published_generation: Local<u64>,           // 保存上一份状态对应的拓扑代次。
    mut status: ResMut<VegetationMeshBillboardStatus>, // 只在真实拓扑变化时写入，保持变更检测安静。
) {
    let generation = topology.generation(); // 单次读取当前公告板结构身份。
    if *published_generation == generation {
        return; // 稳定游戏帧不再遍历全部固定与动态公告板节点。
    } // 完成稳定帧O(1)快速路径。

    let mut next = VegetationMeshBillboardStatus::default(); // 从完整零状态重建，删除全部节点时不会残留旧计数。
    for node in &nodes {
        next.total_nodes += 1; // 固定与动态模式共同进入总量。
        if node.billboard.requires_camera() {
            next.camera_facing_nodes += 1; // 动态模式由每个渲染视图的GPU顶点Shader朝向。
            if node.gpu_material_ready {
                next.oriented_nodes += 1; // 专用扩展材质已经承担当前节点的GPU转向。
            } else {
                next.fallback_nodes += 1; // 材质配置错误时保留作者Transform而不写入NaN或猜测姿态。
            } // 完成动态节点材质状态分类。
        } else {
            next.fixed_nodes += 1; // 固定交叉面保持作者姿态且不进入自定义顶点管线。
        } // 完成当前公告板模式分类。
    } // 完成变化帧唯一一次节点扫描。

    next.phase = if next.total_nodes == 0 {
        VegetationMeshBillboardPhase::NoBillboards // 世界中没有通用公告板节点。
    } else if next.camera_facing_nodes == 0 {
        VegetationMeshBillboardPhase::FixedOnly // 固定交叉面不增加GPU动态转向成本。
    } else if next.fallback_nodes == 0 {
        VegetationMeshBillboardPhase::Active // 全部动态节点都已进入GPU逐视图路径。
    } else {
        VegetationMeshBillboardPhase::ActiveWithFallback // 至少一个动态节点缺少专用扩展材质。
    }; // 完成结构化阶段聚合。
    *status = next; // 同一变化帧一次性发布完整自洽快照。
    *published_generation = generation; // 最后推进游标，后续稳定帧直接返回。
} // 完成GPU公告板变化驱动诊断同步。
