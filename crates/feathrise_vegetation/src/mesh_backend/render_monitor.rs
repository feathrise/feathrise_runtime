//! 通用Mesh后端的可选PBR RenderApp分箱监控。
//! 【功能设计】监控只按提取后的层、原型、LOD与部件身份读取Bevy公开分箱，不按共享Mesh句柄猜测所有权。

use bevy::core_pipeline::core_3d::{AlphaMask3d, Opaque3d}; // 主画面不透明与遮罩阶段覆盖通用树木的树干、树冠和公告板。
use bevy::ecs::system::SystemParam; // 把按需配置、三类分箱与输出快照收成单一渲染观察职责。
use bevy::pbr::{Shadow, queue_material_meshes, queue_shadows}; // 官方PBR排队系统提供稳定的只读采样边界。
use bevy::prelude::*; // 引入App、Plugin、Query和资源系统参数。
use bevy::render::sync_component::SyncComponentPlugin; // 只维护身份节点与RenderEntity映射，逐帧复制改由按需系统控制。
use bevy::render::sync_world::{MainEntity, MainEntityHashMap, RenderEntity}; // 公开分箱与提取组件通过同一主世界实体身份关联。
use bevy::render::{
    Extract, ExtractSchedule, MainWorld, Render, RenderApp, RenderSystems,
    render_phase::ViewBinnedRenderPhases,
}; // 上一完整Render阶段快照在下一帧Extract阶段无锁回传主世界。

use crate::pbr_mdi_monitor::{PbrMdiQueueMirror, PbrMdiQueueMirrorParams, PbrPhaseStats}; // Bevy 0.19隐藏MDI成员后由项目只读镜像恢复与旧版一致的精确统计。

use super::components::VegetationMeshRenderIdentity; // 分箱只能命中通用后端实际生成并提取的渲染子节点。
use super::resources::{VegetationMeshRenderMonitorConfig, VegetationMeshRenderStats}; // 主世界按需开关控制身份提取和完整帧快照。
use super::types::{
    VegetationMeshRenderPart, VegetationMeshRenderPartStats, VegetationMeshRenderPhaseStats,
    VegetationMeshRenderTarget, VegetationMeshRenderTargetStats,
}; // 统计按内容无关技术身份稳定分组。

/// 可选安装通用Mesh RenderApp分箱监控。
///
/// `VegetationMeshPlugin`不会自动安装本插件；项目必须在迁移诊断或正式性能观测阶段显式组合，
/// 因而仅创建公共Mesh实例不会改变生产渲染调度或旧实现所有权。
#[derive(Clone, Copy, Debug, Default)]
pub struct VegetationMeshRenderMonitorPlugin; // 独立插件维持P5树木生产候选注册默认关闭。

impl Plugin for VegetationMeshRenderMonitorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VegetationMeshRenderMonitorConfig>() // 默认关闭，普通游戏帧不承担诊断身份提取成本。
            .init_resource::<VegetationMeshRenderStats>(); // 首个按需完整Render阶段前向读取者提供明确未采样状态。
        if app.get_sub_app_mut(RenderApp).is_none() {
            return; // 无RenderApp的ECS回归仍可装配插件并核对默认关闭资源。
        } // 无渲染世界时也不安装SyncComponent钩子，避免fixture销毁实体时读取不存在的同步资源。
        app.add_plugins(SyncComponentPlugin::<VegetationMeshRenderIdentity>::default()); // 组件出现或移除时维护同步实体，稳态不逐帧复制身份。
        let render_app = app
            .get_sub_app_mut(RenderApp)
            .expect("RenderApp existence was checked before installing synchronization"); // 同步插件装配后重新取得独立渲染世界。
        render_app
            .init_resource::<VegetationMeshRenderMonitorConfig>() // Extract阶段从主世界同步当前按需状态。
            .init_resource::<VegetationMeshRenderStats>() // Render世界原位维护当前完整PBR排队快照。
            .add_systems(
                ExtractSchedule,
                (
                    extract_vegetation_mesh_render_identities, // 仅在F3景观物页显示时遍历并复制技术身份。
                    publish_vegetation_mesh_render_stats, // 同步开关并发布上一完整Render阶段结果。
                ),
            ) // 帧首完成按需身份提取、状态同步和快照发布。
            .add_systems(
                Render,
                observe_vegetation_mesh_render_phases
                    .in_set(RenderSystems::QueueMeshes)
                    .after(queue_material_meshes)
                    .after(queue_shadows), // 所有标准或扩展PBR材质完成主画面和阴影排队后再只读分箱。
            ); // 完成可选监控调度装配。
    } // 完成插件注册。
} // 完成通用Mesh渲染监控插件实现。

