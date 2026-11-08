//! 主题无关的按钮交互、数值输入、提交和拖动状态机。

mod button; // 通用按钮三态颜色初始化与交互视觉。
mod numeric_input; // 数字输入焦点、键盘编辑和提交主循环。
mod numeric_progress; // 专业属性框当前值进度填充自动同步。
mod numeric_scrub; // 可选数字框左右键相对拖动与单击抑制。
mod set_value; // 业务消息到数字输入内部状态的外部赋值桥。

use crate::InteractionButton;
use numeric_scrub::process_numeric_input_scrub; // 数字输入主循环先仲裁单击与左右键拖动。

pub use button::{MissingButtonColors, button_color_system, ensure_button_colors_system};
pub use numeric_input::{NumericInputInteractions, NumericInputState, numeric_input_system};
pub use numeric_progress::sync_numeric_scrub_field_fills; // 插件统一调度进度视觉同步。
pub(crate) use numeric_progress::{position_numeric_scrub_sensitivity_feedback, sync_numeric_scrub_sensitivity_feedback}; // 右拖倍率反馈与业务值模型分离。
pub use set_value::handle_numeric_input_set_value;

use bevy::prelude::*;

use crate::components::NumericInputPalette; // 字段所属 UI 库显式提供主题。
use crate::components::{
    ButtonColors, NumericInputConfig, NumericInputDisabled, NumericInputDisplayText,
    NumericInputEditState, NumericInputFieldButton, NumericInputRoot, NumericInputScrubConfig,
    NumericInputStepButton, NumericInputValue, NumericStepDirection,
};
use crate::helpers::{
    button_released, format_numeric_value, lighten_color, normalize_numeric_value,
    parse_numeric_buffer, push_numeric_char, step_numeric_value,
};
use crate::messages::{
    NumericInputChanged, NumericInputCommitOutcome, NumericInputCommitSource,
    NumericInputCommitted, NumericInputScrubSensitivityChanged,
    NumericInputScrubSensitivityCommitted, NumericInputSetValue,
};
use crate::resources::{ActiveNumericInput, ActiveNumericInputScrub};

fn prepare_numeric_text_input(edit_state: &mut NumericInputEditState) -> bool {
    if edit_state.replace_on_next_input {
        let changed = !edit_state.buffer.is_empty();
        edit_state.buffer.clear();
        edit_state.replace_on_next_input = false;
        return changed;
    }
    false
}

/// 录入一个候选字符并同时维护跨帧“用户确实编辑过”身份。
fn push_numeric_text_input(
    edit_state: &mut NumericInputEditState,
    character: char,
    config: crate::types::NumericInputSpec,
) -> bool {
    let prepared = prepare_numeric_text_input(edit_state);
    let accepted = push_numeric_char(&mut edit_state.buffer, character, config);
    let changed = prepared || accepted;
    edit_state.edited |= changed;
    changed
}

/// 删除当前缓冲末字符；首键替换模式仍会先清空原显示值。
fn pop_numeric_text_input(edit_state: &mut NumericInputEditState) -> bool {
    let prepared = prepare_numeric_text_input(edit_state);
    let popped = edit_state.buffer.pop().is_some();
    let changed = prepared || popped;
    edit_state.edited |= changed;
    changed
}

fn begin_numeric_input_edit(root: Entity, state: &mut NumericInputState<'_, '_>) {
    let Ok((config, value, mut edit_state, disabled)) = state.root_query.get_mut(root) else {
        return;
    };
    if disabled || !config.0.editable {
        state.active.entity = None;
        return;
    }
    edit_state.editing = true;
    edit_state.previous_value = value.0;
    edit_state.buffer = format_numeric_value(value.0, config.0);
    edit_state.replace_on_next_input = config.0.replace_on_first_key;
    edit_state.edited = false;
    refresh_numeric_input_text(root, &edit_state.buffer, &mut state.text_query);
    state.active.entity = Some(root);
}

