//! F3 四档 LOD 控件的水墨铜木界面、输入与视觉同步。

use bevy::prelude::*; // 引入UI实体创建、交互查询、颜色和文本类型。
use bevy::ui::FocusPolicy; // 只有LOD按钮消费鼠标，容器和文字允许输入穿透。

use super::state::{ALL_LODS_ENABLED, LOD_LEVEL_COUNT, LodControlState, content_for_page}; // 界面把F3页面转换为共享内容身份，并只向用户显示可读中文状态。
use crate::resources::ResourceMonitorState; // 当前分页决定标签和操作所属内容。

/// F3 LOD 控件根容器。
#[derive(Component)]
pub(in crate::resource_monitor) struct ResourceMonitorLodControls; // 总览页使用Display::None让整块控件退出布局。

/// 单个可点击 LOD 操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LodControlAction {
    Toggle(usize), // 切换当前页指定档位的开启或关闭状态。
    Solo(usize),   // 只开启当前页指定档位并关闭同类其他三档。
    EnableAll,     // 恢复当前页四档全部开启。
} // 完成不向用户暴露位掩码的操作集合。

/// LOD 操作按钮身份。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::resource_monitor) struct LodControlButton {
    action: LodControlAction, // 当前按钮执行切换、单独查看或全部开启。
} // 完成按钮与行为绑定。

/// LOD 操作按钮文字身份。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::resource_monitor) struct LodControlButtonLabel {
    action: LodControlAction, // 标签与父按钮共享行为，切页后可刷新成对应档位名称。
} // 完成按钮标签绑定。

type LodControlInteractionData = (&'static LodControlButton, &'static Interaction);
type ChangedLodControlInteractionFilter = Changed<Interaction>;
type LodControlVisualData = (
    &'static LodControlButton,
    &'static Interaction,
    &'static mut BackgroundColor,
    &'static mut BorderColor,
);
type LodControlLabelData = (&'static LodControlButtonLabel, &'static mut Text);

/// 在二级页签与正文之间创建两行紧凑 LOD 控件。
pub(in crate::resource_monitor) fn spawn_lod_controls(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
) {
    parent
        .spawn((
            ResourceMonitorLodControls, // 标记根节点供切页时显示或隐藏。
            Node {
                width: Val::Percent(100.0),            // 控件横向使用F3面板全部可用宽度。
                min_width: Val::Px(0.0),               // 小视口允许控件合法收缩。
                padding: UiRect::all(Val::Px(5.0)),    // 与铜色细边保留紧凑内边距。
                border: UiRect::all(Val::Px(1.0)),     // 使用水墨UI一致的一像素分区边界。
                flex_direction: FlexDirection::Column, // 档位开关和单独查看分成两行。
                row_gap: Val::Px(4.0),                 // 两行保持紧凑且可清晰区分。
                display: Display::None,                // 默认总览页不占用任何布局高度。
                ..default()                            // 其余布局使用Bevy稳定默认值。
            },
            BackgroundColor(Color::srgba(0.044, 0.036, 0.030, 0.72)), // 使用正文同系暗墨半透明底。
            BorderColor::all(Color::srgba(0.520, 0.380, 0.190, 0.44)), // 使用弱铜细边区分诊断控件。
            FocusPolicy::Pass,                                        // 根容器不消费鼠标。
        ))
        .with_children(|controls| {
            spawn_control_row(
                controls,                 // 第一行挂到控件根节点。
                font,                     // 复用F3中文字体。
                "档位开关",               // 明确第一行按钮可独立开关。
                true,                     // 第一行额外提供全部开启。
                LodControlAction::Toggle, // 四个档位按钮执行独立切换。
            ); // 完成档位开关行。
            spawn_control_row(
                controls,               // 第二行挂到控件根节点。
                font,                   // 复用F3中文字体。
                "单独查看",             // 明确第二行用于性能瓶颈隔离。
                false,                  // 单独查看行不重复创建全部开启。
                LodControlAction::Solo, // 四个按钮只保留对应档位。
            ); // 完成单独查看行。
        }); // 完成LOD控件层级。
} // 完成LOD控件创建入口。