/// 仅在诊断明确启用时，把通用Mesh技术身份复制到已有RenderEntity。
fn extract_vegetation_mesh_render_identities(
    mut commands: Commands, // 批量写入本轮需要观察的RenderApp实体。
    config: Extract<Res<VegetationMeshRenderMonitorConfig>>, // 直接读取主世界F3按需状态，避免常驻提取插件遍历。
    identities: Extract<Query<(RenderEntity, &VegetationMeshRenderIdentity)>>, // 开启时才迭代真实通用Mesh子节点。
) {
    if !config.enabled() {
        return; // 普通游戏帧不遍历数千节点，也不分配批量写入向量。
    }
    let extracted = identities
        .iter()
        .map(|(render_entity, identity)| (render_entity, *identity)) // 身份为紧凑Copy值，不复制Mesh或材质资产。
        .collect::<Vec<_>>(); // Bevy延迟命令要求拥有所有权的Send + Sync批次，不能借用Extract查询迭代器。
    commands.try_insert_batch(extracted); // 一次把当前会话全部有效身份写入RenderApp。
}

/// 把上一完整Render阶段快照复制回主世界，避免F3读到半更新数据。
fn publish_vegetation_mesh_render_stats(
    mut main_world: ResMut<MainWorld>, // Bevy提供的RenderApp到主世界安全回写通道。
    mut render_config: ResMut<VegetationMeshRenderMonitorConfig>, // Render阶段只读取本次Extract冻结的开关。
    render_stats: Res<VegetationMeshRenderStats>, // 当前值已在上一完整Render阶段覆盖。
) {
    let enabled = main_world
        .get_resource::<VegetationMeshRenderMonitorConfig>()
        .is_some_and(VegetationMeshRenderMonitorConfig::enabled); // 缺主世界配置与默认值都按关闭处理。
    render_config.set_enabled(enabled); // 同一Extract阶段把F3边沿传给后续Render调度。
    if let Some(mut main_stats) = main_world.get_resource_mut::<VegetationMeshRenderStats>() {
        if enabled {
            main_stats.clone_from(&render_stats); // 开启时发布上一完整采样，稳定形状复用Vec容量。
        } else {
            main_stats.sampled = false; // 关闭后不允许F3或其他读取者沿用旧帧证据。
            main_stats.targets.clear(); // 截断目标但保留容量，下一次开启仍可复用分配。
        }
    } // 插件拆装或极简测试期间允许主世界资源暂时不存在。
} // 完成上一完整帧快照发布。

/// 一次通用Mesh RenderApp观察所需的完整按需输入与输出。
#[derive(SystemParam)]
struct VegetationMeshRenderMonitorParams<'w, 's> {
    config: Res<'w, VegetationMeshRenderMonitorConfig>, // Extract阶段冻结的按需开关决定本帧是否承担分箱扫描。
    identities: Query<'w, 's, (&'static MainEntity, &'static VegetationMeshRenderIdentity)>, // 只包含本通用后端生成的同步渲染实体。
    opaque_phases: Option<Res<'w, ViewBinnedRenderPhases<Opaque3d>>>, // PBR插件未安装时保持可选而不是触发系统参数错误。
    alpha_mask_phases: Option<Res<'w, ViewBinnedRenderPhases<AlphaMask3d>>>, // 覆盖树冠与公告板遮罩阶段。
    shadow_phases: Option<Res<'w, ViewBinnedRenderPhases<Shadow>>>, // 覆盖方向光级联及其他灯光阴影阶段。
    mdi: PbrMdiQueueMirrorParams<'w, 's>, // 只读复现官方队列门，补齐0.19私有MDI成员的实体归属。
    render_stats: ResMut<'w, VegetationMeshRenderStats>, // 本轮一次覆盖全部目标和部件快照。
}

