//! 主题无关的按钮描述、数值规范和拖动数据类型。

use crate::ButtonColors;
use bevy::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct UiButtonVisual {
    pub normal: Color,
    pub hovered: Color,
    pub pressed: Color,
    pub border: Option<Color>,
    pub text: Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericValueKind {
    U32, // 无符号离散量继续用于既有数量输入。
    I32, // 有符号离散量允许优先级等作者参数直接复用公共控件。
    F32, // 连续量继续使用受小数位约束的浮点输入。
}

/// 数值越过输入上下界时采用的统一边界语义。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NumericInputBoundaryMode {
    #[default]
    Clamp, // 普通参数停在闭区间端点，保持既有数字框行为。
    Wrap, // 角度等周期量在`[min, max)`半开区间内循环。
}

/// 数字框左键拖动时使用的相对数值映射；整数和浮点字段共享同一映射入口。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumericInputScrubMapping {
    /// 每个物理像素增加固定业务单位，适合百分比和世界高度等线性量。
    Linear {
        units_per_pixel: f32, // 未乘用户灵敏度前的基础业务单位/像素。
    },
    /// 每个物理像素按指数改变正数，适合跨度很大的半径和速度。
    Exponential {
        exponent_per_pixel: f32, // 未乘用户灵敏度前的自然指数/像素。
    },
}

/// 当前数字框拖动修改的是业务值还是该字段自己的灵敏度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NumericInputScrubButton {
    Value,       // 左键拖动业务值。
    Sensitivity, // 右键拖动独立灵敏度。
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumericValue {
    U32(u32), // 无符号整数业务值。
    I32(i32), // 有符号整数业务值。
    F32(f32), // 单精度浮点业务值。
}

impl NumericValue {
    pub fn kind(self) -> NumericValueKind {
        match self {
            Self::U32(_) => NumericValueKind::U32,
            Self::I32(_) => NumericValueKind::I32, // 有符号整数保持独立类型身份。
            Self::F32(_) => NumericValueKind::F32,
        }
    }

    pub fn as_u32(self) -> Option<u32> {
        match self {
            Self::U32(value) => Some(value),
            Self::I32(_) | Self::F32(_) => None, // 不做可能丢失符号或精度的隐式转换。
        }
    }

    /// 仅在值本身是有符号整数时返回，业务层可显式选择所需表示。
    pub fn as_i32(self) -> Option<i32> {
        match self {
            Self::I32(value) => Some(value),     // 保留完整有符号离散值。
            Self::U32(_) | Self::F32(_) => None, // 跨类型转换统一交给输入规范化边界。
        }
    }

