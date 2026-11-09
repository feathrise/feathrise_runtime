//! F3分页资源监控面板的实体创建。
//! 本模块只创建标题、两级页签、共享LOD控件和十份互斥正文；输入、采样与报告构建由其他子模块负责。

use feathrise_ui_widgets::UiInteraction;
use bevy::prelude::*; // 引入Commands、UI组件、字体和颜色类型。
use bevy::text::LineHeight; // Bevy 0.18未从prelude导出行高组件。
use bevy::ui::FocusPolicy; // 根和正文允许输入穿透，只有页签按钮消费鼠标。

use super::helpers::{
    DIAGNOSTIC_FONT_SIZE, DIAGNOSTIC_LINE_HEIGHT, resource_monitor_content_node,
    resource_monitor_panel_node, resource_monitor_subtab_node, resource_monitor_tab_node,
    resource_monitor_tabs_node, resource_monitor_text_node,
}; // 生产与结构测试共享同一分页布局构造器。
#[cfg(test)] // 根标记只在无窗口结构测试中直接查询。
use super::lod_controls::ResourceMonitorLodControls; // 验证总览页不会留下LOD控件布局高度。
use super::lod_controls::spawn_lod_controls; // 九类内容页共享同一组四档调试控件。
use crate::components::{
    ResourceMonitorGroupButton, ResourceMonitorPageText, ResourceMonitorPanel,
    ResourceMonitorSubTabRow, ResourceMonitorTabButton,
}; // 标记根、顶层组、二级页签和十份互斥正文。
use crate::resources::{FpsDisplayFont, ResourceMonitorGroup, ResourceMonitorPage}; // 固定页序和应用提供的字体共同驱动UI生成。

/// 创建默认隐藏的F3分页资源监控面板。
pub(super) fn spawn_resource_monitor_panel(
    mut commands: Commands,         // 创建面板根、页签按钮和正文实体。
    font: Res<FpsDisplayFont>, // 与FPS共用消费应用的字体，公共诊断不选择应用资源目录。
) {
    spawn_resource_monitor_panel_with_font(&mut commands, font.0.clone()); // 使用可测试helper创建固定分页层级。
} // 完成生产面板创建入口。

