//! F3分页资源监控的输入、显隐与低频刷新系统。
//! UI实体创建位于`ui`模块；本文件只决定当前页，并且每个半秒周期只采样、格式化该页。

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin}; // 总览与植被验收读取同一平滑帧诊断。
use bevy::ecs::system::SystemParam; // 把四页可选采样依赖组合成具名参数集。
use bevy::prelude::*; // 引入输入、查询、颜色和资源类型。
use feathrise_environment::terrain_surface::TerrainSurfaceRenderStats; // 地表诊断独立读取阶段、完整纹理组、管线和Opaque快照。
use feathrise_environment::tree::TreeRenderMonitorConfig;
use feathrise_environment::vegetation::blade::{
    GpuBladeGpuReadbackStats, GpuBladeGpuTimingStats, GpuBladeInteraction,
    GpuBladeRenderStats, GpuBladeStreamingStats,
}; // 普通草和麦田分页复用同一GPU叶片后端但按profile独立格式化。
use feathrise_environment::vegetation::{
    VegetationBladeAdoptionStatus, VegetationMeshRenderMonitorConfig, VegetationMeshRenderStats,
}; // F3分别读取叶片采用状态与通用Mesh精确RenderApp分箱。
use super::lod_controls::{LodControlContent, LodControlPresetBindings};
use feathrise_map_content::VegetationPresetRegistry;


use super::grass::{GrassMonitorQueries, VegetationReportSources, build_vegetation_report}; // 当前页只传入一种内容身份，并共享同一批GPU与流送证据。
use super::helpers::resource_monitor_page_display; // 非当前页使用Display::None彻底退出布局，避免分页高度累加。
use super::mesh::{MeshMonitorQueries, build_mesh_report}; // 通用批次与逐实例共享正式原型身份。
use super::queries::{ResourceMonitorAssets, ResourceMonitorQueries}; // 总览采样及树木可见几何所需Mesh资产。
use super::report::format_overview_report; // 总览格式器不依赖任何植被字段。
use super::terrain_surface::{
    // 叶片分页只嵌入已经独立构建的一行地表报告。
    TerrainSurfaceMonitorQueries,
    build_terrain_surface_report,
};
use crate::components::{
    ResourceMonitorGroupButton, ResourceMonitorPageText, ResourceMonitorPanel,
    ResourceMonitorSubTabRow, ResourceMonitorTabButton,
}; // 两级分页身份决定按钮样式、正文布局参与状态和写回目标。
use crate::resources::{ResourceMonitorGroup, ResourceMonitorPage, ResourceMonitorState}; // 当前组、叶子页和半秒刷新状态由同一功能域资源持有。

type ResourceMonitorGroupButtonData = (
    &'static ResourceMonitorGroupButton,
    &'static Interaction,
    &'static mut BackgroundColor,
    &'static mut BorderColor,
    &'static mut Node,
);
type ResourceMonitorGroupButtonFilter = (With<Button>, Without<ResourceMonitorTabButton>);
type ResourceMonitorTabButtonData = (
    &'static ResourceMonitorTabButton,
    &'static Interaction,
    &'static mut BackgroundColor,
    &'static mut BorderColor,
    &'static mut Node,
);
type ResourceMonitorTabButtonFilter = (With<Button>, Without<ResourceMonitorGroupButton>);
type ResourceMonitorSubTabRowFilter = (
    With<ResourceMonitorSubTabRow>,
    Without<ResourceMonitorGroupButton>,
    Without<ResourceMonitorTabButton>,
    Without<ResourceMonitorPageText>,
);
type ResourceMonitorPageNodeData = (&'static ResourceMonitorPageText, &'static mut Node);
type ResourceMonitorPageNodeFilter = (
    Without<ResourceMonitorGroupButton>,
    Without<ResourceMonitorSubTabRow>,
    Without<ResourceMonitorTabButton>,
);

