//! F3分页资源监控面板的纯布局配置。
//! 本文件集中宽度、最大高度、页签和正文Node构造，生产系统与无窗口测试使用完全相同的布局。

use bevy::prelude::*; // 引入Node、Val、UiRect和FlexDirection等Bevy UI布局类型。

#[cfg(test)] // 目标视口宽度只服务确定性布局测试，不进入运行时二进制。
pub(super) const TARGET_VIEWPORT_WIDTH: f32 = 1_920.0; // 以用户指定的1920像素宽验收面板不横向超屏。
#[cfg(test)] // 目标视口高度只服务确定性布局测试，不进入运行时二进制。
pub(super) const TARGET_VIEWPORT_HEIGHT: f32 = 1_080.0; // 以用户指定的1080像素高验收分页面板不纵向堆叠全部功能域。
pub(super) const PANEL_WIDTH: f32 = 1_760.0; // 宽页保留长GPU诊断的横向可读性。
pub(super) const PANEL_TOP: f32 = 28.0; // 避开右上角常驻FPS文字并沿用原面板垂直起点。
pub(super) const PANEL_RIGHT: f32 = 10.0; // 保留原有右侧十像素安全边距。
pub(super) const PANEL_MAX_WIDTH_PERCENT: f32 = 96.0; // 较窄视口时把面板限制在视口宽度内。
pub(super) const PANEL_MAX_HEIGHT_PERCENT: f32 = 92.0; // 分页正文最多占视口九成二，防止内容重新顶出屏幕。
pub(super) const PANEL_PADDING: f32 = 8.0; // 紧凑内边距降低总高度且保留边框呼吸空间。
pub(super) const PANEL_GAP: f32 = 6.0; // 标题、页签和正文之间使用统一紧凑间距。
pub(super) const CONTENT_PADDING: f32 = 8.0; // 正文与铜金边框之间保留稳定安全距离。
pub(super) const TAB_GAP: f32 = 6.0; // 四个页签之间保持清晰可点击间隔。
pub(super) const DIAGNOSTIC_FONT_SIZE: f32 = 12.0; // 1080p下保持长技术数据可读且不易折行。
pub(super) const DIAGNOSTIC_LINE_HEIGHT: f32 = 1.1; // 紧凑行高避免当前单页内部出现无意义纵向膨胀。

/// 构造固定右上定位且受视口宽高约束的面板根节点。
pub(super) fn resource_monitor_panel_node() -> Node {
    Node {
        position_type: PositionType::Absolute, // 脱离普通HUD流式布局并固定到视口右上角。
        top: Val::Px(PANEL_TOP),               // 避开顶部常驻FPS文字。
        right: Val::Px(PANEL_RIGHT),           // 与右侧屏幕边缘保留稳定间距。
        width: Val::Px(PANEL_WIDTH),           // 1920宽下为长GPU字段提供主要横向空间。
        max_width: Val::Percent(PANEL_MAX_WIDTH_PERCENT), // 小视口自动收缩，禁止横向越界。
        max_height: Val::Percent(PANEL_MAX_HEIGHT_PERCENT), // 单页内容也不能无限向下扩张。
        padding: UiRect::all(Val::Px(PANEL_PADDING)), // 为标题、页签和正文提供统一外层留白。
        border: UiRect::all(Val::Px(1.0)),     // 一像素铜金边框保持武侠UI语言。
        flex_direction: FlexDirection::Column, // 标题、页签和当前正文从上到下排列。
        row_gap: Val::Px(PANEL_GAP),           // 三段之间保持稳定紧凑间距。
        overflow: Overflow::clip_y(),          // 异常超长报告被限制在屏幕内而不是覆盖底部HUD。
        ..default()                            // 其余布局属性使用Bevy稳定默认值。
    } // 完成分页面板根节点。
} // 返回受视口约束的根布局。

/// 构造四个分页按钮的横向容器。
pub(super) fn resource_monitor_tabs_node() -> Node {
    Node {
        width: Val::Percent(100.0),         // 页签横向使用根面板全部可用宽度。
        flex_direction: FlexDirection::Row, // 四个功能域固定从左到右排列。
        column_gap: Val::Px(TAB_GAP),       // 按钮之间保留可识别间隔。
        ..default()                         // 其余布局使用默认值。
    } // 完成页签容器节点。
} // 返回横向页签布局。

/// 构造单个固定宽度分页按钮。
pub(super) fn resource_monitor_tab_node() -> Node {
    Node {
        min_width: Val::Px(0.0), // 顶层四组在窄视口中允许等比例收缩。
        flex_grow: 1.0,          // 五个顶层功能组等分整行宽度。
        height: Val::Px(28.0),   // 保持足够鼠标命中区域且不浪费纵向空间。
        padding: UiRect::horizontal(Val::Px(10.0)), // 文字与左右边框保持呼吸空间。
        border: UiRect::all(Val::Px(1.0)), // 选中和未选中页签都保留铜金轮廓。
        justify_content: JustifyContent::Center, // 标签水平居中。
        align_items: AlignItems::Center, // 标签垂直居中。
        ..default()              // 其余按钮布局使用默认值。
    } // 完成分页按钮节点。
} // 返回统一按钮布局。

