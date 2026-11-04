//! 输入法判定、按钮交互及数值解析的主题无关纯函数。

use crate::{
    ButtonColors, NumericInputBoundaryMode, NumericInputSpec, NumericValue, NumericValueKind,
    UiButtonVisual,
};
use bevy::prelude::*;

pub fn button_released(interaction: &Interaction, mouse: &ButtonInput<MouseButton>) -> bool {
    *interaction == Interaction::Hovered && mouse.just_released(MouseButton::Left)
}

/// 判断本帧普通键盘消息是否仍属于操作系统输入法组合过程。
///
/// Windows 等平台可能在一帧内同时发送 `Ime::Commit` 和对应的键盘确认消息；所有自定义
/// 文本框都用这个判定屏蔽拼音、候选确认键和重复提交文本，只保留 IME 的正式提交结果。
pub fn ime_owns_keyboard_input(
    was_composing: bool,
    handled_composition_event: bool,
    current_preedit: &str,
) -> bool {
    was_composing || handled_composition_event || !current_preedit.is_empty()
}

pub fn lighten_color(color: Color, amount: f32) -> Color {
    let c = color.to_srgba();
    Color::srgba(
        (c.red + amount).min(1.0),
        (c.green + amount).min(1.0),
        (c.blue + amount).min(1.0),
        c.alpha,
    )
}

pub fn button_visual(normal: Color, border: Option<Color>, text: Color) -> UiButtonVisual {
    UiButtonVisual {
        normal,
        hovered: lighten_color(normal, 0.12),
        pressed: lighten_color(normal, 0.25),
        border,
        text,
    }
}

pub fn apply_button_visual(
    interaction: &Interaction,
    bg: &mut BackgroundColor,
    border: Option<&mut BorderColor>,
    colors: &mut ButtonColors,
    children: &Children,
    text_color_query: &mut Query<&mut TextColor>,
    visual: UiButtonVisual,
) {
    colors.normal = visual.normal;
    colors.hovered = visual.hovered;
    colors.pressed = visual.pressed;
    bg.0 = match interaction {
        Interaction::Pressed => visual.pressed,
        Interaction::Hovered => visual.hovered,
        Interaction::None => visual.normal,
    };
    if let (Some(border), Some(border_color)) = (border, visual.border) {
        *border = BorderColor::all(border_color);
    }
    set_child_text_color(children, text_color_query, visual.text);
}

pub fn set_child_text(
    children: &Children,
    text_query: &mut Query<&mut Text>,
    value: impl Into<String>,
) {
    let value = value.into();
    for child in children.iter() {
        if let Ok(mut text) = text_query.get_mut(child) {
            text.0 = value.clone();
        }
    }
}

pub fn set_child_text_color(
    children: &Children,
    text_color_query: &mut Query<&mut TextColor>,
    color: Color,
) {
    for child in children.iter() {
        if let Ok(mut text_color) = text_color_query.get_mut(child) {
            text_color.0 = color;
        }
    }
}

pub fn slider_thumb_left(
    display_value: f32,
    display_min: f32,
    display_max: f32,
    track_width: f32,
    thumb_width: f32,
) -> f32 {
    let range = display_max - display_min;
    let rel = (display_value - display_min) / range;
    (rel * track_width - thumb_width / 2.0).clamp(0.0, track_width - thumb_width)
}

mod numeric;
pub use numeric::*;

#[cfg(test)]
mod ime_tests {
    use super::ime_owns_keyboard_input;

    #[test]
    fn ime_owns_keyboard_during_preedit_and_commit_frames() {
        assert!(ime_owns_keyboard_input(false, true, "你"));
        assert!(ime_owns_keyboard_input(true, false, ""));
        assert!(ime_owns_keyboard_input(false, true, ""));
        assert!(!ime_owns_keyboard_input(false, false, ""));
    }
}
