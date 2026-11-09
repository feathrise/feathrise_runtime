use crate::UiInteraction;
// 数字输入外部赋值桥：业务系统通过消息同步数值，不直接操作控件内部组件。

use crate::InteractionButton;
use super::*; // 复用数字输入查询、格式化和视觉同步辅助函数。

/// Apply external value updates pushed by business systems.
pub fn handle_numeric_input_set_value(
    mut requests: MessageReader<NumericInputSetValue>,
    mut active: ResMut<ActiveNumericInput>,
    default_palette: Option<Res<crate::resources::DefaultNumericInputPalette>>,
    mut root_query: Query<
        (
            &NumericInputConfig,
            &mut NumericInputValue,
            &mut NumericInputEditState,
        ),
        With<NumericInputRoot>,
    >,
    mut text_query: Query<(&NumericInputDisplayText, &mut Text)>,
    mut field_query: Query<
        (
            &NumericInputFieldButton,
            &UiInteraction,
            &mut ButtonColors,
            &mut BackgroundColor,
            &mut BorderColor,
            Option<&NumericInputPalette>,
        ),
        With<InteractionButton>,
    >,
    disabled_query: Query<(), With<NumericInputDisabled>>,
) {
    let mut needs_visual_refresh = false;
    for request in requests.read() {
        let Ok((config, mut value, mut edit_state)) = root_query.get_mut(request.entity) else {
            continue;
        };
        let normalized = normalize_numeric_value(request.value, config.0);
        value.0 = normalized;
        edit_state.previous_value = normalized;
        if request.cancel_edit {
            edit_state.editing = false;
            edit_state.buffer.clear();
            edit_state.replace_on_next_input = config.0.replace_on_first_key;
            edit_state.edited = false;
            if active.entity == Some(request.entity) {
                active.entity = None;
            }
        }
        let display = if edit_state.editing && !request.cancel_edit {
            edit_state.buffer.clone()
        } else {
            format_numeric_value(normalized, config.0)
        };
        refresh_numeric_input_text(request.entity, &display, &mut text_query);
        needs_visual_refresh = true;
    }
    if needs_visual_refresh {
        sync_numeric_input_field_visuals(
            active.entity,
            &disabled_query,
            default_palette
                .as_ref()
                .map(|palette| palette.0)
                .unwrap_or_default(),
            &mut field_query,
        ); // 外部赋值和焦点释放同样保持字段自己的主题。
    }
}
