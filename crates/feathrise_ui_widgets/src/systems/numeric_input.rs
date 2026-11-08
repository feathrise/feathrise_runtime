// 数字输入交互主循环：统一处理步进、焦点、键盘编辑、提交与取消。

use crate::InteractionButton;
use bevy::ecs::system::SystemParam; // 把输入源与控件状态分别收拢为高内聚系统参数。
use bevy::input::keyboard::{Key, KeyboardInput}; // 使用Bevy 0.19保留键盘布局结果的文本消息读取负号等字符。
use bevy::ui::RelativeCursorPosition; // 可拖动字段以节点中心为原点换算物理像素位移。

use super::*; // 复用父模块中的数字输入组件、消息和私有操作辅助函数。

/// 数字输入根实体的可写模型数据。
type NumericRootQueryData = (
    &'static NumericInputConfig,
    &'static mut NumericInputValue,
    &'static mut NumericInputEditState,
    Has<NumericInputDisabled>,
);
/// 数字输入显示文本查询数据。
type NumericTextQueryData = (&'static NumericInputDisplayText, &'static mut Text);
/// 字段按钮视觉查询数据。
type NumericFieldQueryData = (
    &'static NumericInputFieldButton,
    &'static Interaction,
    &'static mut ButtonColors,
    &'static mut BackgroundColor,
    &'static mut BorderColor,
    Option<&'static NumericInputPalette>,
);

/// 只读交互源：按钮状态、键盘和鼠标输入。
#[derive(SystemParam)]
pub struct NumericInputInteractions<'w, 's> {
    step_buttons: Query<
        'w,
        's,
        (&'static Interaction, &'static NumericInputStepButton),
        Changed<Interaction>,
    >,
    field_clicks: Query<
        'w,
        's,
        (&'static Interaction, &'static NumericInputFieldButton),
        Changed<Interaction>,
    >,
    field_states: Query<'w, 's, (&'static Interaction, &'static NumericInputFieldButton)>,
    keyboard_inputs: MessageReader<'w, 's, KeyboardInput>,
    pub(super) keyboard: Res<'w, ButtonInput<KeyCode>>,
    pub(super) mouse: Res<'w, ButtonInput<MouseButton>>, // 拖动扩展与普通失焦提交共享同一鼠标边沿。
    pub(super) scrub_fields: Query<
        'w,
        's,
        (
            &'static NumericInputFieldButton, // 字段反查业务数字输入根。
            &'static Interaction,             // Bevy命中测试只把真正顶层字段标记为Hover/Pressed。
            &'static RelativeCursorPosition,  // 鼠标离开字段后仍保留相对拖动坐标。
            &'static ComputedNode,            // 归一化X乘物理宽度得到稳定像素阈值。
        ),
        Without<NumericInputDisabled>, // 禁用的底层字段不能遮断同位置仍可编辑的顶层字段。
    >,
    pub(super) ui_hierarchy: Query<
        'w,
        's,
        (
            &'static Node,                        // 任一祖先Display::None都使字段不可命中。
            Option<&'static Visibility>,          // 直接Visibility::Hidden同样关闭交互。
            Option<&'static InheritedVisibility>, // 继承隐藏在层级传播后提供第二道门。
            Option<&'static ChildOf>,             // 仅在按键边沿向上检查短UI路径。
        ),
    >,
    pub(super) scrub_configs: Query<'w, 's, &'static mut NumericInputScrubConfig>, // 右拖只修改当前根的独立倍率。
    pub(super) active_scrub: ResMut<'w, ActiveNumericInputScrub>, // 全部字段共享唯一左右键捕获。
    pub(super) ordered_pointer: super::numeric_scrub::OrderedPointer<'w, 's>,
    pub(super) sensitivity_changed: MessageWriter<'w, NumericInputScrubSensitivityChanged>, // 右拖途中只通知状态栏即时回显。
    pub(super) sensitivity_committed: MessageWriter<'w, NumericInputScrubSensitivityCommitted>, // 右键松开后才通知业务持久化。
}

/// 数字输入控件的焦点、模型、文本、视觉和领域消息出口。
#[derive(SystemParam)]
pub struct NumericInputState<'w, 's> {
    pub(super) active: ResMut<'w, ActiveNumericInput>,
    default_palette: Option<Res<'w, crate::resources::DefaultNumericInputPalette>>, // 精简测试可省略主题资源。
    pub(super) root_query: Query<'w, 's, NumericRootQueryData, With<NumericInputRoot>>,
    pub(super) text_query: Query<'w, 's, NumericTextQueryData>,
    pub(super) field_query: Query<'w, 's, NumericFieldQueryData, With<InteractionButton>>,
    pub(super) disabled_query: Query<'w, 's, (), With<NumericInputDisabled>>,
    pub(super) changed_writer: MessageWriter<'w, NumericInputChanged>,
    pub(super) committed_writer: MessageWriter<'w, NumericInputCommitted>,
}

