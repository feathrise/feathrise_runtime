//! 开箱即用的专业数值属性框：一次生成标签、进度填充、数值、单位和完整输入模型。
//!
//! 【功能设计】业务只提供稳定marker、初值、数值规格和拖动规格；公共控件统一拥有
//! 左键拖实际值、右键拖独立灵敏度、中键进入精确输入的视觉与命中结构。

use bevy::prelude::*; // 构建公共UI实体、文本、颜色和字体组件。
use bevy::ui::{FocusPolicy, OverrideClip, RelativeCursorPosition}; // 提示越过窄框裁切，子视觉仍穿透命中。

use crate::components::{
    ButtonColors,
    NumericInputConfig,
    NumericInputDisplayText,
    NumericInputEditState,
    NumericInputFieldButton,
    NumericInputProgressFill,
    NumericInputRoot,
    NumericInputScrubConfig,
    NumericInputScrubSensitivityText,
    NumericInputScrubValueText,
    NumericInputValue, // 一次挂齐公共数字输入模型、拖动配置和自动进度关联。
};
use crate::helpers::{format_numeric_value, normalize_numeric_value}; // 初始显示与后续系统使用同一格式口径。
use crate::types::NumericScrubFieldPalette; // 上层 UI 库注入独立色板。
use crate::types::{NumericInputSpec, NumericValue}; // 调用方声明正式数值类型、边界和初值。

/// 专业属性框的紧凑视觉尺寸；业务可覆盖布局，但无需重建交互层级。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumericScrubFieldStyle {
    pub width: f32,           // 整个属性框宽度，默认适合F12单行工具栏。
    pub height: f32,          // 整个属性框高度。
    pub border_radius: f32,   // 统一背景和进度填充裁剪圆角。
    pub column_gap: f32,      // 标签、数值和单位之间的水平间隔。
    pub label_font_size: f32, // 左侧属性名字号。
    pub value_font_size: f32, // 中间精确数值字号。
    pub unit_font_size: f32,  // 右侧单位字号。
}

impl Default for NumericScrubFieldStyle {
    fn default() -> Self {
        Self {
            width: 120.0,         // 与既有地图编辑器紧凑属性框宽度一致。
            height: 20.0,         // 保持单行工具栏高度不增长。
            border_radius: 2.0,   // 小圆角适合密集作者工具。
            column_gap: 2.0,      // 信息紧凑但不粘连。
            label_font_size: 9.0, // 中文短标签在120像素内保持可读。
            value_font_size: 9.5, // 数值略强于标签形成视觉焦点。
            unit_font_size: 8.5,  // 单位作为辅助信息降低层级。
        }
    }
}

/// 一次生成专业数值框所需的全部业务参数。
#[derive(Clone, Debug, PartialEq)]
pub struct NumericScrubFieldOptions {
    pub label: String,                  // 属性框左侧稳定业务名称。
    pub unit: String,                   // 数字右侧显示单位，可传空字符串。
    pub value: NumericValue,            // 首帧业务初值，会先经过输入规格规范化。
    pub input: NumericInputSpec,        // 精确输入类型、范围、小数位和边界语义。
    pub scrub: NumericInputScrubConfig, // 左右拖动映射和字段独立灵敏度。
    pub style: NumericScrubFieldStyle,  // 可选紧凑布局覆盖。
}

impl NumericScrubFieldOptions {
    /// 使用公共默认视觉创建完整属性框描述。
    pub fn new(
        label: impl Into<String>,       // 左侧属性名。
        unit: impl Into<String>,        // 右侧显示单位。
        value: NumericValue,            // 正式业务初值。
        input: NumericInputSpec,        // 精确输入规格。
        scrub: NumericInputScrubConfig, // 左右拖动规格。
    ) -> Self {
        Self {
            label: label.into(), // 在生成前拥有文本，避免借用调用侧临时字符串。
            unit: unit.into(),   // 空单位同样保留统一层级。
            value,               // 规范化延迟到生成时统一执行。
            input,               // 保留业务正式范围与类型。
            scrub,               // 保留当前字段独立灵敏度。
            style: NumericScrubFieldStyle::default(), // 默认即可直接投入地图编辑器。
        }
    }

