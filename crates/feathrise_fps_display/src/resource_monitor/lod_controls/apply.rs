//! 把F3四档调试状态应用到GPU叶片提取标记和Bevy可见性。

use bevy::prelude::*; // 引入资源变更检测、实体查询、可见性和延迟命令。
use feathrise_environment::landscape::{LandscapeKind, LandscapeLodLevel, LandscapeLodMesh}; // 灌木与岩石通过节点可见性退出主视图和阴影。
use feathrise_environment::tree::{TreeLodLevel, TreeLodMesh}; // 树木高、中、低和公告板使用同一可见性路径。
use feathrise_environment::vegetation::blade::{
    GpuBladeLodDebugDisabled, GpuBladePatch,
}; // GPU叶片通过移除渲染世界草窗停止全部GPU阶段，当前地图区分复用零号profile的普通草与湿地草。

use feathrise_environment::vegetation::VegetationBladeLayer;
use feathrise_map_content::VegetationPresetRegistry;
use crate::resource_monitor::blade_identity::content_for_blade_layer;
use super::LodControlPresetBindings;
use super::state::{LodControlContent, LodControlState}; // 读取F3/F4共享且九类内容互不串改的四档状态。

type TreeLodVisibilityData = (&'static TreeLodMesh, &'static mut Visibility);
type LandscapeLodVisibilityData = (&'static LandscapeLodMesh, &'static mut Visibility);
type NewTreeLodFilter = (Added<TreeLodMesh>, Without<LandscapeLodMesh>);
type NewLandscapeLodFilter = (Added<LandscapeLodMesh>, Without<TreeLodMesh>);

/// 当调试状态变化时，把九类内容的开关应用到所有已存在 LOD 节点。
pub(in crate::resource_monitor) fn apply_changed_lod_controls(
    lod_state: Res<LodControlState>, // 只有资源变化帧才扫描全部已有LOD节点。
    bindings: Option<Res<LodControlPresetBindings>>, // 应用提供的诊断绑定独立于预设运行。
    presets: Option<Res<VegetationPresetRegistry>>, // 数据预设决定正式层内容，不依赖地图编号。
    mut previous: Local<super::targets::Targets>,
    mut presence: Local<(bool, bool)>,
    mut commands: Commands,                         // 为GPU叶片窗口插入或移除主世界调试关闭标记。
    grass_q: Query<(
        Entity,                        // 标记命令需要主世界实体编号。
        &GpuBladePatch,                // LOD档号来自草窗本身。
        Option<&VegetationBladeLayer>, // 只有明确层身份的窗口归F3控制。
        Has<GpuBladeLodDebugDisabled>, // 避免重复插入或移除同一关闭标记。
    )>,
    mut tree_q: Query<TreeLodVisibilityData, Without<LandscapeLodMesh>>, // 树木节点关闭后同时退出主视图与阴影视图。
    mut landscape_q: Query<LandscapeLodVisibilityData, Without<TreeLodMesh>>, // 景观节点与树木可变查询严格互斥。
) {
    let next_presence = (presets.is_some(), bindings.is_some());
    let removed = *presence != next_presence;
    *presence = next_presence;
    if !removed && !lod_state.is_changed() && presets.as_ref().is_none_or(|presets| !presets.is_changed())
        && bindings.as_ref().is_none_or(|bindings| !bindings.is_changed()) {
        return; // 没有用户操作时不逐帧扫描数千个固定景观节点。
    } // 完成低成本快速返回。
    let targets = super::targets::Targets::resolve(presets.as_deref(), bindings.as_deref());
    for (entity, patch, layer, disabled) in &grass_q {
        if disabled && (patch.lod_level() as usize) < super::state::LOD_LEVEL_COUNT
            && layer.is_some_and(|layer| previous.blade(layer.0.0).is_some() && targets.blade(layer.0.0).is_none()) {
            commands.entity(entity).remove::<GpuBladeLodDebugDisabled>();
        } // 解绑仅归还此前明确拥有的层，不触碰F4当前自定义目标。
        sync_grass_patch(&lod_state, presets.as_deref(), bindings.as_deref(), &mut commands, entity, patch, layer, disabled); // 同步五类页面身份对应的GPU叶片四档。
    } // 完成全部现有GPU草窗同步。
    for (lod, mut visibility) in &mut tree_q {
        if targets.bound[LodControlContent::Trees.slot()] {
            sync_tree_visibility(&lod_state, lod, &mut visibility);
        } else if previous.bound[LodControlContent::Trees.slot()] {
            write_lod_visibility(true, &mut visibility);
        }
    } // 完成全部现有树木节点同步。
    for (lod, mut visibility) in &mut landscape_q {
        let content = landscape_content(lod);
        if targets.bound[content.slot()] {
            sync_landscape_visibility(&lod_state, lod, &mut visibility);
        } else if previous.bound[content.slot()] { write_lod_visibility(true, &mut visibility); }
    } // 完成全部现有固定景观节点同步。
    *previous = targets;
} // 完成状态变化全量应用系统。