/// 把支持的物理按键映射为数字编辑缓冲字符。
fn numeric_character(key: &KeyCode) -> Option<char> {
    match key {
        KeyCode::Digit0 | KeyCode::Numpad0 => Some('0'),
        KeyCode::Digit1 | KeyCode::Numpad1 => Some('1'),
        KeyCode::Digit2 | KeyCode::Numpad2 => Some('2'),
        KeyCode::Digit3 | KeyCode::Numpad3 => Some('3'),
        KeyCode::Digit4 | KeyCode::Numpad4 => Some('4'),
        KeyCode::Digit5 | KeyCode::Numpad5 => Some('5'),
        KeyCode::Digit6 | KeyCode::Numpad6 => Some('6'),
        KeyCode::Digit7 | KeyCode::Numpad7 => Some('7'),
        KeyCode::Digit8 | KeyCode::Numpad8 => Some('8'),
        KeyCode::Digit9 | KeyCode::Numpad9 => Some('9'),
        KeyCode::Period | KeyCode::NumpadDecimal => Some('.'),
        KeyCode::Minus | KeyCode::NumpadSubtract => Some('-'), // 无文本消息的测试环境和数字小键盘仍可录入负数。
        _ => None,
    }
}

/// Unified numeric input interaction system.
pub fn numeric_input_system(
    mut interactions: NumericInputInteractions,
    mut state: NumericInputState,
) {
    let scrub_frame = process_numeric_input_scrub(&mut interactions, &mut state); // 先区分单击、左拖数值与右拖灵敏度。
    if scrub_frame.consume_frame {
        interactions.keyboard_inputs.clear(); // 拖动期间不让同帧数字键污染任何精确输入缓冲。
        sync_numeric_input_field_visuals(
            state.active.entity,   // 拖动开始时已经提交并释放旧精确输入焦点。
            &state.disabled_query, // 禁用字段继续使用公共暗色视觉。
            state
                .default_palette
                .as_ref()
                .map(|palette| palette.0)
                .unwrap_or_default(),
            &mut state.field_query, // 左键Pressed与右键Hovered均只更新固定字段实体。
        );
        return; // 指针仍被拖动捕获时不再处理步进、单击或键盘编辑。
    }
    if let Some((root, direction)) =
        interactions
            .step_buttons
            .iter()
            .find_map(|(interaction, button)| {
                button_released(interaction, &interactions.mouse)
                    .then_some((button.root, button.direction))
            })
    {
        if let Some(active_root) = state.active.entity {
            commit_numeric_input(active_root, false, false, &mut state);
        }
        apply_step_to_numeric_input(root, direction, &mut state);
        sync_numeric_input_field_visuals(
            state.active.entity,
            &state.disabled_query,
            state
                .default_palette
                .as_ref()
                .map(|palette| palette.0)
                .unwrap_or_default(),
            &mut state.field_query,
        );
        interactions.keyboard_inputs.clear(); // 步进按钮已结束本帧编辑，丢弃同帧键盘消息避免下次聚焦时补录旧字符。
        return;
    }

    if let Some(clicked_root) = scrub_frame.precise_edit_root.or_else(|| {
        interactions
            .field_clicks
            .iter()
            .find_map(|(interaction, button)| {
                (scrub_frame.suppressed_click_root != Some(button.root)
                    // 专业字段只能由本次捕获的中键进入文本编辑；翻页后的旧左键松开不能借新布局重新聚焦。
                    && interactions.scrub_configs.get(button.root).is_err()
                    && button_released(interaction, &interactions.mouse))
                .then_some(button.root) // 非专业字段仍保留普通左键精确输入。
            })
    }) && state.active.entity != Some(clicked_root)
    {
        if let Some(active_root) = state.active.entity {
            commit_numeric_input(active_root, false, false, &mut state);
        }
        begin_numeric_input_edit(clicked_root, &mut state);
    }

    let Some(active_root) = state.active.entity else {
        interactions.keyboard_inputs.clear(); // 没有编辑焦点时仍推进消息游标，防止以后激活时读取历史按键。
        sync_numeric_input_field_visuals(
            state.active.entity,
            &state.disabled_query,
            state
                .default_palette
                .as_ref()
                .map(|palette| palette.0)
                .unwrap_or_default(),
            &mut state.field_query,
        );
        return;
    };

    let active_input_hovered = interactions
        .field_states
        .iter()
        .find_map(|(interaction, button)| {
            (button.root == active_root).then_some(matches!(
                *interaction,
                Interaction::Hovered | Interaction::Pressed
            ))
        })
        .unwrap_or(false);

    let (should_finish, restore_previous) = {
        let Ok((config, mut value, mut edit_state, disabled)) =
            state.root_query.get_mut(active_root)
        else {
            interactions.keyboard_inputs.clear(); // 根实体已消失时同步清理未消费消息和悬空焦点。
            state.active.entity = None;
            sync_numeric_input_field_visuals(
                state.active.entity,
                &state.disabled_query,
                state
                    .default_palette
                    .as_ref()
                    .map(|palette| palette.0)
                    .unwrap_or_default(),
                &mut state.field_query,
            );
            return;
        };
        let config = config.0;
        if !edit_state.editing {
            interactions.keyboard_inputs.clear(); // 外部取消编辑后不让当前消息滞留到下一次编辑。
            state.active.entity = None;
            sync_numeric_input_field_visuals(
                state.active.entity,
                &state.disabled_query,
                state
                    .default_palette
                    .as_ref()
                    .map(|palette| palette.0)
                    .unwrap_or_default(),
                &mut state.field_query,
            );
            return;
        }

        let mut restore_previous = false;
        let mut should_finish = false;
        let mut input_changed = false;

        if !disabled && config.editable {
            // 正常运行时优先读取KeyboardInput.text，避免键盘布局与物理KeyCode不一致；没有消息时再兼容原有ButtonInput测试路径。
            let mut received_pressed_keyboard_input = false;
            for input in interactions.keyboard_inputs.read() {
                if !input.state.is_pressed() {
                    continue;
                }
                received_pressed_keyboard_input = true;
                match &input.logical_key {
                    Key::Backspace => {
                        input_changed |= pop_numeric_text_input(&mut edit_state);
                    }
                    Key::Enter => {
                        should_finish = true;
                    }
                    Key::Escape => {
                        restore_previous = true;
                        should_finish = true;
                    }
                    _ => {
                        if let Some(text) = input.text.as_deref() {
                            for character in text.chars() {
                                input_changed |=
                                    push_numeric_text_input(&mut edit_state, character, config);
                            }
                        } else if let Some(character) = numeric_character(&input.key_code) {
                            // 部分平台不会给数字小键盘减号生成text，此处仅对同一消息的物理键作精确回退。
                            input_changed |=
                                push_numeric_text_input(&mut edit_state, character, config);
                        }
                    }
                }
            }
            if !received_pressed_keyboard_input {
                for key in interactions.keyboard.get_just_pressed() {
                    if let Some(character) = numeric_character(key) {
                        input_changed |=
                            push_numeric_text_input(&mut edit_state, character, config);
                        continue;
                    }
                    match key {
                        KeyCode::Backspace => {
                            input_changed |= pop_numeric_text_input(&mut edit_state);
                        }
                        KeyCode::Enter | KeyCode::NumpadEnter => {
                            should_finish = true;
                        }
                        KeyCode::Escape => {
                            restore_previous = true;
                            should_finish = true;
                        }
                        _ => {}
                    }
                }
            }
        } else {
            interactions.keyboard_inputs.clear(); // 禁用状态也要消费消息，恢复可编辑后不能补录禁用期间的按键。
        }

        if (interactions.mouse.just_pressed(MouseButton::Left)
            || interactions.mouse.just_released(MouseButton::Left))
            && !active_input_hovered
        {
            should_finish = true;
        }

        if input_changed {
            refresh_numeric_input_text(active_root, &edit_state.buffer, &mut state.text_query);
            if config.live_update
                && let Some(parsed) = if edit_state.buffer.is_empty() {
                    Some(edit_state.previous_value)
                } else {
                    parse_numeric_buffer(&edit_state.buffer, config)
                }
                && value.0 != parsed
            {
                value.0 = parsed;
                state.changed_writer.write(NumericInputChanged {
                    entity: active_root,
                    value: parsed,
                });
            }
        }

        (should_finish, restore_previous)
    }; // 查询借用在提交前自然结束，避免用显式drop操纵临时生命周期。

    if should_finish {
        commit_numeric_input(active_root, restore_previous, false, &mut state);
    }

    sync_numeric_input_field_visuals(
        state.active.entity,
        &state.disabled_query,
        state
            .default_palette
            .as_ref()
            .map(|palette| palette.0)
            .unwrap_or_default(),
        &mut state.field_query,
    );
}
