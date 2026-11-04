//! F3 资源监控功能域。
//!
//! 本模块把面板生命周期、ECS 资源采样、草地派生诊断和最终文本格式化集中管理，
//! 防止通用 FPS 系统再次演变成难以审查的超长文件。

mod clipmap; // 实际几何与构建/上传计数按固定预算采样，隐藏时停止。
mod blade_identity; // 预设层身份用于F3内容筛选与LOD控制。
mod grass;
mod helpers;
mod mesh; // 树木、灌木和岩石统一读取数据预设及正式批次。
mod lod_controls; // 九类植物与景观物四档开关、单档隔离和真实渲染同步。
mod application;
mod queries;
pub use application::{ResourceMonitorApplicationReport, ResourceMonitorRefresh, ResourceMonitorSet, resource_monitor_overview_refresh};
mod report;
mod systems;
mod terrain_surface; // 地表阶段、完整纹理组、管线、承接实体和Opaque证据独立于草地统计。
mod ui; // 标题、五个顶层组、八个二级页签和十份互斥正文的单一 UI 创建职责。

pub use lod_controls::{ALL_LODS_ENABLED, LOD_LEVEL_COUNT, LodControlContent, LodControlState, LodControlPresetBindings, LodControlApplySet}; // crate根只经本功能域公开F3/F4共享逐档API，内部系统和页面继续保持私有。

use bevy::prelude::{App, IntoScheduleConfigs, Startup, Timer, TimerMode, Update}; // 功能域自行注册资源和有序系统，不向crate根暴露私有SystemParam类型。

use crate::resources::{ResourceMonitorRefreshTimer, ResourceMonitorState}; // 半秒刷新计时器和F3显隐状态仍沿用原稳定资源类型。

/// 向 FPS 插件安装完整 F3 资源监控功能域。
pub(crate) fn install_resource_monitor(app: &mut App) {
    app.insert_resource(ResourceMonitorRefreshTimer(Timer::from_seconds(
        0.5,                  // 保持原半秒采样周期，避免诊断面板逐帧扫描全部资源。
        TimerMode::Repeating, // 面板显示期间持续低频刷新。
    )))
    .init_resource::<ResourceMonitorRefresh>()
    .init_resource::<ResourceMonitorApplicationReport>()
    .configure_sets(Update, (ResourceMonitorSet::Prepare, ResourceMonitorSet::Collect, ResourceMonitorSet::Present).chain())
    .init_resource::<ResourceMonitorState>() // 默认隐藏面板并保留首次显示强制刷新语义。
    .init_resource::<clipmap::ClipmapMonitorState>()
    .init_resource::<lod_controls::LodControlState>() // 九类内容默认四档全开，状态只保留到本次进程结束。
    .add_systems(Startup, ui::spawn_resource_monitor_panel) // 启动阶段只创建一次两级分页 F3 面板。
    .add_systems(
        Update,
        (
            systems::toggle_resource_monitor_panel, // 先处理F3显隐边沿并设置强制刷新。
            systems::select_resource_monitor_page,  // 再处理顶部页签点击并隐藏其余九页。
            systems::synchronize_mesh_render_monitor, // 只有可见树木/灌木/岩石页授权下一RenderApp帧精确采样。
            lod_controls::handle_lod_control_interactions, // 接收当前内容页的独立开关、单档隔离或全部恢复操作。
            lod_controls::update_lod_control_ui, // 同步当前内容的四档名称、开关文字与水墨铜木样式。
            clipmap::sample_clipmaps,
            application::prepare_refresh, // 唯一半秒节流入口，供应用采样与最终报告共用。
        )
            .chain().in_set(ResourceMonitorSet::Prepare),
    ); // 完成Update输入、旧路径同步、UI与低频报告链。
    app.add_systems(bevy::prelude::PostUpdate, (
        lod_controls::apply_changed_lod_controls,
        lod_controls::apply_lod_controls_to_new_nodes,
    ).chain().in_set(LodControlApplySet)
        .before(bevy::prelude::TransformSystems::Propagate)
        .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate));
    app.add_systems(Update, systems::update_resource_monitor_panel.in_set(ResourceMonitorSet::Present));
    lod_controls::install_managed_mesh_lod_controls(app); // 通用树木/景观根必须在生成当帧、可见性传播前应用F3状态。
} // 完成功能域安装入口。