/// 使用已取得的字体句柄创建固定两级分页层级。
fn spawn_resource_monitor_panel_with_font(
    commands: &mut Commands, // 向当前ECS世界创建面板及全部父子节点。
    font: Handle<Font>,      // 标题、两级按钮和十份正文共享同一字体句柄。
) {
    commands
        .spawn((
            ResourceMonitorPanel,          // 标记根节点供F3显隐查询。
            resource_monitor_panel_node(), // 使用受视口宽高约束的右上布局。
            BackgroundColor(Color::srgba(0.035, 0.030, 0.024, 0.88)), // 使用水墨铜木强化底色。
            BorderColor::all(Color::srgba(0.745, 0.529, 0.243, 0.56)), // 使用规范铜金细边。
            Visibility::Hidden,            // 启动时隐藏，由F3显式打开。
            FocusPolicy::Pass,             // 根节点不拦截鼠标，页签按钮自行消费点击。
            Pickable::IGNORE,              // 现代拾取不读取 FocusPolicy，根与装饰均显式透传。
            GlobalZIndex(260),             // 位于普通HUD与右上角FPS之上。
        ))
        .with_children(|panel| {
            panel.spawn((
                Text::new("资源监控 [F3] · 0.5s刷新 · 选择分页查看"), // 明示分页后一次只查看一个功能域。
                TextFont {
                    font: font.clone().into(), // 复用项目统一中文字体。
                    font_size: 14.0.into(),    // 标题略大于诊断正文。
                    ..default()                // 其余字体属性使用默认值。
                },
                TextColor(Color::srgba(0.922, 0.886, 0.804, 0.94)), // 使用规范米白主文字。
                TextShadow {
                    offset: Vec2::new(1.0, 1.0), // 一像素阴影避免文字融入亮色画面。
                    color: Color::srgba(0.0, 0.0, 0.0, 0.75), // 深色阴影强化字形轮廓。
                },
                LineHeight::RelativeToFont(DIAGNOSTIC_LINE_HEIGHT), // 标题沿用紧凑行高。
                FocusPolicy::Pass,                                  // 标题不截获鼠标。
                Pickable::IGNORE,
            )); // 完成标题创建。
            panel
                .spawn((resource_monitor_tabs_node(), FocusPolicy::Pass, Pickable::IGNORE)) // 横向页签容器不自行截获输入。
                .with_children(|tabs| {
                    for group in ResourceMonitorGroup::ALL {
                        let selected = group == ResourceMonitorGroup::Overview; // 默认总览组在首次显示前已经具有选中样式。
                        tabs.spawn((
                            UiInteraction::None, // 保留监视器的悬停与按压交互。
                            ResourceMonitorGroupButton { group }, // 点击后切换顶层功能组。
                            resource_monitor_tab_node(),          // 五个按钮使用一致尺寸。
                            BackgroundColor(if selected {
                                Color::srgba(0.38, 0.24, 0.10, 0.94) // 默认总览使用选中深铜底。
                            } else {
                                Color::srgba(0.055, 0.044, 0.035, 0.86) // 其他页保持暗墨木色。
                            }),
                            BorderColor::all(if selected {
                                Color::srgba(0.88, 0.66, 0.31, 0.92) // 默认总览使用明铜金边。
                            } else {
                                Color::srgba(0.52, 0.38, 0.19, 0.52) // 未选中页使用弱铜边。
                            }),
                            FocusPolicy::Block, // 只有页签按钮消费鼠标点击，避免点透游戏世界。
                        ))
                        .with_child((
                            Text::new(group.label()), // 使用顶层组资源的唯一中文标签。
                            TextFont {
                                font: font.clone().into(), // 五个按钮共享项目中文字体。
                                font_size: 13.0.into(),    // 页签略大于正文便于快速扫描。
                                ..default()                // 其余字体属性使用默认值。
                            },
                            TextColor(Color::srgba(0.94, 0.88, 0.73, 0.96)), // 铜木底上的浅米金文字。
                            FocusPolicy::Pass, // 按钮文字不建立第二个鼠标消费层。
                            Pickable::IGNORE,
                        )); // 完成当前分页按钮和标签。
                    } // 完成五个固定分页按钮创建。
                }); // 完成顶部功能组行。
            let mut sub_tabs_node = resource_monitor_tabs_node(); // 二级页签继续横向紧凑排列。
            sub_tabs_node.display = Display::None; // 默认总览没有二级页，整行完全退出布局。
            panel
                .spawn((ResourceMonitorSubTabRow, sub_tabs_node, FocusPolicy::Pass, Pickable::IGNORE)) // 二级容器只在作物或景观物组显示。
                .with_children(|tabs| {
                    for page in ResourceMonitorPage::SUBTABS {
                        tabs.spawn((
                            UiInteraction::None, // 保留监视器的悬停与按压交互。
                            ResourceMonitorTabButton { page }, // 点击后切换当前组内叶子页。
                            resource_monitor_subtab_node(false), // 默认总览组下全部二级按钮退出布局。
                            BackgroundColor(Color::srgba(0.055, 0.044, 0.035, 0.86)), // 未选中二级页使用暗墨木色。
                            BorderColor::all(Color::srgba(0.52, 0.38, 0.19, 0.52)), // 未选中二级页使用弱铜边。
                            FocusPolicy::Block, // 二级按钮消费鼠标点击，避免点透世界。
                        ))
                        .with_child((
                            Text::new(page.label()), // 使用叶子页唯一中文标签。
                            TextFont {
                                font: font.clone().into(), // 八个二级按钮共享项目中文字体。
                                font_size: 12.0.into(),    // 二级标签略小于顶层组。
                                ..default()                // 其余字体属性使用默认值。
                            },
                            TextColor(Color::srgba(0.94, 0.88, 0.73, 0.96)), // 暗铜底上的浅米金文字。
                            FocusPolicy::Pass, // 按钮文字不建立第二个鼠标消费层。
                            Pickable::IGNORE,
                        )); // 完成当前二级按钮和标签。
                    } // 完成七个故道植被、作物与景观物叶子按钮。
                }); // 完成二级页签行。
            spawn_lod_controls(panel, &font); // 在页签与正文之间创建默认隐藏、切页复用的四档性能隔离控件。
            panel
                .spawn((
                    resource_monitor_content_node(), // 当前页独占整行正文宽度。
                    BackgroundColor(Color::srgba(0.044, 0.036, 0.030, 0.64)), // 使用规范暗墨半透明分区底。
                    BorderColor::all(Color::srgba(0.520, 0.380, 0.190, 0.38)), // 使用规范弱铜细边。
                    FocusPolicy::Pass, // 正文容器不截获鼠标。
                    Pickable::IGNORE,
                ))
                .with_children(|content| {
                    for page in ResourceMonitorPage::ALL {
                        content.spawn((
                            ResourceMonitorPageText { page }, // 标记当前文字所属分页。
                            Text::new(format!("{}\n等待刷新...", page.label())), // 首次打开或切页前提供明确状态。
                            TextFont {
                                font: font.clone().into(),              // 十份正文共享项目中文字体。
                                font_size: DIAGNOSTIC_FONT_SIZE.into(), // 保持1080p技术报告可读。
                                ..default()                             // 其余字体属性使用默认值。
                            },
                            TextColor(Color::srgba(0.922, 0.886, 0.804, 0.94)), // 使用规范米白主文字。
                            TextShadow {
                                offset: Vec2::new(1.0, 1.0),              // 一像素阴影保证亮场景可读性。
                                color: Color::srgba(0.0, 0.0, 0.0, 0.75), // 深色阴影强化字形轮廓。
                            },
                            TextLayout::new(Justify::Left, LineBreak::AnyCharacter), // 中文长行按字符安全换行。
                            LineHeight::RelativeToFont(DIAGNOSTIC_LINE_HEIGHT), // 紧凑行高控制当前页总高度。
                            resource_monitor_text_node(page == ResourceMonitorPage::Overview), // 默认仅总览参与布局，其他页使用Display::None彻底退出排版。
                            FocusPolicy::Pass, // 报告文字不截获鼠标。
                            Pickable::IGNORE,
                        )); // 完成当前分页正文实体。
                    } // 完成十份互斥正文创建。
                }); // 完成统一正文分区。
        }); // 完成F3分页面板层级。
} // 完成可测试面板创建helper。

