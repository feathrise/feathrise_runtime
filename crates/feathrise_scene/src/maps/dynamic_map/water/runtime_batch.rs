//! 动态地图三类水体的作者值冻结与主线程整批安装辅助。
//!
//! 【功能设计】`systems.rs`继续只编排资产门、single-flight身份和原子替换；河流/瀑布作者值
//! 克隆以及完成Mesh到统一水面实体的组装集中在这里，避免准备和安装系统继续膨胀。

use bevy::prelude::{Assets, Commands, Mesh}; // 主线程分帧登记资产，回执后一次生成实体。
use feathrise_map_content::{MapId, MapWaterAsset, MapWaterRiver, MapWaterfall}; // 冻结完整样条作者值供后台任务独占。
use feathrise_scene_core::state::ScenePhase; // 新安装水面统一随动态地图场景退出回收。

use crate::shared::large_map::{
    BuiltWaterSurface, DynamicWaterMaterial, PaintedWaterPurpose, StagedWaterSurface,
    spawn_staged_water_surface, stage_water_surface_chunk,
}; // 湖、河、急流和瀑布共用唯一正式安装入口。

use super::resources::InstalledDynamicWaterLayer; // 整批安装结果继续由动态地图状态拥有资产句柄。

/// 从当前已验证水域资产冻结三维河流与瀑布作者值，后台闭包不借用主世界Assets。
pub(super) fn freeze_spline_water_network(
    water: &MapWaterAsset, // 与R8层、清单和GLB派生高度属于同一准备代次的水域资产。
) -> (Vec<MapWaterRiver>, Vec<MapWaterfall>) {
    (
        water.rivers.clone(),     // 控制点和材质小型作者值完整移交single-flight任务。
        water.waterfalls.clone(), // 端点身份虽不参与建网格，仍与同代次完整资产一起冻结。
    )
}

/// 每帧只把一张后台完成Mesh登记为不可见资产候选。
pub(super) fn stage_dynamic_water_surface(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<DynamicWaterMaterial>,
    staged: &mut Vec<StagedWaterSurface>,
    build: BuiltWaterSurface,
    elapsed_seconds: f32,
) -> usize {
    stage_water_surface_chunk(commands, meshes, materials, staged, build, elapsed_seconds)
}

/// RenderApp完整回执后一次生成整批真实实体；调用方随后在同一Commands提交中回收旧批。
pub(super) fn spawn_dynamic_water_batch(
    commands: &mut Commands,
    map_id: &MapId,
    staged: Vec<StagedWaterSurface>,
) -> Vec<InstalledDynamicWaterLayer> {
    let mut installed = Vec::with_capacity(staged.len());
    for staged in staged {
        let spawn = spawn_staged_water_surface(
            commands,
            map_id.clone(),
            PaintedWaterPurpose::Runtime,
            staged,
        );
        commands
            .entity(spawn.entity)
            .insert(bevy::prelude::DespawnOnExit(ScenePhase::Active)); // 退出动态地图时提供第二道实体回收保险。
        installed.push(InstalledDynamicWaterLayer {
            spawn, // 保存实体、Mesh和材质强句柄供原子替换时回收。
        });
    }
    installed
}
