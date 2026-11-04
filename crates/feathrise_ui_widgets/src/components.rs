//! 游戏与编辑器共用的 UI 组件和数值控件契约。

use bevy::prelude::*;

use crate::types::{NumericInputScrubMapping, NumericInputSpec, NumericValue};

/// Three-state button colors reused across UI crates.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct ButtonColors {
    pub normal: Color,
    pub hovered: Color,
    pub pressed: Color,
}

impl ButtonColors {
    pub const fn new(normal: Color, hovered: Color, pressed: Color) -> Self {
        Self {
            normal,
            hovered,
            pressed,
        }
    }
}

/// Marks the business-facing root entity of a numeric input widget.
#[derive(Component)]
pub struct NumericInputRoot;

/// Stores the current normalized numeric value on the root entity.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct NumericInputValue(pub NumericValue);

/// Stores widget behavior and constraints on the root entity.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct NumericInputConfig(pub NumericInputSpec);

/// Per-widget edit state owned by the UI subsystem.
#[derive(Component, Clone, Debug)]
pub struct NumericInputEditState {
    pub editing: bool,
    pub buffer: String,
    pub previous_value: NumericValue,
    pub replace_on_next_input: bool,
    /// 本次聚焦后是否发生过字符或退格编辑；用于区分未编辑失焦与显式重输同值。
    pub edited: bool,
}

impl NumericInputEditState {
    pub fn new(initial_value: NumericValue, replace_on_next_input: bool) -> Self {
        Self {
            editing: false,
            buffer: String::new(),
            previous_value: initial_value,
            replace_on_next_input,
            edited: false,
        }
    }
}

/// Marks the clickable field button that activates editing.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumericInputFieldButton {
    pub root: Entity,
}

/// Marks the child text entity that renders the current display string.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumericInputDisplayText {
    pub root: Entity,
}

/// 标记专业数字框的低对比度进度填充，并稳定指回业务根实体。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumericInputProgressFill {
    pub root: Entity, // 公共同步系统从根读取正式值、范围和线性或指数映射。
}

/// 专业数值框的业务数值和单位；鼠标旁倍率提示不替换这些原始显示。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumericInputScrubValueText {
    pub root: Entity,
}

/// 鼠标旁的临时灵敏度倍率反馈，非右拖期间隐藏，始终不占用字段布局。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumericInputScrubSensitivityText {
    pub root: Entity,
}

/// Direction for a step button.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericStepDirection {
    Decrease,
    Increase,
}

/// Marks a plus/minus button and points back to its root widget.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumericInputStepButton {
    pub root: Entity,
    pub direction: NumericStepDirection,
}

/// When present on the root entity, the widget ignores activation and key input.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NumericInputDisabled;

/// 为公共数字输入增加“中键精确输入、左拖数值、右拖灵敏度”的可选专业编辑行为。
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct NumericInputScrubConfig {
    pub mapping: NumericInputScrubMapping, // 当前字段的线性或指数相对拖动映射。
    pub drag_step: f32,                    // 左拖结果的业务吸附步长，必须为有限正数。
    pub minimum: f32, // 左拖允许到达的最小业务值；精确输入仍服从NumericInputSpec。
    pub maximum: f32, // 左拖允许到达的最大业务值；精确输入仍服从NumericInputSpec。
    pub sensitivity: f32, // 当前字段独立的用户灵敏度倍率。
    pub minimum_sensitivity: f32, // 右拖允许设置的最小倍率。
    pub maximum_sensitivity: f32, // 右拖允许设置的最大倍率。
    pub drag_threshold_pixels: f32, // 左键超过该物理像素数后才发布实际值变化；短按无动作。
    pub sensitivity_pixels_per_octave: f32, // 右拖多少物理像素使灵敏度翻倍或减半。
}