#[cfg(test)] // UI结构测试以空World创建生产层级，不启动窗口或渲染器。
mod tests {
    use super::*; // 复用生产创建helper和私有标记组件。
    use bevy::ecs::system::RunSystemOnce; // 通过一次性系统提供Commands并自动应用延迟命令。

    #[test] // 验证五组、五二级页、八正文及默认唯一可见页完整存在。
    fn panel_spawns_hierarchy_and_one_visible_page() {
        let mut world = World::new(); // 创建不含窗口与渲染插件的最小ECS世界。
        world
            .run_system_once(|mut commands: Commands| {
                spawn_resource_monitor_panel_with_font(&mut commands, Handle::<Font>::default()); // 默认字体句柄足以验证UI层级。
            })
            .expect("F3分页面板应能在最小World中创建"); // 创建失败代表UI组件约束回归。
        let group_count = world
            .query_filtered::<(), With<ResourceMonitorGroupButton>>()
            .iter(&world)
            .count(); // 统计生产顶层组按钮。
        let subtab_count = world
            .query_filtered::<(), With<ResourceMonitorTabButton>>()
            .iter(&world)
            .count(); // 统计生产二级页签实体。
        let subtab_row_display = world
            .query_filtered::<&Node, With<ResourceMonitorSubTabRow>>()
            .single(&world)
            .expect("F3 必须只有一个二级页签行")
            .display; // 默认总览组必须让完整二级行退出布局。
        let lod_controls_display = world
            .query_filtered::<&Node, With<ResourceMonitorLodControls>>()
            .single(&world)
            .expect("F3必须只有一组共享LOD控件")
            .display; // 默认总览页必须让LOD控件完全退出布局。
        let mut page_query = world.query::<(&ResourceMonitorPageText, &Node)>(); // 读取十份正文身份和默认布局状态。
        let pages = page_query.iter(&world).collect::<Vec<_>>(); // 冻结结构样本便于多项断言。
        assert_eq!(group_count, ResourceMonitorGroup::ALL.len()); // 顶部必须正好五个功能组。
        assert_eq!(subtab_count, ResourceMonitorPage::SUBTABS.len()); // 二级必须正好八个叶子页签。
        assert_eq!(pages.len(), ResourceMonitorPage::ALL.len()); // 每页必须正好一份独立正文。
        assert_eq!(subtab_row_display, Display::None); // 默认总览不得留下空白二级行高度。
        assert_eq!(lod_controls_display, Display::None); // 默认总览不得留下空白LOD控件高度。
        assert_eq!(
            pages
                .iter()
                .filter(|(_, node)| node.display != Display::None)
                .count(),
            1,
        ); // 默认只能有总览一页占用布局高度。
        assert!(pages.iter().any(|(page, node)| {
            page.page == ResourceMonitorPage::Overview && node.display == Display::Flex
        })); // 默认唯一可见正文必须是总览。
    } // 完成分页结构回归验证。
} // 结束UI结构测试模块。