    /// 覆盖视觉尺寸但继续复用同一交互和消息协议。
    pub const fn with_style(mut self, style: NumericScrubFieldStyle) -> Self {
        self.style = style; // 只改变布局，不改变数值语义。
        self // 支持调用侧链式构建。
    }
}

/// 生成完整专业数值属性框并返回稳定业务根实体。
pub fn spawn_numeric_scrub_field_with_palette<M: Component>(
    parent: &mut ChildSpawnerCommands, // 新属性框所属的Bevy UI父节点。
    marker: M,                         // 业务层用于识别Changed/Committed消息的稳定marker。
    font: Handle<Font>,                // 标签、数值和单位共享的已加载字体。
    mut options: NumericScrubFieldOptions, // 全部业务数值和视觉参数。
    palette: NumericScrubFieldPalette, // 两类 UI 使用同一实体结构、分别注入主题。
) -> Entity {
    options.input.show_step_buttons = false; // 专业属性框通过拖动调整，不生成另一套加减按钮。
    let value = normalize_numeric_value(options.value, options.input); // 首帧模型立即满足范围和周期语义。
    let display = format_numeric_value(value, options.input); // 数值文字与后续更新使用完全相同格式。
    let style = options.style; // 复制轻量样式后允许把字符串移入子实体。
    let mut root = parent.spawn((
        marker,                            // 业务marker直接挂在返回根，消息实体无需二次映射。
        NumericInputRoot,                  // 声明公共数值输入业务根。
        NumericInputValue(value),          // 保存规范化首帧值。
        NumericInputConfig(options.input), // 保存精确输入和边界规格。
        NumericInputEditState::new(value, options.input.replace_on_first_key), // 初始化字符编辑事务状态。
        palette.input,                     // 焦点刷新沿用字段自身的主题。
        options.scrub,                     // 启用左值、右灵敏度和中键精确输入专业手势。
        Button,                            // 复用公共字段交互与视觉系统。
        Interaction::default(),            // 首帧从无交互状态开始。
        RelativeCursorPosition::default(), // 水平相对坐标驱动与帧率无关的总位移拖动。
        FocusPolicy::Block,                // 整个属性框消费鼠标，不能点透至游戏视口。
        Node {
            width: Val::Px(style.width),             // 使用调用方或默认紧凑宽度。
            height: Val::Px(style.height),           // 使用调用方或默认单行高度。
            flex_shrink: 0.0,                        // 工具栏拥挤时不压扁数值命中区。
            align_items: AlignItems::Center,         // 三段文字垂直居中。
            justify_content: JustifyContent::Center, // 标签、值和单位作为整体居中。
            column_gap: Val::Px(style.column_gap),   // 保持三段文字清晰分隔。
            border: UiRect::all(Val::Px(1.0)),       // 一像素铜色边界表达可交互性。
            border_radius: BorderRadius::all(Val::Px(style.border_radius)), // 与紧凑工具栏统一圆角。
            overflow: Overflow::clip(), // 进度填充不会越过圆角边界。
            ..default()                 // 其余布局使用Bevy安全默认值。
        },
        BackgroundColor(palette.input.normal_bg), // 默认显示低对比度输入背景。
        BorderColor::all(palette.input.normal_border), // 默认显示弱铜色边界。
        ButtonColors::new(
            palette.input.normal_bg,  // 无交互背景。
            palette.input.hovered_bg, // 悬停时轻度强化。
            palette.input.active_bg,  // 按压时明确反馈。
        ),
    ));
    let root_entity = root.id(); // 子填充和数字文字稳定关联同一业务根。
    root.insert(NumericInputFieldButton { root: root_entity }); // 整个一体式属性框作为公共输入命中区。
    root.with_children(|field| {
        field.spawn((
            NumericInputProgressFill { root: root_entity }, // 公共系统自动同步当前值进度。
            FocusPolicy::Pass,                              // 绝对底色不能截获根的三种鼠标手势。
            Node {
                position_type: PositionType::Absolute, // 填充位于文字下方且不参与横向布局。
                left: Val::Px(0.0),                    // 从属性框左边缘开始增长。
                top: Val::Px(0.0),                     // 完整覆盖属性框内部高度。
                width: Val::Percent(0.0),              // 公共同步系统在首个Update写入正式比例。
                height: Val::Percent(100.0),           // 作为低对比度整高背景。
                ..default()                            // 其余定位属性保持默认。
            },
            BackgroundColor(palette.fill), // 用低对比度选中铜色表达范围位置。
        ));
        field.spawn((
            Text::new(options.label), // 左侧显示业务属性名。
            FocusPolicy::Pass,        // 文字命中穿透给整个属性框。
            TextFont {
                font: font.clone().into(),               // 与调用侧UI共享字体句柄。
                font_size: style.label_font_size.into(), // 标签使用较低视觉层级字号。
                ..default()                              // 字体平滑等属性使用项目默认值。
            },
            TextColor(palette.label), // 标签弱于正式数值。
        ));
        field.spawn((
            NumericInputDisplayText { root: root_entity }, // 精确输入和拖动只更新这一段数字。
            NumericInputScrubValueText { root: root_entity }, // 右拖倍率另用鼠标旁提示，业务值和原精度继续可见。
            Text::new(display),                               // 首帧立即显示规范化业务值。
            FocusPolicy::Pass,                                // 数值文字不截获中键或左右拖动。
            TextFont {
                font: font.clone().into(),               // 复用同一字体资产。
                font_size: style.value_font_size.into(), // 数值略大形成中心焦点。
                ..default()                              // 保持字体默认渲染设置。
            },
            TextColor(palette.value), // 数值使用主要文本颜色确保可读。
            palette.shadow,           // 进度填充经过文字下方时仍保持清晰。
        ));
        if !options.unit.is_empty() {
            field.spawn((
                NumericInputScrubValueText { root: root_entity }, // 倍率反馈期间仍保留米、角度等业务单位。
                Text::new(options.unit), // 只在非空时生成单位，避免无意义布局间隔。
                FocusPolicy::Pass,       // 单位文字同样穿透给属性框根。
                TextFont {
                    font: font.clone().into(), // 临时倍率反馈继续共享同一字体资产。
                    font_size: style.unit_font_size.into(), // 单位使用最小辅助字号。
                    ..default()                // 其余字体设置保持默认。
                },
                TextColor(palette.unit), // 单位使用提示文本色降低层级。
            ));
        }
        field.spawn((
            NumericInputScrubSensitivityText { root: root_entity }, // 复用原提示实体，不为每次拖动创建浮层或改写业务值。
            Text::default(),
            TextLayout::no_wrap(),
            OverrideClip, // LOD和XYZ窄框及其滚动祖先不能裁掉鼠标旁的实时倍率。
            GlobalZIndex(951), // 高于参数/设置及帮助，低于加载和切图遮罩；只在数值捕获期间可见。
            Pickable::IGNORE,
            FocusPolicy::Pass,
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                // 按字号为最长的“灵敏度 100.00×”预留独立宽度，不受原数值框宽度约束。
                width: Val::Px(style.value_font_size * 10.0 + 16.0),
                flex_shrink: 0.0,
                padding: UiRect::axes(Val::Px(7.0), Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(style.border_radius)),
                ..default()
            }, // 绝对定位不占布局，原字段、相邻轴和LOD行的尺寸始终不变。
            BackgroundColor(palette.input.normal_bg),
            BorderColor::all(palette.input.normal_border), // 沿用主题弱分隔色，不出现亮白外框。
            TextFont {
                font: font.into(),
                font_size: style.value_font_size.into(),
                ..default()
            },
            TextColor(palette.value),
            palette.shadow,
        ));
    });
    root_entity // 调用方可直接插入禁用组件或保存稳定字段身份。
}