/// 在官方PBR排队完成后按提取技术身份统计真实公开分箱。
fn observe_vegetation_mesh_render_phases(
    mut params: VegetationMeshRenderMonitorParams, // 具名参数组保持按需状态、PBR输入和快照输出属于同一职责。
    mut identity_by_entity: Local<MainEntityHashMap<VegetationMeshRenderIdentity>>, // 每帧清空并复用容量的实体身份索引。
    mut groups: Local<Vec<(VegetationMeshRenderTarget, VegetationMeshRenderPart)>>, // 稳态复用容量的确定性分组形状。
    mut mdi_mirror: Local<PbrMdiQueueMirror>, // 仅在F3授权期间重建并跨帧复用条目容量。
) {
    if !params.config.enabled() {
        params.render_stats.sampled = false; // 关闭期间明确表示没有执行过本帧诊断采样。
        params.render_stats.targets.clear(); // 释放逻辑长度并保留容量，避免重开时读取陈旧目标。
        return; // 不建立身份索引，也不遍历Opaque、AlphaMask或Shadow分箱。
    }
    let (Some(opaque_phases), Some(alpha_mask_phases), Some(shadow_phases)) = (
        &params.opaque_phases,
        &params.alpha_mask_phases,
        &params.shadow_phases,
    ) else {
        params.render_stats.sampled = false; // 没有完整PBR阶段时不得把空值冒充真实零提交。
        params.render_stats.targets.clear(); // 同时截断可能来自上一套渲染插件的陈旧目标。
        return; // 极简RenderApp没有可观察分箱。
    }; // 完成完整PBR资源门。
    identity_by_entity.clear(); // 每帧只保留当前仍同步到RenderApp的通用Mesh节点。
    identity_by_entity.extend(
        params
            .identities
            .iter()
            .map(|(main_entity, identity)| (*main_entity, *identity)), // 键和值均为紧凑Copy身份，不复制Mesh或材质资产。
    ); // 完成本帧实体身份索引。
    reconcile_stats_shape(&mut params.render_stats, &identity_by_entity, &mut groups); // 只有身份结构变化时重建目标/部件向量。
    if !mdi_mirror.rebuild(
        &params.mdi,
        opaque_phases,
        alpha_mask_phases,
        shadow_phases,
        |entity, _| identity_by_entity.contains_key(&entity), // 镜像只记录本监控已提取的通用Mesh实体。
    ) {
        params.render_stats.sampled = false; // 官方队列输入不完整时不能把普通分箱的部分统计冒充完整结果。
        params.render_stats.targets.clear(); // 同时清除上一套RenderApp留下的陈旧目标。
        return;
    } // MDI为空也属于一次有效重建，后续与公开普通分箱统一汇总。

    for target_stats in &mut params.render_stats.targets {
        let target = target_stats.target; // Copy目标解除后续字段可变借用冲突。
        let matches_target = |entity: MainEntity| {
            identity_by_entity
                .get(&entity)
                .copied()
                .is_some_and(|identity| render_target(identity) == target)
        }; // 合计只匹配当前层与原型，旧实现没有提取身份因此永不命中。
        target_stats.opaque =
            phase_stats(mdi_mirror.summarize_opaque(opaque_phases, |_| true, matches_target)); // 完成目标Opaque3d普通与MDI合计。
        target_stats.alpha_mask = phase_stats(mdi_mirror.summarize_alpha_mask(
            alpha_mask_phases,
            |_| true,
            matches_target,
        )); // 完成目标AlphaMask3d普通与MDI合计。
        target_stats.shadow =
            phase_stats(mdi_mirror.summarize_shadow(shadow_phases, |_| true, matches_target)); // 完成目标Shadow普通与MDI合计。

        for part_stats in &mut target_stats.parts {
            let part = part_stats.part; // Copy部件键供三个阶段共享同一过滤口径。
            let matches_part = |entity: MainEntity| {
                identity_by_entity
                    .get(&entity)
                    .copied()
                    .is_some_and(|identity| {
                        render_target(identity) == target && render_part(identity) == part
                    }) // 同时匹配层、原型、LOD、部件和公告板位。
            }; // 完成精确部件过滤器。
            part_stats.opaque =
                phase_stats(mdi_mirror.summarize_opaque(opaque_phases, |_| true, matches_part)); // 统计当前部件Opaque3d普通与MDI分箱。
            part_stats.alpha_mask = phase_stats(mdi_mirror.summarize_alpha_mask(
                alpha_mask_phases,
                |_| true,
                matches_part,
            )); // 统计当前部件AlphaMask3d普通与MDI分箱。
            part_stats.shadow =
                phase_stats(mdi_mirror.summarize_shadow(shadow_phases, |_| true, matches_part)); // 统计当前部件Shadow普通与MDI分箱。
        } // 完成当前目标全部LOD部件统计。
    } // 完成全部层与原型目标统计。
    params.render_stats.sampled = true; // 包括确实没有通用节点的全零帧，也属于一次完整PBR采样。
} // 完成当前Render阶段观察。