/// 创建一行标题与四个档位操作按钮。
fn spawn_control_row(
    parent: &mut ChildSpawnerCommands, // 接收当前LOD控件根的子节点创建器。
    font: &Handle<Font>,               // 行标题和按钮共享F3中文字体。
    title: &'static str,               // 当前行显示“档位开关”或“单独查看”。
    include_enable_all: bool,          // 只有开关行需要一键全部恢复按钮。
    action_for_level: fn(usize) -> LodControlAction, // 把零至三档转换为本行操作。
) {
    parent
        .spawn((
            Node {
                width: Val::Percent(100.0),         // 当前行使用完整控件宽度。
                flex_direction: FlexDirection::Row, // 标题和操作按钮从左到右排列。
                align_items: AlignItems::Center,    // 文字和按钮垂直居中。
                column_gap: Val::Px(5.0),           // 保留清晰鼠标命中间隔。
                ..default()                         // 其余布局使用默认值。
            },
            FocusPolicy::Pass, // 行容器不消费鼠标。
        ))
        .with_children(|row| {
            row.spawn((
                Text::new(title), // 显示当前行的短中文用途。
                TextFont {
                    font: font.clone().into(), // 复用项目中文字体。
                    font_size: 11.0.into(),    // 控件辅助标题略小于页签和正文。
                    ..default()                // 其余字体属性使用默认值。
                },
                TextColor(Color::srgba(0.83, 0.74, 0.58, 0.94)), // 使用低一层级的米铜辅助文字。
                Node {
                    width: Val::Px(66.0), // 两个四字标题使用相同固定宽度对齐按钮。
                    ..default()           // 其余文字布局保持默认。
                },
                FocusPolicy::Pass, // 行标题不消费鼠标。
            )); // 完成行标题。
            for level in 0..LOD_LEVEL_COUNT {
                spawn_action_button(row, font, action_for_level(level)); // 当前档位绑定本行确定性行为。
            } // 完成四档按钮创建。
            if include_enable_all {
                spawn_action_button(row, font, LodControlAction::EnableAll); // 在四档开关后提供一键恢复。
            } // 开关行完成恢复入口。
        }); // 完成当前LOD控制行。
} // 完成一行控件创建。

/// 创建一个带水墨铜木样式的 LOD 操作按钮。
fn spawn_action_button(
    parent: &mut ChildSpawnerCommands, // 接收当前控制行的子节点创建器。
    font: &Handle<Font>,               // 按钮标签使用F3统一中文字体。
    action: LodControlAction,          // 按钮绑定的确定性调试行为。
) {
    let button_width = if action == LodControlAction::EnableAll {
        104.0 // 全部开启文字较短但保留明确命中区域。
    } else {
        112.0 // 档位名称加状态或“只看”前缀需要更宽空间。
    }; // 完成按行为选择稳定按钮宽度。
    parent
        .spawn((
            Interaction::None, // 保留监视器的悬停与按压交互。
            LodControlButton { action }, // 保存当前按钮行为供输入系统读取。
            Node {
                width: Val::Px(button_width), // 使用稳定宽度避免状态文字切换时抖动。
                height: Val::Px(24.0),        // 保持可点击高度且不拉长F3面板。
                padding: UiRect::horizontal(Val::Px(7.0)), // 文字与边框保留紧凑呼吸空间。
                border: UiRect::all(Val::Px(1.0)), // 开关状态始终保留清晰铜边。
                justify_content: JustifyContent::Center, // 标签水平居中。
                align_items: AlignItems::Center, // 标签垂直居中。
                ..default()                   // 其余按钮布局使用默认值。
            },
            BackgroundColor(Color::srgba(0.055, 0.044, 0.035, 0.90)), // 初始使用暗墨木色，首帧同步真实状态。
            BorderColor::all(Color::srgba(0.52, 0.38, 0.19, 0.56)),   // 初始使用弱铜边。
            FocusPolicy::Block, // 操作按钮消费鼠标，避免点击穿透到游戏世界。
        ))
        .with_child((
            LodControlButtonLabel { action }, // 标签和父按钮绑定同一行为。
            Text::new("等待同步"),            // 首帧由当前页名称和状态替换。
            TextFont {
                font: font.clone().into(), // 复用项目统一中文字体。
                font_size: 11.0.into(),    // 控件标签使用紧凑可读字号。
                ..default()                // 其余字体属性使用默认值。
            },
            TextColor(Color::srgba(0.94, 0.88, 0.73, 0.96)), // 暗铜底上的浅米金文字。
            FocusPolicy::Pass,                               // 标签不建立第二个鼠标消费层。
        )); // 完成LOD操作按钮与标签。
} // 完成单按钮创建。