/// 固定诊断槽只在消费应用显式绑定时开放，公共工程不会看见旧游戏占位分类。
fn page_available(page: ResourceMonitorPage, bindings: Option<&LodControlPresetBindings>) -> bool {
    super::lod_controls::content_for_page(page).is_none_or(|content| {
        bindings.and_then(|bindings| bindings.profile(content)).is_some()
    })
}

fn first_available_page(group: ResourceMonitorGroup, bindings: Option<&LodControlPresetBindings>) -> Option<ResourceMonitorPage> {
    let default = group.default_page();
    if page_available(default, bindings) { return Some(default); }
    ResourceMonitorPage::ALL.into_iter().find(|page| page.group() == group && page_available(*page, bindings))
}

/// F3当前页一次刷新可能使用的全部资源、查询与唯一输出集合。
#[derive(SystemParam)]
pub(super) struct ResourceMonitorPanelParams<'w, 's> {
    refresh: Res<'w, super::application::ResourceMonitorRefresh>,
    application: Res<'w, super::application::ResourceMonitorApplicationReport>,
    state: Res<'w, ResourceMonitorState>,
    clipmap: Res<'w, super::clipmap::ClipmapMonitorState>, // 已完整分批采样的短报告，刷新时不扫描实例。
    diagnostics: Res<'w, DiagnosticsStore>,         // 总览和草麦验收读取同一平滑FPS及帧耗。
    queries: ResourceMonitorQueries<'w, 's>,        // 仅总览页调用snapshot遍历通用世界。
    assets: ResourceMonitorAssets<'w>,              // 总览读取资产数量，树木页读取可见Mesh几何。
    grass_queries: GrassMonitorQueries<'w, 's>,     // 仅普通草或麦田页遍历对应窗口。
    terrain_surface_queries: TerrainSurfaceMonitorQueries<'w, 's>, // 地表承接实体与草窗查询职责分离。
    terrain_surface_stats: Option<Res<'w, TerrainSurfaceRenderStats>>, // 上一完整地表RenderApp快照。
    grass_render_stats: Option<Res<'w, GpuBladeRenderStats>>, // 上一完整RenderApp草地流水线快照。
    grass_gpu_stats: Option<Res<'w, GpuBladeGpuReadbackStats>>, // 三缓冲异步实例统计快照。
    grass_gpu_timing: Option<Res<'w, GpuBladeGpuTimingStats>>, // 时间戳与管线统计快照。
    grass_interaction: Option<Res<'w, GpuBladeInteraction>>,  // 主世界三层风场与踩踏源快照。
    grass_streaming: Option<Res<'w, GpuBladeStreamingStats>>, // 阶段5预测、淡入与常驻资产生命周期快照。
    blade_adoption: Option<Res<'w, VegetationBladeAdoptionStatus>>, // 当前版本可见窗口采用阶段和分子/分母。
    mesh_queries: MeshMonitorQueries<'w>, // 只读当前Mesh页对应的原型及已有统计。
    preset_bindings: Option<Res<'w, LodControlPresetBindings>>, // 应用显式绑定；未绑定不猜测游戏素材。
    vegetation_presets: Option<Res<'w, VegetationPresetRegistry>>, // 按当前目录解析稳定素材身份。
    mesh_render_stats: Option<Res<'w, VegetationMeshRenderStats>>, // 独立监控存在时读取按层、原型、LOD和部件分组的候选分箱。
    text_q: Query<'w, 's, (&'static mut Text, &'static ResourceMonitorPageText)>, // 只写当前分页对应的唯一正文。
} // 完成分页刷新参数分组。