/// 构造单个二级叶子页签按钮。
pub(super) fn resource_monitor_subtab_node(is_visible: bool) -> Node {
    Node {
        width: Val::Px(132.0), // 作物与景观物短中文标签使用稳定宽度。
        height: Val::Px(26.0), // 二级按钮略低于顶层组，层级清晰且保持紧凑。
        padding: UiRect::horizontal(Val::Px(10.0)), // 文字与边框保留稳定命中空间。
        border: UiRect::all(Val::Px(1.0)), // 选中和未选中二级页都保留弱铜轮廓。
        justify_content: JustifyContent::Center, // 标签水平居中。
        align_items: AlignItems::Center, // 标签垂直居中。
        display: resource_monitor_page_display(is_visible), // 非当前组二级按钮完全退出布局。
        ..default()            // 其余布局使用 Bevy 稳定默认值。
    } // 完成二级页签按钮节点。
} // 返回二级按钮布局。

/// 构造只承载当前分页正文的统一分区。
pub(super) fn resource_monitor_content_node() -> Node {
    Node {
        width: Val::Percent(100.0), // 正文独占面板可用宽度，不再与其他功能域并排或堆叠。
        min_width: Val::Px(0.0),    // 小视口允许正文合法收缩。
        padding: UiRect::all(Val::Px(CONTENT_PADDING)), // 文字与弱铜边框保持安全距离。
        border: UiRect::all(Val::Px(1.0)), // 统一正文分区边界。
        flex_direction: FlexDirection::Column, // 当前唯一可见文字自然决定内容高度。
        ..default()                 // 其余布局使用默认值。
    } // 完成正文容器节点。
} // 返回分页正文分区布局。

/// 根据当前分页身份返回正文是否参与Bevy UI布局。
pub(super) fn resource_monitor_page_display(is_selected: bool) -> Display {
    if is_selected {
        Display::Flex // 当前页参与布局，因此始终从正文容器的起点排版。
    } else {
        Display::None // 非当前页必须退出布局；Visibility::Hidden只隐藏绘制，仍会累加页面高度。
    } // 完成分页布局状态选择。
} // 返回当前正文的Display状态。

/// 构造占满正文分区宽度的报告文字节点。
pub(super) fn resource_monitor_text_node(is_selected: bool) -> Node {
    Node {
        width: Val::Percent(100.0), // 中文长行严格使用正文全部可用宽度。
        display: resource_monitor_page_display(is_selected), // 只有当前页参与布局，消除A/B/C页高度依次累加。
        ..default() // 其余布局属性保持默认，文字自身决定当前页实际高度。
    } // 完成报告文字节点。
} // 返回分页共用文字布局。

#[cfg(test)] // 纯布局测试不启动渲染器即可锁定1080p边界与分页方向。
mod tests {
    use super::*; // 复用生产常量和纯Node构造函数。

    #[test] // 验证根面板同时具有横纵视口保护。
    fn panel_layout_fits_target_1080p_viewport() {
        let panel = resource_monitor_panel_node(); // 使用生产根节点配置进行断言。
        assert_eq!(panel.width, Val::Px(PANEL_WIDTH)); // 固定宽度必须与宽屏诊断设计一致。
        assert_eq!(panel.max_width, Val::Percent(PANEL_MAX_WIDTH_PERCENT)); // 小视口必须保留横向百分比防护。
        assert_eq!(panel.max_height, Val::Percent(PANEL_MAX_HEIGHT_PERCENT)); // 纵向必须限制为单屏可见范围。
        let Val::Px(panel_width) = panel.width else {
            panic!("资源监控面板宽度必须使用像素值");
        };
        let Val::Px(panel_right) = panel.right else {
            panic!("资源监控面板右边距必须使用像素值");
        };
        let Val::Px(panel_top) = panel.top else {
            panic!("资源监控面板顶部偏移必须使用像素值");
        };
        assert!(panel_width + panel_right < TARGET_VIEWPORT_WIDTH); // 1920宽下不得触碰左侧屏幕边界。
        assert!(panel_top < TARGET_VIEWPORT_HEIGHT); // 面板垂直起点必须落在1080高视口内。
    } // 完成目标视口边界验证。

    #[test] // 验证五个顶层组横向排列且正文恢复单列全宽。
    fn tabs_are_horizontal_and_content_uses_full_width() {
        let tabs = resource_monitor_tabs_node(); // 读取生产页签容器。
        let content = resource_monitor_content_node(); // 读取生产正文容器。
        assert_eq!(tabs.flex_direction, FlexDirection::Row); // 页签不得纵向继续拉长面板。
        assert_eq!(content.width, Val::Percent(100.0)); // 当前页必须独占全部正文宽度。
    } // 完成分页布局策略验证。

    #[test] // 锁定非当前页完全退出布局，防止再次误用Visibility::Hidden造成分页高度累加。
    fn inactive_page_uses_display_none() {
        assert_eq!(resource_monitor_text_node(true).display, Display::Flex,); // 当前页必须从正文容器起点参与排版。
        assert_eq!(resource_monitor_text_node(false).display, Display::None,); // 非当前页必须不占任何布局高度。
    } // 完成分页Display策略回归验证。
} // 结束纯布局测试模块。