/// 响应当前内容页的 LOD 控件点击。
pub(in crate::resource_monitor) fn handle_lod_control_interactions(
    mut monitor_state: ResMut<ResourceMonitorState>, // 读取当前页并在操作后请求立即刷新性能报告。
    mut lod_state: ResMut<LodControlState>,          // 保存九类内容独立且不持久化的四档开关。
    button_q: Query<LodControlInteractionData, ChangedLodControlInteractionFilter>, // 只读取本帧发生变化的LOD按钮。
) {
    let Some(content) = content_for_page(monitor_state.page) else {
        return; // 总览页没有可操作的LOD内容。
    }; // 固定当前页面对应的共享内容身份。
    if !monitor_state.visible {
        return; // 面板隐藏或总览页不处理任何LOD输入。
    } // 完成输入快速返回。
    let mut changed = false; // 本帧没有按下操作时不触发资源变更或报告刷新。
    for (button, interaction) in &button_q {
        if *interaction != Interaction::Pressed {
            continue; // 悬停和释放变化只用于视觉反馈，不修改LOD状态。
        } // 完成按下过滤。
        match button.action {
            LodControlAction::Toggle(level) => lod_state.toggle(content, level), // 独立翻转一档。
            LodControlAction::Solo(level) => lod_state.solo(content, level),     // 只保留指定档。
            LodControlAction::EnableAll => lod_state.enable_all(content), // 恢复当前内容四档。
        } // 完成当前按钮行为。
        changed = true; // 标记需要立即刷新报告并同步真实渲染状态。
    } // 完成本帧按钮变化扫描。
    if changed {
        monitor_state.force_refresh = true; // 性能隔离操作后不等待下一个半秒周期才更新正文。
    } // 完成报告立即刷新请求。
} // 完成LOD输入系统。

/// 同步当前页的 LOD 控件显隐、标签和水墨铜木交互样式。
pub(in crate::resource_monitor) fn update_lod_control_ui(
    monitor_state: Res<ResourceMonitorState>, // 当前页决定控件是否显示及四档中文名称。
    lod_state: Res<LodControlState>,          // 当前页独立掩码决定开关文字和选中样式。
    mut root_q: Query<&mut Node, With<ResourceMonitorLodControls>>, // 总览页让完整控件退出布局。
    mut button_q: Query<LodControlVisualData>, // 读取按钮操作、悬停及两类视觉颜色。
    mut label_q: Query<LodControlLabelData>,  // 切页后更新四档名称与开关状态。
) {
    let content = content_for_page(monitor_state.page); // 总览页没有共享LOD内容。
    let supported = content.is_some(); // 只有九个内容页显示两行控件。
    for mut node in &mut root_q {
        node.display = if supported {
            Display::Flex // 六个内容页显示两行LOD调试控件。
        } else {
            Display::None // 总览页彻底移除控件布局高度。
        }; // 完成控件根布局显隐。
    } // 同步唯一控件根节点。
    if !supported {
        return; // 总览页无需格式化隐藏按钮标签或样式。
    } // 完成总览快速返回。
    let content = content.expect("支持LOD的F3页面必须映射到共享内容身份"); // 上方显隐判断已经排除总览页。
    let current_mask = lod_state.mask(content); // 冻结当前页四档状态供九个按钮复用。
    for (button, interaction, mut background, mut border) in &mut button_q {
        let selected = match button.action {
            LodControlAction::Toggle(level) => {
                current_mask & (1_u8 << level.min(LOD_LEVEL_COUNT - 1)) != 0
            } // 档位开关开启时使用选中样式。
            LodControlAction::Solo(level) => current_mask == 1_u8 << level.min(LOD_LEVEL_COUNT - 1), // 当前确实只开启一档时突出对应单独查看按钮。
            LodControlAction::EnableAll => current_mask == ALL_LODS_ENABLED, // 四档全开时突出恢复按钮。
        }; // 完成按钮选中状态推导。
        *background = BackgroundColor(if selected {
            Color::srgba(0.38, 0.24, 0.10, 0.94) // 开启或当前模式使用深铜底。
        } else if *interaction == Interaction::Hovered {
            Color::srgba(0.22, 0.16, 0.09, 0.90) // 未选中悬停按钮轻微提亮。
        } else {
            Color::srgba(0.055, 0.044, 0.035, 0.90) // 关闭或非当前模式使用暗墨木色。
        }); // 完成按钮背景同步。
        *border = BorderColor::all(if selected {
            Color::srgba(0.88, 0.66, 0.31, 0.92) // 选中状态使用明铜金边。
        } else {
            Color::srgba(0.52, 0.38, 0.19, 0.56) // 未选中状态使用弱铜边。
        }); // 完成按钮边框同步。
    } // 完成九个按钮样式同步。
    for (label, mut text) in &mut label_q {
        let next_text = match label.action {
            LodControlAction::Toggle(level) => format!(
                "{}：{}",
                content.level_label(level), // 使用叶片或Mesh对应的四档名称。
                if lod_state.is_enabled(content, level) {
                    "开" // 当前档位进入正式渲染。
                } else {
                    "关" // 当前档位完全退出相应渲染路径。
                },
            ), // 完成独立开关标签。
            LodControlAction::Solo(level) => {
                format!("只看{}", content.level_label(level))
            } // 完成单档隔离标签。
            LodControlAction::EnableAll => "全部开启".to_string(), // 恢复按钮使用固定用户可读文字。
        }; // 完成当前标签字符串构建。
        if text.as_str() != next_text {
            **text = next_text; // 只有内容变化时写Text，避免无意义触发UI文本重排。
        } // 完成标签差异写回。
    } // 完成九个按钮标签同步。
} // 完成LOD控件视觉同步系统。