/// 响应F3并同步面板显隐；重新打开时强制立即刷新当前页。
pub(super) fn toggle_resource_monitor_panel(
    keyboard: Res<ButtonInput<KeyCode>>,     // 读取本帧键盘边沿输入。
    mut state: ResMut<ResourceMonitorState>, // 保存面板显隐和强制刷新标志。
    mut panel_q: Query<&mut Visibility, With<ResourceMonitorPanel>>, // 更新唯一资源监控根节点。
) {
    if !keyboard.just_pressed(KeyCode::F3) {
        return; // 非F3边沿帧不触碰状态与UI。
    } // 完成输入过滤。
    state.visible = !state.visible; // 切换资源监控逻辑显隐状态。
    state.force_refresh = state.visible; // 打开时无视半秒周期立即生成当前页报告。
    for mut visibility in &mut panel_q {
        *visibility = if state.visible {
            Visibility::Visible // 打开时显示标题、页签与当前正文。
        } else {
            Visibility::Hidden // 关闭时保留实体和各页最后报告。
        }; // 完成根节点显隐选择。
    } // 同步全部资源监控根节点。
} // 完成F3显隐系统。

/// 处理顶层组与二级页签点击，并同步十份正文显隐。
pub(super) fn select_resource_monitor_page(
    mut state: ResMut<ResourceMonitorState>, // 读取面板显隐并写入用户选择组和叶子页。
    bindings: Option<Res<LodControlPresetBindings>>,
    mut group_q: Query<ResourceMonitorGroupButtonData, ResourceMonitorGroupButtonFilter>, // 顶层组按钮样式查询与二级页签严格不相交。
    mut tab_q: Query<ResourceMonitorTabButtonData, ResourceMonitorTabButtonFilter>, // 二级页签额外写Display并与顶层组严格不相交。
    mut subtab_row_q: Query<&mut Node, ResourceMonitorSubTabRowFilter>, // 总览与普通草隐藏完整二级行。
    mut page_q: Query<ResourceMonitorPageNodeData, ResourceMonitorPageNodeFilter>, // 保证同时只有一页正文参与 Bevy UI 布局。
) {
    if !state.visible {
        return; // 面板隐藏时不扫描两级按钮或正文实体。
    } // 完成隐藏快速返回。
    if !page_available(state.page, bindings.as_deref()) {
        state.page = first_available_page(state.group, bindings.as_deref()).unwrap_or(ResourceMonitorPage::Overview);
        state.group = state.page.group();
        state.force_refresh = true;
    }
    let mut requested_group = None; // 本帧最多接受一个顶层组按下。
    for (button, interaction, _, _, _) in &mut group_q {
        if *interaction == Interaction::Pressed && first_available_page(button.group, bindings.as_deref()).is_some() {
            requested_group = Some(button.group); // 保存用户点击组，下一段统一切换默认叶子页。
        } // 悬停只影响样式。
    } // 完成五个顶层组输入扫描。
    if let Some(group) = requested_group
        && group != state.group
    {
        state.group = group; // 切换到用户选择的顶层功能组。
        state.page = first_available_page(group, bindings.as_deref()).expect("仅可用组接受点击");
        state.force_refresh = true; // 新组不等待半秒周期。
    } // 完成顶层组状态更新。
    let mut requested_page = None; // 本帧最多接受一个鼠标按下页签。
    for (tab, interaction, _, _, _) in &mut tab_q {
        if *interaction == Interaction::Pressed && tab.page.group() == state.group && page_available(tab.page, bindings.as_deref()) {
            requested_page = Some(tab.page); // 保存用户点击页，下一段统一更新资源状态。
        } // 非当前组隐藏按钮不能切换正文。
    } // 完成当前组二级页签输入扫描。
    if let Some(page) = requested_page
        && page != state.page
    {
        state.page = page; // 切换到用户选择的独立功能域。
        state.force_refresh = true; // 新页不等待半秒周期，立即生成首份报告。
    } // 完成分页状态更新。
    for (button, interaction, mut background, mut border, mut node) in &mut group_q {
        let display = resource_monitor_page_display(first_available_page(button.group, bindings.as_deref()).is_some());
        if node.display != display { node.display = display; }
        let selected = button.group == state.group; // 当前顶层组样式优先于鼠标悬停。
        let target_background = BackgroundColor(if selected {
            Color::srgba(0.38, 0.24, 0.10, 0.94) // 选中组使用深铜底色。
        } else if *interaction == Interaction::Hovered {
            Color::srgba(0.22, 0.16, 0.09, 0.90) // 未选中悬停组轻微提亮。
        } else {
            Color::srgba(0.055, 0.044, 0.035, 0.86) // 普通组保持暗墨木色。
        }); // 完成当前组按钮背景。
        let target_border = BorderColor::all(if selected {
            Color::srgba(0.88, 0.66, 0.31, 0.92) // 选中组使用明铜金边。
        } else {
            Color::srgba(0.52, 0.38, 0.19, 0.52) // 未选中组使用弱铜边。
        }); // 完成当前组按钮边框。
        if *background != target_background {
            *background = target_background; // 稳态页签不重复污染背景色变更检测。
        }
        if *border != target_border {
            *border = target_border; // 稳态页签不重复触发布局/绘制提取。
        }
    } // 同步全部五个顶层组视觉状态。
    for (tab, interaction, mut background, mut border, mut node) in &mut tab_q {
        let belongs_to_group = tab.page.group() == state.group && page_available(tab.page, bindings.as_deref());
        let target_display = resource_monitor_page_display(belongs_to_group);
        if node.display != target_display {
            node.display = target_display; // 其他组二级按钮完全退出布局。
        }
        let selected = tab.page == state.page; // 当前页样式优先于鼠标悬停。
        let target_background = BackgroundColor(if selected {
            Color::srgba(0.38, 0.24, 0.10, 0.94) // 选中页使用清晰但克制的深铜底色。
        } else if *interaction == Interaction::Hovered {
            Color::srgba(0.22, 0.16, 0.09, 0.90) // 未选中悬停页轻微提亮。
        } else {
            Color::srgba(0.055, 0.044, 0.035, 0.86) // 普通页签保持暗墨木色。
        }); // 完成当前按钮背景样式。
        let target_border = BorderColor::all(if selected {
            Color::srgba(0.88, 0.66, 0.31, 0.92) // 选中页使用明铜金边。
        } else {
            Color::srgba(0.52, 0.38, 0.19, 0.52) // 未选中页使用弱铜边。
        }); // 完成当前按钮边框样式。
        if *background != target_background {
            *background = target_background;
        }
        if *border != target_border {
            *border = target_border;
        }
    } // 同步全部八个二级页签视觉与布局状态。
    for mut node in &mut subtab_row_q {
        let target_display = resource_monitor_page_display(state.group.has_subtabs());
        if node.display != target_display {
            node.display = target_display; // 单页顶层组不占第二行高度。
        }
    } // 同步唯一二级页签行布局状态。
    for (page_text, mut node) in &mut page_q {
        let target_display = resource_monitor_page_display(page_text.page == state.page);
        if node.display != target_display {
            node.display = target_display; // 非当前页退出布局，确保A/B/C/D正文都从同一顶部起点开始。
        }
    } // 同步全部四个分页正文。
} // 完成页签输入与UI同步系统。

