//! 游戏与编辑器共用的无主题 UI 控件层。
//! 数值类型、消息、焦点与拖动捕获只有一份；主题库只提供视觉参数。
pub mod components;
pub mod helpers;
mod interaction;
pub mod messages;
pub mod resources;
mod systems;
pub mod types;
pub mod widgets;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
pub use components::*;
pub use helpers::*;
pub use interaction::UiInteractionPlugin;
pub use messages::*;
pub use resources::*;
pub use systems::{
    MissingButtonColors, NumericInputInteractions, NumericInputState, button_color_system,
    ensure_button_colors_system, handle_numeric_input_set_value, numeric_input_system,
    sync_numeric_scrub_field_fills,
};
pub use types::*;
pub use widgets::*;
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub enum UiWidgetsSystemSet {
    /// 输入先统一提交，游戏或编辑器业务随后消费本帧消息。
    NumericInput,
}
pub struct UiWidgetsPlugin;
impl Plugin for UiWidgetsPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<UiInteractionPlugin>() {
            app.add_plugins(UiInteractionPlugin);
        }
        app.init_resource::<ActiveNumericInput>()
            .init_resource::<ActiveNumericInputScrub>() // 所有界面共用一个指针捕获，不会各自持有一次拖动。
            .init_resource::<DefaultNumericInputPalette>()
            .add_message::<KeyboardInput>() // 精简宿主也能接收实际字符消息；与 InputPlugin 重复注册幂等。
            .add_message::<bevy::picking::pointer::PointerInput>() // 复用官方有序指针坐标，不从本帧终点倒推按下位置。
            .add_message::<NumericInputChanged>()
            .add_message::<NumericInputCommitted>()
            .add_message::<NumericInputScrubSensitivityChanged>() // 拖动中只即时回显灵敏度。
            .add_message::<NumericInputScrubSensitivityCommitted>() // 松开后再由业务持久化一次。
            .add_message::<NumericInputSetValue>()
            .add_systems(
                Update,
                (
                    ensure_button_colors_system,
                    button_color_system,
                    handle_numeric_input_set_value,
                    numeric_input_system.in_set(UiWidgetsSystemSet::NumericInput),
                    sync_numeric_scrub_field_fills,
                    systems::sync_numeric_scrub_sensitivity_feedback, // 倍率变化同帧可读，松键立即收起提示。
                )
                    .chain(),
            )
            .add_systems(
                PostUpdate,
                systems::position_numeric_scrub_sensitivity_feedback
                    .after(bevy::ui::UiSystems::PostLayout), // 按实际字体尺寸避让窗口边缘，不改变原字段布局。
            );
    }
}