/// 让统计向量精确匹配当前提取身份，并在稳定帧复用既有容量。
fn reconcile_stats_shape(
    stats: &mut VegetationMeshRenderStats, // 保留稳定目标和部件Vec分配。
    identities: &MainEntityHashMap<VegetationMeshRenderIdentity>, // 当前全部通用渲染子节点身份。
    groups: &mut Vec<(VegetationMeshRenderTarget, VegetationMeshRenderPart)>, // 调用方持有的跨帧暂存向量。
) {
    groups.clear(); // 保留上一帧容量，只清除陈旧身份。
    groups.extend(
        identities
            .values()
            .copied()
            .map(|identity| (render_target(identity), render_part(identity))), // 每个实体投影为精确目标与部件组合。
    ); // 完成原始分组键收集。
    groups.sort_unstable(); // 层、原型、LOD、部件和公告板按派生Ord形成确定性顺序。
    groups.dedup(); // 同一部件的数百实例只保留一个统计槽。

    if stats_shape_matches(stats, groups) {
        reset_stats_values(stats); // 稳态仅归零数值，目标和每个部件Vec都不重新分配。
        return; // 已保持精确形状，无需重建。
    } // 完成稳定形状快速路径。

    stats.targets.clear(); // 身份热替换或离图时截断不再存在的目标。
    for &(target, part) in groups.iter() {
        if stats.targets.last().map(|stats| stats.target) != Some(target) {
            stats.targets.push(VegetationMeshRenderTargetStats {
                target,               // 新层或原型创建唯一目标统计槽。
                ..Default::default()  // 三阶段从零开始，部件随后按稳定顺序填入。
            }); // 完成目标槽创建。
        } // 同目标后续部件复用末尾槽。
        stats
            .targets
            .last_mut()
            .expect("target was inserted before its first render part")
            .parts
            .push(VegetationMeshRenderPartStats {
                part,                 // 当前精确LOD、部件和公告板位身份。
                ..Default::default()  // 三阶段统计由本帧公开分箱随后覆盖。
            }); // 完成目标部件槽创建。
    } // 完成全部确定性目标和部件形状重建。
} // 完成统计形状对齐。

/// 判断既有统计目标和部件是否与本帧精确分组键完全一致。
fn stats_shape_matches(
    stats: &VegetationMeshRenderStats, // 上一帧保留的排序形状。
    groups: &[(VegetationMeshRenderTarget, VegetationMeshRenderPart)], // 本帧排序去重后的期望形状。
) -> bool {
    let mut group_index = 0usize; // 线性游标同时核对目标边界和部件顺序。
    for target_stats in &stats.targets {
        let Some((target, _)) = groups.get(group_index) else {
            return false; // 旧统计仍有目标但本帧分组已经提前结束。
        }; // 完成当前期望目标读取。
        if *target != target_stats.target {
            return false; // 目标层或原型变化必须重建。
        } // 完成目标身份核对。
        let mut part_count = 0usize; // 当前目标期望部件数。
        while let Some((group_target, group_part)) = groups.get(group_index) {
            if *group_target != target_stats.target {
                break; // 下一目标由外层循环核对。
            } // 完成目标边界检测。
            if target_stats.parts.get(part_count).map(|stats| stats.part) != Some(*group_part) {
                return false; // LOD、部件编号或公告板位变化必须重建。
            } // 完成当前部件身份核对。
            part_count = part_count.saturating_add(1); // 推进当前目标部件游标。
            group_index = group_index.saturating_add(1); // 推进全局分组游标。
        } // 完成当前目标全部期望部件核对。
        if part_count != target_stats.parts.len() {
            return false; // 旧统计残留额外部件时同样必须截断。
        } // 完成当前目标部件数量核对。
    } // 完成全部既有目标核对。
    group_index == groups.len() // 本帧仍有未消费目标或部件时形状不完整。
} // 完成形状一致性判断。