/// 把F3当前可见页同步为通用Mesh精确RenderApp监控的唯一生产授权。
pub(super) fn synchronize_mesh_render_monitor(
    state: Res<ResourceMonitorState>, // F3显隐与当前叶子页共同决定是否确实需要精确分箱。
    mesh_config: Option<ResMut<VegetationMeshRenderMonitorConfig>>, // 极简FPS应用未安装通用监控插件时保持安全无操作。
    tree_config: Option<ResMut<TreeRenderMonitorConfig>>, // TreePlugin未组合时不隐式创建旧树木渲染职责。
) {
    let mesh_enabled = state.needs_mesh_render_monitor(); // 树、灌木与岩石页共享通用精确分箱。
    if let Some(mut config) = mesh_config
        && config.enabled() != mesh_enabled
    {
        config.set_enabled(mesh_enabled); // 只在真实边沿取得可写引用，稳态不污染资源变更检测。
    }
    let tree_enabled = state.needs_tree_render_monitor(); // 七Mesh兼容汇总只服务树木页。
    if let Some(mut config) = tree_config
        && config.enabled() != tree_enabled
    {
        config.set_enabled(tree_enabled); // 关闭F3或切页后下一Extract立即停止树木阶段扫描。
    }
}

/// 在低频周期只采样当前页并写回其唯一正文。
pub(super) fn update_resource_monitor_panel(
    params: ResourceMonitorPanelParams, // 具名参数组保持四个功能域依赖可审查。
) {
    let ResourceMonitorPanelParams {
        refresh, application, state,
        clipmap,
        diagnostics,             // 解构Bevy诊断存储。
        queries,                 // 解构通用ECS查询集合。
        assets,                  // 解构通用资产集合。
        grass_queries,           // 解构草麦窗口查询。
        terrain_surface_queries, // 解构独立地表承接查询。
        terrain_surface_stats,   // 解构独立地表生命周期与渲染快照。
        grass_render_stats,      // 解构可选草地流水线快照。
        grass_gpu_stats,         // 解构可选草地实例回读。
        grass_gpu_timing,        // 解构可选草地GPU查询。
        grass_interaction,       // 解构可选风场与踩踏源。
        grass_streaming,         // 解构可选流送状态。
        blade_adoption,          // 解构可选解析密度采用快照。
        mesh_queries,
        vegetation_presets,
        preset_bindings,
        mesh_render_stats,       // 解构可选通用Mesh精确分箱。
        mut text_q,              // 解构九个分页正文写回查询。
    } = params; // 后续只进入当前页对应分支。
    if !refresh.due { return; } // 与应用采样使用同一显隐、分页及半秒门。
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|diagnostic| diagnostic.smoothed())
        .unwrap_or_default(); // 诊断尚未产出样本时以零显示。
    let frame_ms = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .and_then(|diagnostic| diagnostic.smoothed())
        .unwrap_or_default(); // 草麦验收与总览共享同一平滑CPU帧耗。
    let terrain_report = build_terrain_surface_report(
        // 每次可见刷新只遍历零至两个地表承接标记。
        &terrain_surface_queries,         // 读取主世界承接实体数。
        terrain_surface_stats.as_deref(), // 读取上一完整RenderApp帧与主世界阶段。
    ); // 完成独立地表报告构建。
    let grass_sources = VegetationReportSources {
        presets: vegetation_presets.as_deref(),
        render_stats: grass_render_stats.as_deref(),
        gpu_stats: grass_gpu_stats.as_deref(),
        gpu_timing: grass_gpu_timing.as_deref(),
        interaction: grass_interaction.as_deref(),
        streaming: grass_streaming.as_deref(),
        adoption: blade_adoption.as_deref(),
        terrain_report: &terrain_report, // 地表行不再从GpuBladeRenderStats读取任何字段。
        frame_ms,
    }; // 六个叶片页在同一刷新周期内消费同一批诊断快照，只有内容身份不同。
    let profile = |content| preset_bindings.as_deref().and_then(|bindings| bindings.profile(content));
    let report = match state.page {
        ResourceMonitorPage::Overview => {
            let snapshot = queries.snapshot(&assets); // 只有总览页遍历通用世界并统计全部资产数量。
            let mut report = format_overview_report(fps, frame_ms, &application.0, &snapshot);
            report.push_str(&clipmap.report);
            report
        }
        ResourceMonitorPage::OrdinaryGrass => build_vegetation_report(
            profile(LodControlContent::OrdinaryGrass).unwrap_or(""), "普通草", &grass_queries, &grass_sources,
        ),
        ResourceMonitorPage::Wheat => build_vegetation_report(
            profile(LodControlContent::Wheat).unwrap_or(""), "麦田", &grass_queries, &grass_sources,
        ),
        ResourceMonitorPage::MixedGrain => build_vegetation_report(
            profile(LodControlContent::MixedGrain).unwrap_or(""), "杂粮田", &grass_queries, &grass_sources,
        ),
        ResourceMonitorPage::WetlandGrass => build_vegetation_report(
            profile(LodControlContent::WetlandGrass).unwrap_or(""), "湿地草", &grass_queries, &grass_sources,
        ),
        ResourceMonitorPage::Reeds => build_vegetation_report(
            profile(LodControlContent::Reeds).unwrap_or(""), "芦苇", &grass_queries, &grass_sources,
        ),
        ResourceMonitorPage::SandbarGrass => build_vegetation_report(
            profile(LodControlContent::SandbarGrass).unwrap_or(""), "沙洲矮草", &grass_queries, &grass_sources,
        ),
        ResourceMonitorPage::Trees => build_mesh_report(
            "树木", profile(LodControlContent::Trees).unwrap_or(""), &mesh_queries, assets.meshes(),
            vegetation_presets.as_deref(), mesh_render_stats.as_deref(),
        ),
        ResourceMonitorPage::Shrubs => build_mesh_report(
            "灌木", profile(LodControlContent::Shrubs).unwrap_or(""), &mesh_queries, assets.meshes(),
            vegetation_presets.as_deref(), mesh_render_stats.as_deref(),
        ),
        ResourceMonitorPage::Rocks => build_mesh_report(
            "岩石", profile(LodControlContent::Rocks).unwrap_or(""), &mesh_queries, assets.meshes(),
            vegetation_presets.as_deref(), mesh_render_stats.as_deref(),
        ),
    }; // 完成当前页唯一报告构建。
    for (mut text, page_text) in &mut text_q {
        if page_text.page == state.page {
            **text = report; // 当前页恰好一个正文实体，移动字符串避免额外克隆。
            break; // 其他七页保持上一次文本且不参与本轮写回。
        } // 非当前页不触碰Text变更检测。
    } // 完成当前页唯一写回。
} // 完成分页低频刷新系统。