/// 把已经选择的调试状态应用到后续新加载的 LOD 节点。
pub(in crate::resource_monitor) fn apply_lod_controls_to_new_nodes(
    lod_state: Res<LodControlState>, // 地图切换后继续沿用本次进程内用户选择。
    bindings: Option<Res<LodControlPresetBindings>>, // 应用提供的诊断绑定独立于预设运行。
    presets: Option<Res<VegetationPresetRegistry>>, // 新窗口按正式层身份继承现有开关。
    mut commands: Commands,                         // 为新GPU叶片窗口补齐关闭标记。
    grass_q: Query<
        (
            Entity,                        // 标记命令需要新窗口实体编号。
            &GpuBladePatch,                // 读取新窗口内容和档位。
            Option<&VegetationBladeLayer>, // 新窗使用同一正式身份。
            Has<GpuBladeLodDebugDisabled>, // 核对新窗口是否已经带正确标记。
        ),
        Added<GpuBladePatch>, // 只扫描本帧新增草窗，不增加稳态逐帧成本。
    >,
    mut tree_q: Query<TreeLodVisibilityData, NewTreeLodFilter>, // 只扫描新增树木，并与景观物可变查询保持互斥。
    mut landscape_q: Query<LandscapeLodVisibilityData, NewLandscapeLodFilter>, // 只扫描新增景观物，并与树木可变查询保持互斥。
) {
    for (entity, patch, layer, disabled) in &grass_q {
        sync_grass_patch(&lod_state, presets.as_deref(), bindings.as_deref(), &mut commands, entity, patch, layer, disabled); // 新草窗立即遵守当前地图内容页已有调试选择。
    } // 完成新增GPU草窗同步。
    for (lod, mut visibility) in &mut tree_q {
        if bindings.as_deref().and_then(|bindings| bindings.profile(LodControlContent::Trees)).is_some() {
            sync_tree_visibility(&lod_state, lod, &mut visibility);
        }
    } // 完成新增树木节点同步。
    for (lod, mut visibility) in &mut landscape_q {
        if bindings.as_deref().and_then(|bindings| bindings.profile(landscape_content(lod))).is_some() {
            sync_landscape_visibility(&lod_state, lod, &mut visibility);
        }
    } // 完成新增固定景观节点同步。
} // 完成新增节点增量应用系统。

/// 同步一个GPU叶片窗口的调试关闭标记。
fn sync_grass_patch(
    state: &LodControlState, // 读取当前内容独立四档状态。
    presets: Option<&VegetationPresetRegistry>, // 读取当前目录的稳定层身份。
    bindings: Option<&LodControlPresetBindings>,
    commands: &mut Commands, // 插入或移除主世界调试标记。
    entity: Entity,          // 当前GPU草窗实体。
    patch: &GpuBladePatch,   // 当前窗口的LOD编号。
    layer: Option<&VegetationBladeLayer>, // 未绑定窗口不猜测内容。
    disabled: bool,          // 当前是否已经带关闭标记。
) {
    let Some(page) = layer.and_then(|layer| content_for_blade_layer(layer, presets, bindings)) else {
        return; // 未知或未来内容不受当前九类内容控件影响。
    }; // 完成GPU叶片内容到F3页映射。
    let level = patch.lod_level() as usize;
    if level >= super::state::LOD_LEVEL_COUNT {
        return; // F3四档控件不覆盖数据预设的后续扩展档。
    }
    let should_disable = !state.is_enabled(page, level); // 关闭只影响当前内容当前档。
    if should_disable && !disabled {
        commands.entity(entity).insert(GpuBladeLodDebugDisabled); // 下一次提取主动移除渲染草窗并停止全部GPU阶段。
    } else if !should_disable && disabled {
        commands.entity(entity).remove::<GpuBladeLodDebugDisabled>(); // 恢复后重新提取草窗并在原有距离范围内参与渲染。
    } // 已经一致时不产生延迟命令。
} // 完成单个GPU草窗同步。