/// 稳态原位归零全部阶段值，保留目标和部件向量容量。
fn reset_stats_values(stats: &mut VegetationMeshRenderStats) {
    for target in &mut stats.targets {
        target.opaque = VegetationMeshRenderPhaseStats::default(); // 清除上一帧Opaque3d计数。
        target.alpha_mask = VegetationMeshRenderPhaseStats::default(); // 清除上一帧AlphaMask3d计数。
        target.shadow = VegetationMeshRenderPhaseStats::default(); // 清除上一帧Shadow计数。
        for part in &mut target.parts {
            part.opaque = VegetationMeshRenderPhaseStats::default(); // 清除当前部件上一帧Opaque3d计数。
            part.alpha_mask = VegetationMeshRenderPhaseStats::default(); // 清除当前部件上一帧AlphaMask3d计数。
            part.shadow = VegetationMeshRenderPhaseStats::default(); // 清除当前部件上一帧Shadow计数。
        } // 完成当前目标全部部件归零。
    } // 完成全部目标归零。
} // 完成稳定形状数值重置。

/// 把共享PBR统计值投影为通用植被F3公开快照。
fn phase_stats(stats: PbrPhaseStats) -> VegetationMeshRenderPhaseStats {
    VegetationMeshRenderPhaseStats {
        views: stats.views,             // 保持主相机与阴影子视图去重口径。
        draw_groups: stats.draw_groups, // 保持MDI批集、普通批次和独立实体提交组口径。
        bins: stats.bins,               // 保持完整官方键的唯一分箱口径。
        instances: stats.instances,     // 保持跨视图目标实体累计口径。
    }
}

/// 把提取组件投影为层与原型目标身份。
fn render_target(identity: VegetationMeshRenderIdentity) -> VegetationMeshRenderTarget {
    VegetationMeshRenderTarget {
        layer: identity.layer,         // 保留布局层归属。
        prototype: identity.prototype, // 保留注册原型归属。
    } // 返回精确目标键。
} // 完成目标身份投影。

/// 把提取组件投影为LOD与部件身份。
fn render_part(identity: VegetationMeshRenderIdentity) -> VegetationMeshRenderPart {
    VegetationMeshRenderPart {
        level: identity.level,         // 保留任意LOD档位。
        part_id: identity.part_id,     // 保留原型稳定部件编号。
        billboard: identity.billboard, // 保留公告板位供F3独立核对。
    } // 返回精确部件键。
} // 完成部件身份投影。

#[cfg(test)] // 纯App与值结构回归不启动窗口或RenderApp。
mod tests {
    use super::*; // 复用私有形状对齐函数和公共监控资源。
    use crate::{VegetationLayerId, VegetationMeshLodLevel, VegetationPrototypeId}; // 构造内容无关技术身份。

    /// 构造一个无需Mesh资产的渲染身份fixture。
    fn identity(
        layer: u64,      // 测试布局层数字身份。
        prototype: u64,  // 测试原型数字身份。
        level: u8,       // 测试LOD档位。
        part_id: u32,    // 测试部件编号。
        billboard: bool, // 测试公告板位。
    ) -> VegetationMeshRenderIdentity {
        VegetationMeshRenderIdentity {
            layer: VegetationLayerId(layer),             // 写入稳定层身份。
            prototype: VegetationPrototypeId(prototype), // 写入稳定原型身份。
            level: VegetationMeshLodLevel(level),        // 写入稳定LOD档位。
            part_id,                                     // 写入稳定部件编号。
            billboard,                                   // 写入公告板位。
        } // 返回紧凑提取组件。
    } // 完成fixture构造。

