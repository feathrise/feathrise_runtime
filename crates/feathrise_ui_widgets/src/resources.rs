//! 游戏与编辑器共用的唯一数值焦点及指针捕获资源。

use bevy::prelude::*;

/// 只记住上轮实际命中的少量控件，离开时局部清理；稳定帧不扫描全部三态实体。
#[derive(Resource, Default)]
pub(crate) struct UiInteractionTargets {
    pub(crate) hovered: bevy::ecs::entity::EntityHashSet,
}

use crate::types::{NumericInputScrubButton, NumericValue}; // 拖动捕获保存按键职责和按下时数值快照。

#[derive(Resource, Default, Debug)]
pub struct ActiveNumericInput {
    pub entity: Option<Entity>,
}

/// 全部公共数字框共享一个指针捕获，保证一次只拖动一个业务值或一项灵敏度。
#[derive(Resource, Default, Debug)]
pub struct ActiveNumericInputScrub {
    pub entity: Option<Entity>, // 对外只暴露当前是否存在拖动，供业务快捷键和关闭门阻止冲突操作。
    pub(crate) button: Option<NumericInputScrubButton>, // 左键数值或右键灵敏度职责。
    pub(crate) start_cursor_x: f32, // 按下时冻结的物理像素X；整笔使用同一坐标系，不跟随字段布局漂移。
    pub(crate) start_value: Option<NumericValue>, // 左拖开始前的规范化数值。
    pub(crate) start_sensitivity: f32, // 右拖开始前的独立灵敏度倍率。
    pub(crate) dragged: bool,   // 是否曾超过四像素阈值；一旦成立，回到原点也仍是拖动。
    pub(crate) canceled_button: Option<NumericInputScrubButton>, // Esc收口后等待原物理按压释放，不能重放迟到移动。
}

impl ActiveNumericInputScrub {
    /// 数值事务已经取消也仍持有原按压；释放后的静默帧才允许其他鼠标工具接管。
    pub fn owns_mouse(&self) -> bool {
        self.entity.is_some() || self.canceled_button.is_some()
    }
}

/// 兼容手工创建的旧字段；新字段的组件色板优先于默认值。
#[derive(Resource, Default, Clone, Copy)]
pub struct DefaultNumericInputPalette(pub crate::components::NumericInputPalette);