impl NumericInputScrubConfig {
    /// 创建使用项目标准阈值和灵敏度范围的线性专业输入配置。
    pub const fn linear(
        units_per_pixel: f32, // 一倍灵敏度下每物理像素改变的业务单位。
        drag_step: f32,       // 松散连续结果吸附到的业务粒度。
        minimum: f32,         // 左拖和进度填充使用的业务最小值。
        maximum: f32,         // 左拖和进度填充使用的业务最大值。
        sensitivity: f32,     // 从用户设置加载的字段独立倍率。
    ) -> Self {
        Self {
            mapping: NumericInputScrubMapping::Linear { units_per_pixel }, // 固定单位相对拖动。
            drag_step,                           // 保留调用方声明的正式吸附粒度。
            minimum,                             // 保存字段业务下界。
            maximum,                             // 保存字段业务上界。
            sensitivity,                         // 保存本字段当前独立倍率。
            minimum_sensitivity: 0.01,           // 标准最细手感为一百分之一倍。
            maximum_sensitivity: 100.0,          // 标准最快手感限制为一百倍。
            drag_threshold_pixels: 4.0,          // 四像素以内不形成一次实际值拖动。
            sensitivity_pixels_per_octave: 48.0, // 右拖四十八像素使倍率翻倍或减半。
        }
    }

    /// 创建使用项目标准阈值和灵敏度范围的正数指数专业输入配置。
    pub const fn exponential(
        exponent_per_pixel: f32, // 一倍灵敏度下每物理像素使用的自然指数。
        drag_step: f32,          // 指数结果最终吸附的业务粒度。
        minimum: f32,            // 必须为正数的推荐业务最小值。
        maximum: f32,            // 跨数量级业务最大值。
        sensitivity: f32,        // 从用户设置加载的字段独立倍率。
    ) -> Self {
        Self {
            mapping: NumericInputScrubMapping::Exponential { exponent_per_pixel }, // 保留低值精调空间。
            drag_step,                           // 保存最终吸附粒度。
            minimum,                             // 保存进度对数映射和拖动范围下界。
            maximum,                             // 保存进度对数映射和拖动范围上界。
            sensitivity,                         // 保存字段自己的当前倍率。
            minimum_sensitivity: 0.01,           // 默认允许百倍精调。
            maximum_sensitivity: 100.0,          // 默认限制百倍快调。
            drag_threshold_pixels: 4.0,          // 与线性字段共享稳定手势阈值。
            sensitivity_pixels_per_octave: 48.0, // 与线性字段共享右拖倍率行程。
        }
    }

    /// 覆盖右拖灵敏度允许范围和翻倍行程，其余专业手势保持标准行为。
    pub const fn with_sensitivity_range(
        mut self,               // 接收已经声明业务映射的配置。
        minimum: f32,           // 最细允许倍率。
        maximum: f32,           // 最快允许倍率。
        pixels_per_octave: f32, // 倍率翻倍或减半需要的物理像素。
    ) -> Self {
        self.minimum_sensitivity = minimum; // 保存调用方专用最小倍率。
        self.maximum_sensitivity = maximum; // 保存调用方专用最大倍率。
        self.sensitivity_pixels_per_octave = pixels_per_octave; // 保存专用右拖行程。
        self // 继续支持链式配置。
    }

    /// 覆盖区分点击抖动和正式拖动的物理像素阈值。
    pub const fn with_drag_threshold(mut self, pixels: f32) -> Self {
        self.drag_threshold_pixels = pixels; // 公共拖动仲裁会对非法负数安全钳制。
        self // 继续支持链式配置。
    }
}

/// 数字字段自身持有的主题；游戏与编辑器共享输入状态但互不覆盖外观。
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct NumericInputPalette {
    pub normal_bg: Color,
    pub normal_border: Color,
    pub hovered_bg: Color,
    pub active_bg: Color,
    pub active_border: Color,
    pub disabled_bg: Color,
    pub disabled_border: Color,
}
impl Default for NumericInputPalette {
    fn default() -> Self {
        Self {
            normal_bg: Color::srgb(0.10, 0.10, 0.10),
            normal_border: Color::srgb(0.30, 0.30, 0.30),
            hovered_bg: Color::srgb(0.15, 0.15, 0.15),
            active_bg: Color::srgb(0.20, 0.20, 0.20),
            active_border: Color::srgb(0.60, 0.60, 0.60),
            disabled_bg: Color::srgb(0.05, 0.05, 0.05),
            disabled_border: Color::srgb(0.15, 0.15, 0.15),
        } // 精简宿主的中性托底；正式字段由自己的 builder 显式注入。
    }
}
