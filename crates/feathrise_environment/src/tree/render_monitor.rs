//! 树木PBR渲染阶段遥测。
//! 本模块只读取Bevy公开分箱并把上一完整帧回传主世界，不创建GPU查询、不等待GPU，也不改动绘制命令。
//! 【模块所有权】RenderApp监控随树木子域内收，既有F3统计和无GPU读回边界保持不变。

use std::collections::HashSet; // 七个Mesh资产ID转为常数时间匹配集合。

use bevy::core_pipeline::core_3d::{AlphaMask3d, Opaque3d}; // 同时统计不透明树干与真实裁切树冠/公告板阶段。
use bevy::pbr::{Shadow, queue_material_meshes, queue_shadows}; // 阴影阶段及两个官方PBR排队系统提供明确采样顺序。
use bevy::prelude::*; // 引入Res、ResMut和系统调度扩展。
use bevy::render::{
    ExtractSchedule,
    MainWorld,
    Render,
    RenderApp,
    RenderSystems,                        // 安装渲染调度并通过官方入口回写主世界。
    render_phase::ViewBinnedRenderPhases, // 读取公开普通分箱，MDI由共享只读镜像补齐。
};
use feathrise_vegetation::pbr_mdi_monitor::{
    PbrMdiQueueMirror, PbrMdiQueueMirrorParams, PbrPhaseStats,
}; // Bevy 0.19隐藏MDI成员后复用项目只读排队镜像恢复精确树木统计。

use super::resources::{
    TreeRenderMonitorConfig,
    TreeRenderPhaseStats,
    TreeRenderStats, // 提取目标ID并发布阶段快照。
};

/// 为树木插件安装无GPU读回的PBR分箱遥测。
pub(super) fn install_tree_render_monitor(app: &mut App) {
    app.init_resource::<TreeRenderMonitorConfig>() // 非树地图保留空资产ID表。
        .init_resource::<TreeRenderStats>() // 首次完整渲染帧前向F3提供明确未采样状态。
        .add_plugins(bevy::render::extract_resource::ExtractResourcePlugin::<
            TreeRenderMonitorConfig,
        >::default()); // 每帧只向RenderApp复制最多七个AssetId。
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return; // 无渲染子应用的ECS测试无需安装PBR分箱采样。
    }; // 取得独立渲染世界。
    render_app
        .init_resource::<TreeRenderStats>() // RenderApp持有当前完整排队帧，下一帧再回传。
        .add_systems(ExtractSchedule, publish_tree_render_stats) // 帧首发布上一完整Render阶段结果。
        .add_systems(
            Render,
            observe_tree_render_phases
                .in_set(RenderSystems::QueueMeshes)
                .after(queue_material_meshes)
                .after(queue_shadows), // 标准材质与树木扩展材质全部完成主画面和阴影排队后再读取分箱。
        ); // 完成树木渲染遥测调度。
} // 完成遥测插件装配。

/// 把上一完整Render阶段的树木分箱快照复制回主世界。
fn publish_tree_render_stats(
    mut main_world: ResMut<MainWorld>, // Bevy官方RenderApp到主世界回写通道。
    render_stats: Res<TreeRenderStats>, // 上一完整Render阶段已经覆盖的不可变快照。
) {
    let enabled = main_world
        .get_resource::<TreeRenderMonitorConfig>()
        .is_some_and(TreeRenderMonitorConfig::enabled); // 直接读取主世界F3状态，关闭边沿不再多发布一帧旧值。
    if let Some(mut main_stats) = main_world.get_resource_mut::<TreeRenderStats>() {
        *main_stats = if enabled {
            *render_stats // 开启时整份复制上一完整Render阶段结果。
        } else {
            TreeRenderStats::default() // 关闭时立即截断旧样本，禁止普通帧沿用历史证据。
        };
    } // 逻辑测试或插件拆装期间允许主世界资源暂时不存在。
} // 完成上一完整帧发布。