#[cfg(test)]
mod page_binding_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn ordinary_projects_offer_only_overview_and_partial_bindings_choose_real_default() {
        assert_eq!(ResourceMonitorPage::ALL.into_iter().filter(|page| page_available(*page, None)).collect::<Vec<_>>(), vec![ResourceMonitorPage::Overview]);
        let mut bindings = LodControlPresetBindings::default();
        bindings.bind(LodControlContent::Rocks, "project_crystal");
        assert_eq!(first_available_page(ResourceMonitorGroup::Landscape, Some(&bindings)), Some(ResourceMonitorPage::Rocks));
        assert_eq!(first_available_page(ResourceMonitorGroup::Crops, Some(&bindings)), None);
        for content in LodControlContent::ALL { bindings.bind(content, format!("project_{content:?}")); }
        assert!(ResourceMonitorPage::ALL.into_iter().all(|page| page_available(page, Some(&bindings))));
    }

    #[test]
    fn removing_binding_hides_old_group_and_releases_active_page() {
        let mut world = World::new();
        world.insert_resource(ResourceMonitorState { visible: true, group: ResourceMonitorGroup::Crops,
            page: ResourceMonitorPage::Wheat, ..Default::default() });
        for group in ResourceMonitorGroup::ALL {
            world.spawn((ResourceMonitorGroupButton { group }, Button, Interaction::None,
                BackgroundColor::default(), BorderColor::default(), Node::default()));
        }
        world.run_system_once(select_resource_monitor_page).expect("动态页签查询不冲突");
        assert_eq!(world.resource::<ResourceMonitorState>().page, ResourceMonitorPage::Overview);
        let visible = world.query::<(&ResourceMonitorGroupButton, &Node)>().iter(&world)
            .filter(|(_, node)| node.display != Display::None).map(|(tab, _)| tab.group).collect::<Vec<_>>();
        assert_eq!(visible, vec![ResourceMonitorGroup::Overview]);
    }
}
