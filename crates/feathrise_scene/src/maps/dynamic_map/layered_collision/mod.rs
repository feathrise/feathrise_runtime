//! 玩家中心、固定分块、按区域共享的可配置多层碰撞；与显示LOD完全独立。
mod activity;
mod activity_components; // 活动门独占的约束快照。
mod build; // 后台布局、原始高度采样与形状构建。
mod build_parallel; // 完整批次内按变化块有界并行，保留串行对照及原子发布。
mod driver; // 玩家需求身份和单飞阶段推进。
mod geometry; // 共享边与显式粗细过渡面。
mod stitches; // 跨对象边界在后台补齐共同曲线的物理节点。
mod lifecycle; // 首载、离场与失败保留旧版。
mod loading; // 局部源页加载及复用身份。
mod parallel; // 后台独立块worker调度及全员完成屏障。
mod planning; // 有界同心圆盘/圆环规划，不在帧关键路径运行。
mod publication; // 完整候选预检和物理步前原子采用。
mod resources; // 单飞任务与完整采用状态。
mod retirement; // 大型规划缓存与布局以固定槽后台释放，避免主帧析构峰值。
mod systems; // 版本身份、资产租约及原子采用。
mod types; // 固定世界块与接缝纯数据。 // 范围外暂停与返回前承托门。
pub(super) use activity::{freeze_terrain_activity, update_terrain_activity};
pub(super) use resources::LayeredCollisionRuntime; // 由地图插件初始化。
pub(super) use systems::{
    clear_layered_collision, drive_layered_collision, reap_layered_collision,
}; // 统一注册运行时生命周期。 // Update及物理步的双重安全门。

#[derive(bevy::prelude::SystemSet, Clone, Debug, PartialEq, Eq, Hash)] // 只提供诊断读取顺序，不暴露碰撞写权限。
pub struct LayeredCollisionDiagnosticsSystemSet; // 已采用集合及活动状态发布边界。

/// 保存任务在后台调用，拒绝默认窗口及对齐边界的超预算配置；运行时仍验证每次真实窗口。
pub fn validate_layered_collision_settings(
    spec: &feathrise_map_content::SparseHeightSpec,
    lods: &[feathrise_map_content::MapCollisionLod],
) -> Result<(), String> {
    // 不在UI帧调用此规模相关检查。
    feathrise_map_content::validate_collision_lods(lods, spec)?; // 首先验证配置含义。
    let center = (bevy::prelude::Vec2::from_array(spec.world_min)
        + bevy::prelude::Vec2::from_array(spec.world_max))
        * 0.5; // 中部窗口一般大于地图边界窗口。
    let step = bevy::prelude::Vec2::new(
        spec.actual_spacing_meters[0] as f32,
        spec.actual_spacing_meters[1] as f32,
    ) * ((1_u64 << planning::chunk_cells_log2(spec, lods)) as f32
        * lods[0].spacing_multiplier as f32); // 与实际固定块同尺度的代表相位。
    for x in [0.0, 0.5, 1.0] {
        for z in [0.0, 0.5, 1.0] {
            planning::plan(
                spec,
                lods, // 保存与运行时均只消费用户配置半径，没有额外覆盖路径。
                center + step * bevy::prelude::Vec2::new(x, z),
            )?;
        }
    } // 不默默提高Mip压预算。
    Ok(()) // 运行时尾块、其他相位和页预算继续保留同等硬门。
} // 与编辑器共享规划而非复制一套误差规则。

#[cfg(test)] // 后台布局与局部复用回归。
mod runtime_tests;
#[cfg(test)] // 纯几何与有限规模回归。
mod tests; // 由同域测试覆盖接缝、预算和层级数量。 // 使用生产构建路径验证完整候选。