/// 在官方PBR排队完成后统计七个树木Mesh落入各阶段的真实公开分箱。
fn observe_tree_render_phases(
    config: Res<TreeRenderMonitorConfig>, // RenderApp提取到的当前地图七个Mesh身份。
    opaque_phases: Res<ViewBinnedRenderPhases<Opaque3d>>, // Bevy 0.19.1原生绑定路径下树干保持不透明分箱。
    alpha_mask_phases: Res<ViewBinnedRenderPhases<AlphaMask3d>>, // 只统计真实树冠及公告板遮罩分箱。
    shadow_phases: Res<ViewBinnedRenderPhases<Shadow>>,   // 实体树方向光级联及其他灯光阴影分箱。
    mut render_stats: ResMut<TreeRenderStats>,            // 原子式覆盖本轮三个阶段结果。
    mdi: PbrMdiQueueMirrorParams,                         // 只读复现官方主画面与阴影队列门。
    mut mdi_mirror: Local<PbrMdiQueueMirror>,             // 只在树木页可见时重建并复用条目容量。
) {
    if !config.enabled() {
        *render_stats = TreeRenderStats::default(); // 普通游戏帧和其他F3页不保留旧树木样本。
        return; // 不创建资产集合，也不遍历任何PBR视图或可见实体。
    }
    let typed_mesh_ids = config.mesh_ids.iter().copied().collect::<HashSet<_>>(); // MDI重建直接使用强类型Mesh资产身份过滤可见实体。
    let mesh_ids = typed_mesh_ids
        .iter()
        .copied()
        .map(|asset_id| asset_id.untyped())
        .collect::<HashSet<_>>(); // 类型擦除后与Bevy PBR分箱键使用同一身份口径。
    if mesh_ids.is_empty() {
        *render_stats = TreeRenderStats {
            sampled: true,        // 空地图同样完成采样，F3可区分未采样与确实无树。
            ..Default::default()  // 三个阶段保持全零并截断上一地图结果。
        }; // 完成非树地图快照覆盖。
        return; // 没有目标资产时无需扫描任何视图分箱。
    } // 完成空配置快速路径。
    if !mdi_mirror.rebuild(
        &mdi,
        &opaque_phases,
        &alpha_mask_phases,
        &shadow_phases,
        |_, mesh_id| typed_mesh_ids.contains(&mesh_id), // 只镜像七个共享树木Mesh，不承担全场景归类成本。
    ) {
        *render_stats = TreeRenderStats::default();
        return; // 官方队列输入不完整时不得发布部分普通分箱结果。
    } // 空MDI镜像仍是有效采样，随后与公开普通分箱统一汇总。
    *render_stats = TreeRenderStats {
        sampled: true, // 本轮已经在PBR排队完成后读取真实阶段。
        opaque: phase_stats(mdi_mirror.summarize_opaque(
            &opaque_phases,
            |bin_key| mesh_ids.contains(&bin_key.asset_id),
            |_| true,
        )), // 匹配树干等真实Opaque3d普通与MDI分箱。
        alpha_mask: phase_stats(mdi_mirror.summarize_alpha_mask(
            &alpha_mask_phases,
            |bin_key| mesh_ids.contains(&bin_key.asset_id),
            |_| true,
        )), // 匹配树冠和公告板真实AlphaMask3d普通与MDI分箱。
        shadow: phase_stats(mdi_mirror.summarize_shadow(
            &shadow_phases,
            |bin_key| mesh_ids.contains(&bin_key.asset_id),
            |_| true,
        )), // 匹配全部灯光阴影视图普通与MDI分箱。
    }; // 一次覆盖完整PBR树木排队快照。
} // 完成当前Render阶段观察。

/// 把共享PBR统计值投影为树木F3既有公开快照类型。
fn phase_stats(stats: PbrPhaseStats) -> TreeRenderPhaseStats {
    TreeRenderPhaseStats {
        views: stats.views,             // 主相机与阴影子视图保持旧版去重口径。
        draw_groups: stats.draw_groups, // MDI批集、普通批次和独立实体保持统一提交组口径。
        bins: stats.bins,               // 只累计七个目标Mesh对应的唯一官方分箱。
        instances: stats.instances,     // 跨视图累计真实目标实体数。
    }
}

#[cfg(test)] // 纯资源行为测试不启动窗口或RenderApp。
mod tests {
    use super::*; // 复用公共监控配置。

    #[test] // 重复句柄不能扩大树木监控资产集合。
    fn render_monitor_config_deduplicates_mesh_handles() {
        let handle = Handle::<Mesh>::default(); // 默认UUID句柄无需Assets存储且具有稳定身份。
        let mut config = TreeRenderMonitorConfig::default(); // 从空地图配置开始。
        assert!(!config.enabled()); // 普通游戏默认不得扫描PBR分箱。
        assert!(config.set_enabled(true)); // 打开树木页形成真实授权边沿。
        assert!(!config.set_enabled(true)); // 稳态重复授权不得伪报变化。
        assert!(config.enabled()); // 配置必须保持当前树木页授权。
        config.replace_from_handles([&handle, &handle]); // 模拟两个LOD错误复用同一Mesh。
        assert_eq!(config.mesh_ids, vec![handle.id()]); // RenderApp只能收到一个唯一资产身份。
        config.clear(); // 模拟离开树木地图。
        assert!(config.mesh_ids.is_empty()); // 离场必须截断上一地图目标。
        assert!(config.enabled()); // 地图资产清理不能擅自改变F3页面授权。
        assert!(config.set_enabled(false)); // 关闭F3形成撤销边沿。
        assert!(!config.enabled()); // 下一Extract帧必须停止扫描并清除快照。
    } // 完成配置去重与清理验证。
} // 结束树木渲染遥测测试。