    pub fn as_f32(self) -> Option<f32> {
        match self {
            Self::U32(_) | Self::I32(_) => None, // 整数不会在读取辅助函数中悄悄转为浮点。
            Self::F32(value) => Some(value),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumericInputSpec {
    pub kind: NumericValueKind,                  // 输入值的正式业务类型。
    pub min: Option<NumericValue>,               // 可选业务下界。
    pub max: Option<NumericValue>,               // 可选业务上界。
    pub step: Option<NumericValue>,              // 离散按钮和拖动吸附共用的推荐步长。
    pub boundary_mode: NumericInputBoundaryMode, // 越界时钳制或周期循环。
    pub editable: bool,                          // 是否允许进入精确字符编辑。
    pub show_step_buttons: bool,                 // 旧式数值行是否显示加减按钮。
    pub live_update: bool,                       // 字符缓冲解析成功时是否发布即时变化。
    pub replace_on_first_key: bool,              // 聚焦后首个字符是否覆盖原显示值。
    pub decimals: u8,                            // 浮点格式化和录入允许的小数位数。
}

impl NumericInputSpec {
    pub const fn u32() -> Self {
        Self {
            kind: NumericValueKind::U32,
            min: None,
            max: None,
            step: Some(NumericValue::U32(1)),
            boundary_mode: NumericInputBoundaryMode::Clamp, // 兼容既有无符号整数钳制行为。
            editable: true,
            show_step_buttons: true,
            live_update: false,
            replace_on_first_key: false,
            decimals: 0,
        }
    }

    /// 创建可录入负号的有符号整数规范。
    pub const fn i32() -> Self {
        Self {
            kind: NumericValueKind::I32,      // 选择正式有符号整数解析路径。
            min: None,                        // 默认允许完整i32下界。
            max: None,                        // 默认允许完整i32上界。
            step: Some(NumericValue::I32(1)), // 默认每次步进一个整数单位。
            boundary_mode: NumericInputBoundaryMode::Clamp, // 普通优先级停在边界端点。
            editable: true, // 默认允许控件约定的精确输入手势；专业数值框使用中键。
            show_step_buttons: true, // 保持与u32构造器一致的旧式数值行默认值。
            live_update: false, // 字符编辑默认只在提交时传播。
            replace_on_first_key: false, // 默认延续原数值缓冲而非首键覆盖。
            decimals: 0,    // 整数不接受小数部分。
        }
    }

    pub const fn f32(decimals: u8) -> Self {
        Self {
            kind: NumericValueKind::F32,
            min: None,
            max: None,
            step: Some(NumericValue::F32(1.0)),
            boundary_mode: NumericInputBoundaryMode::Clamp, // 兼容既有浮点范围钳制行为。
            editable: true,
            show_step_buttons: true,
            live_update: false,
            replace_on_first_key: false,
            decimals,
        }
    }

    pub const fn with_min(mut self, min: NumericValue) -> Self {
        self.min = Some(min);
        self
    }

    pub const fn with_max(mut self, max: NumericValue) -> Self {
        self.max = Some(max);
        self
    }

    pub const fn with_step(mut self, step: NumericValue) -> Self {
        self.step = Some(step);
        self
    }

    /// 设置统一边界语义；周期量必须同时提供同类型且有序的最小值和最大值。
    pub const fn with_boundary_mode(mut self, boundary_mode: NumericInputBoundaryMode) -> Self {
        self.boundary_mode = boundary_mode; // 精确输入、步进、外部赋值和拖动都读取同一字段。
        self // 继续支持链式构造。
    }

    /// 以布尔值便捷切换周期边界，适合由业务配置直接决定角度是否循环。
    pub const fn with_wrap(mut self, wrap: bool) -> Self {
        self.boundary_mode = if wrap {
            NumericInputBoundaryMode::Wrap // 启用`[min, max)`循环。
        } else {
            NumericInputBoundaryMode::Clamp // 关闭后恢复普通端点钳制。
        };
        self // 继续支持链式构造。
    }

    pub const fn with_editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }

    pub const fn with_step_buttons(mut self, show_step_buttons: bool) -> Self {
        self.show_step_buttons = show_step_buttons;
        self
    }

    pub const fn with_live_update(mut self, live_update: bool) -> Self {
        self.live_update = live_update;
        self
    }

    pub const fn with_replace_on_first_key(mut self, replace_on_first_key: bool) -> Self {
        self.replace_on_first_key = replace_on_first_key;
        self
    }

    pub const fn with_decimals(mut self, decimals: u8) -> Self {
        self.decimals = decimals;
        self
    }
}

pub struct TextButtonStyle {
    // 自定义文本按钮样式，供业务 UI 复用同一套按钮实体结构。
    pub node: Node,                  // 按钮布局节点。
    pub colors: ButtonColors,        // 按钮三态背景色。
    pub font_size: f32,              // 按钮文字字号。
    pub text_color: Color,           // 按钮文字颜色。
    pub border_color: Option<Color>, // 可选边框色。
}

pub struct ButtonShellStyle {
    pub node: Node,
    pub colors: ButtonColors,
    pub border_color: Option<Color>,
}

/// 滑块刻度项。
#[derive(Clone, Copy)] // 只保存简单标量和静态文本，Copy 能让调用侧轻松传数组。
pub struct SliderTick {
    // 描述一个刻度文字在轨道上的视觉位置。
    pub px: f32, // 刻度中心附近的横向像素位置，和旧业务代码里的 px 语义保持一致。
    pub label: &'static str, // 刻度显示文本，例如 "30"、"∞"、"0.50"。
}

/// 滑块视觉参数。
#[derive(Clone, Copy)] // 参数都是数值，Copy 方便按值传入。
pub struct SliderVisual {
    // 用于覆盖默认滑块尺寸/刻度布局。
    pub track_width: f32,    // 轨道宽度，单位是 UI 逻辑像素。
    pub track_height: f32,   // 轨道高度，单位是 UI 逻辑像素。
    pub thumb_width: f32,    // 手柄宽度，单位是 UI 逻辑像素。
    pub thumb_height: f32,   // 手柄高度，单位是 UI 逻辑像素。
    pub tick_width: f32,     // 刻度文字容器宽度，用于居中刻度文本。
    pub tick_top: f32,       // 刻度文字相对轨道顶部的偏移。
    pub tick_font_size: f32, // 刻度文字字号。
}

impl Default for SliderVisual {
    // 默认视觉保持设置页当前滑块外观。
    fn default() -> Self {
        // 返回统一默认滑块参数。
        Self {
            // 组装默认视觉参数。
            track_width: 200.0,   // 默认轨道宽度 200px。
            track_height: 16.0,   // 默认轨道高度 16px。
            thumb_width: 14.0,    // 默认手柄宽度 14px。
            thumb_height: 26.0,   // 默认手柄高度 26px。
            tick_width: 24.0,     // 默认刻度宽度 24px，适合 FPS 刻度。
            tick_top: 18.0,       // 默认刻度在轨道下方 18px。
            tick_font_size: 10.0, // 默认刻度字号 10px。
        }
    }
}

/// 专业属性框的完整色板，静态节点和输入状态使用同一主题。
#[derive(Clone, Copy)]
pub struct NumericScrubFieldPalette {
    pub input: crate::NumericInputPalette,
    pub fill: Color,
    pub label: Color,
    pub value: Color,
    pub unit: Color,
    pub shadow: TextShadow,
}
/// 滑块实体树需要的静态语义色。
#[derive(Clone, Copy)]
pub struct SliderPalette {
    pub track: Color,
    pub border: Color,
    pub tick: Color,
    pub thumb: Color,
    pub shadow: TextShadow,
}