fn commit_numeric_input(
    root: Entity,
    restore_previous: bool,
    emit_changed_on_commit: bool,
    state: &mut NumericInputState<'_, '_>,
) {
    let Ok((config, mut value, mut edit_state, _)) = state.root_query.get_mut(root) else {
        if state.active.entity == Some(root) {
            state.active.entity = None;
        }
        return;
    };
    let parsed_value = (!restore_previous && !edit_state.buffer.is_empty())
        .then(|| parse_numeric_buffer(&edit_state.buffer, config.0))
        .flatten();
    let outcome = if restore_previous {
        NumericInputCommitOutcome::Canceled
    } else if parsed_value.is_some() {
        NumericInputCommitOutcome::Accepted
    } else {
        NumericInputCommitOutcome::Rejected
    };
    let final_value = match parsed_value {
        Some(parsed) if outcome == NumericInputCommitOutcome::Accepted => parsed,
        _ => edit_state.previous_value,
    };
    let final_value = normalize_numeric_value(final_value, config.0);
    let edited = edit_state.edited;
    let changed_from_previous = edit_state.previous_value != final_value;
    let changed = value.0 != final_value;
    value.0 = final_value;
    edit_state.previous_value = final_value;
    edit_state.editing = false;
    edit_state.buffer.clear();
    edit_state.replace_on_next_input = config.0.replace_on_first_key;
    edit_state.edited = false;
    refresh_numeric_input_text(
        root,
        &format_numeric_value(final_value, config.0),
        &mut state.text_query,
    );
    if state.active.entity == Some(root) {
        state.active.entity = None;
    }
    if emit_changed_on_commit && changed {
        state.changed_writer.write(NumericInputChanged {
            entity: root,
            value: final_value,
        });
    }
    state.committed_writer.write(NumericInputCommitted {
        entity: root,
        value: final_value,
        outcome,
        source: NumericInputCommitSource::Text, // 公共提交函数只收口字符编辑焦点。
        changed: changed_from_previous,
        edited,
    });
}

fn apply_step_to_numeric_input(
    root: Entity,
    direction: NumericStepDirection,
    state: &mut NumericInputState<'_, '_>,
) {
    let Ok((config, mut value, mut edit_state, disabled)) = state.root_query.get_mut(root) else {
        return;
    };
    if disabled {
        return;
    }
    let sign = match direction {
        NumericStepDirection::Decrease => -1.0,
        NumericStepDirection::Increase => 1.0,
    };
    let previous = value.0;
    let next = step_numeric_value(previous, config.0, sign);
    value.0 = next;
    edit_state.previous_value = next;
    edit_state.editing = false;
    edit_state.buffer.clear();
    edit_state.replace_on_next_input = config.0.replace_on_first_key;
    edit_state.edited = false;
    refresh_numeric_input_text(
        root,
        &format_numeric_value(next, config.0),
        &mut state.text_query,
    );
    state.changed_writer.write(NumericInputChanged {
        entity: root,
        value: next,
    });
    state.committed_writer.write(NumericInputCommitted {
        entity: root,
        value: next,
        outcome: NumericInputCommitOutcome::Accepted,
        source: NumericInputCommitSource::Step, // 加减按钮是独立离散提交，不冒充拖动事务。
        changed: previous != next,
        edited: true,
    });
}

fn refresh_numeric_input_text(
    root: Entity,
    value: &str,
    text_query: &mut Query<(&NumericInputDisplayText, &mut Text)>,
) {
    for (marker, mut text) in text_query.iter_mut() {
        if marker.root == root {
            text.0 = value.to_string();
        }
    }
}

fn sync_numeric_input_field_visuals(
    active_root: Option<Entity>,
    disabled_query: &Query<(), With<NumericInputDisabled>>,
    default_palette: NumericInputPalette, // 兼容未显式指定主题的旧字段。
    field_query: &mut Query<
        (
            &NumericInputFieldButton,
            &Interaction,
            &mut ButtonColors,
            &mut BackgroundColor,
            &mut BorderColor,
            Option<&NumericInputPalette>,
        ),
        With<InteractionButton>,
    >,
) {
    for (field, interaction, mut colors, mut background, mut border, palette) in
        field_query.iter_mut()
    {
        let palette = palette.copied().unwrap_or(default_palette); // 焦点刷新继续使用字段自身的主题。
        let disabled = disabled_query.get(field.root).is_ok();
        let (normal, border_color) = if disabled {
            (palette.disabled_bg, palette.disabled_border)
        } else if active_root == Some(field.root) {
            (palette.active_bg, palette.active_border)
        } else {
            (palette.normal_bg, palette.normal_border)
        };
        colors.normal = normal;
        colors.hovered = lighten_color(normal, 0.08);
        colors.pressed = lighten_color(normal, 0.16);
        background.0 = match interaction {
            Interaction::Pressed => colors.pressed,
            Interaction::Hovered => colors.hovered,
            Interaction::None => colors.normal,
        };
        *border = BorderColor::all(border_color);
    }
}