/// 同步一个树木LOD节点的主视图与阴影可见性。
fn sync_tree_visibility(
    state: &LodControlState,     // 读取树木独立四档状态。
    lod: &TreeLodMesh,           // 当前节点携带高、中、低或公告板身份。
    visibility: &mut Visibility, // Hidden会让节点退出主视图和方向光阴影视图。
) {
    let level = match lod.level {
        TreeLodLevel::High => 0,      // 高模映射零档。
        TreeLodLevel::Medium => 1,    // 中模映射一档。
        TreeLodLevel::Low => 2,       // 低模映射二档。
        TreeLodLevel::Billboard => 3, // 公告板映射三档。
    }; // 完成树木四档映射。
    write_lod_visibility(
        state.is_enabled(LodControlContent::Trees, level),
        visibility,
    ); // 应用树木页独立状态。
} // 完成单个树木节点同步。

/// 同步一个灌木或岩石LOD节点的主视图与阴影可见性。
fn sync_landscape_visibility(
    state: &LodControlState,     // 读取灌木或岩石独立四档状态。
    lod: &LandscapeLodMesh,      // 当前节点携带类别和高、中、低、超低身份。
    visibility: &mut Visibility, // Hidden会让节点退出主视图和方向光阴影视图。
) {
    let content = landscape_content(lod);
    let level = match lod.level {
        LandscapeLodLevel::High => 0,   // 高模映射零档。
        LandscapeLodLevel::Medium => 1, // 中模映射一档。
        LandscapeLodLevel::Low => 2,    // 低模映射二档。
        LandscapeLodLevel::Ultra => 3,  // 超低代理映射三档。
    }; // 完成固定景观四档映射。
    write_lod_visibility(state.is_enabled(content, level), visibility); // 应用当前类别独立状态。
} // 完成单个固定景观节点同步。

fn landscape_content(lod: &LandscapeLodMesh) -> LodControlContent {
    match lod.kind { LandscapeKind::Shrub => LodControlContent::Shrubs, LandscapeKind::Rock => LodControlContent::Rocks }
}

/// 只在目标状态变化时写入LOD节点可见性。
pub(super) fn write_lod_visibility(enabled: bool, visibility: &mut Visibility) {
    let target = if enabled {
        Visibility::Inherited // 恢复父级和VisibilityRange共同决定的正式可见性。
    } else {
        Visibility::Hidden // 关闭后同时退出主视图、深度与阴影阶段。
    }; // 完成目标可见性选择。
    if *visibility != target {
        *visibility = target; // 避免稳态重复触发Bevy可见性传播变更。
    } // 完成差异写回。
} // 完成LOD可见性写入。

#[cfg(test)] // 系统参数初始化回归不启动窗口或渲染器。
mod tests {
    use super::*; // 直接运行生产全量与增量同步系统。
    use bevy::ecs::system::RunSystemOnce; // 一次性系统运行会执行Bevy真实查询访问冲突验证。

    #[test] // 树木与景观物共同可变访问Visibility时必须保持查询互斥。
    fn lod_apply_systems_initialize_without_visibility_conflicts() {
        let mut world = World::new(); // 创建足以初始化查询和资源参数的最小ECS世界。
        world.init_resource::<LodControlState>(); // 两个生产系统都要求默认四档全开的调试状态。
        world
            .run_system_once(apply_changed_lod_controls)
            .expect("全量LOD同步系统不应触发Visibility查询B0001"); // 回归用户启动时发生的全量系统崩溃。
        world
            .run_system_once(apply_lod_controls_to_new_nodes)
            .expect("新增LOD同步系统不应触发Visibility查询B0001"); // 同时锁定同构增量查询不留下潜在崩溃。
    } // 完成生产系统参数初始化回归。
} // 结束LOD应用系统测试模块。