    #[test] // 无RenderApp的机器回归也必须能安全装配可选监控插件。
    fn monitor_plugin_without_render_app_keeps_unsampled_empty_stats() {
        let mut app = App::new(); // 不安装DefaultPlugins或任何渲染子应用。
        app.add_plugins(VegetationMeshRenderMonitorPlugin); // 显式装配可选监控而不启动窗口。
        let entity = app.world_mut().spawn(identity(1, 2, 0, 3, false)).id(); // 模拟fixture生成一个可提取子节点。
        assert!(app.world_mut().despawn(entity)); // 无RenderApp时不得留下读取渲染同步资源的组件钩子。
        let config = app.world().resource::<VegetationMeshRenderMonitorConfig>(); // 可选插件在任何组合下都必须创建同一默认关闭授权资源。
        assert!(!config.enabled()); // 未由F3景观物页授权时不得常驻提取或扫描。
        let stats = app.world().resource::<VegetationMeshRenderStats>(); // 主世界资源必须可供F3安全读取。
        assert!(!stats.sampled); // 没有Render阶段不得伪报已采样。
        assert!(stats.targets.is_empty()); // 没有提取实体不得残留目标。
    } // 完成无RenderApp安全装配验证。

    #[test] // 身份顺序和重复实体不能改变F3目标/部件分组形状。
    fn render_stats_shape_is_sorted_and_exact_by_target_lod_and_part() {
        let mut identities = MainEntityHashMap::default(); // 模拟RenderApp提取后的实体身份表。
        identities.insert(
            MainEntity::from(Entity::from_raw_u32(9).unwrap()),
            identity(2, 8, 3, 4, true),
        ); // 先插入后排序的公告板部件。
        identities.insert(
            MainEntity::from(Entity::from_raw_u32(3).unwrap()),
            identity(1, 7, 0, 1, false),
        ); // 插入第一目标树干部件。
        identities.insert(
            MainEntity::from(Entity::from_raw_u32(4).unwrap()),
            identity(1, 7, 0, 1, false),
        ); // 重复实例不得新增部件槽。
        identities.insert(
            MainEntity::from(Entity::from_raw_u32(5).unwrap()),
            identity(1, 7, 1, 2, false),
        ); // 插入第一目标第二LOD部件。
        let mut stats = VegetationMeshRenderStats::default(); // 从未采样空快照开始。
        let mut groups = Vec::new(); // 模拟跨帧复用的形状暂存。

        reconcile_stats_shape(&mut stats, &identities, &mut groups); // 构建确定性目标和部件结构。

        assert_eq!(stats.targets.len(), 2); // 两个层与原型组合必须保持独立目标。
        assert_eq!(stats.targets[0].target.layer, VegetationLayerId(1)); // 目标按层稳定升序。
        assert_eq!(stats.targets[0].target.prototype, VegetationPrototypeId(7)); // 第一目标保留精确原型。
        assert_eq!(stats.targets[0].parts.len(), 2); // 重复实例折叠为两个唯一LOD部件。
        assert_eq!(
            stats.targets[0].parts[0].part.level,
            VegetationMeshLodLevel(0)
        ); // 部件先按LOD升序。
        assert_eq!(stats.targets[0].parts[1].part.part_id, 2); // 第二部件保留原型编号。
        assert!(stats.targets[1].parts[0].part.billboard); // 第二目标公告板位不能丢失。

        stats.targets[0].opaque.instances = 99; // 模拟上一帧已经写入真实提交量。
        let first_parts_capacity = stats.targets[0].parts.capacity(); // 记录稳定形状分配容量。
        reconcile_stats_shape(&mut stats, &identities, &mut groups); // 同一身份形状进入稳态快速路径。
        assert_eq!(stats.targets[0].opaque.instances, 0); // 稳态必须清除上一帧计数。
        assert_eq!(stats.targets[0].parts.capacity(), first_parts_capacity); // 稳态不得重建部件向量。
    } // 完成精确分组与稳定容量验证。
} // 结束通用Mesh渲染监控回归。
